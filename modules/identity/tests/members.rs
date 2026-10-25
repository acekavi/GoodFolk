use db::testing::app_pool;
use db::{Scope, TenantId, UserId, begin};
use identity::{
    ALL_PERMISSIONS, ALL_ROLES, Acceptor, Grant, GrantSpec, Member, Role, SignupInput, StaffError, accept, allows,
    create_session, describe, invite, members, permission_matrix, resolve_session, revoke_member, set_grants, signup,
};
use mail::MailJob;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

const PASSWORD: &str = "a long enough password";

struct Tenant {
    pool: PgPool,
    /// The test database's owner, which reads the outbox and the audit log.
    superuser: PgPool,
    tenant: TenantId,
    owner: UserId,
    hotel: Uuid,
    other_hotel: Uuid,
}

fn grant(role: Role, property_id: Option<Uuid>) -> GrantSpec {
    GrantSpec { role, property_id }
}

fn signup_input(email: &str, tenant: &str) -> SignupInput {
    SignupInput {
        email: email.into(),
        password: PASSWORD.into(),
        display_name: "Nimal".into(),
        tenant_name: tenant.into(),
    }
}

impl Tenant {
    async fn new(opts: PgConnectOptions) -> Self {
        let superuser = PgPoolOptions::new().max_connections(1).connect_with(opts.clone()).await.unwrap();
        let pool = app_pool(opts, 5).await;
        let (owner, tenant) = signup(&pool, signup_input("owner@example.com", "Lagoon Hotels")).await.unwrap();
        let mut ids = Vec::new();
        for (code, name) in [("GAL", "Galle Fort"), ("KAN", "Kandy Hills")] {
            let id = Uuid::now_v7();
            sqlx::query(
                "insert into property (id, tenant_id, code, name, timezone, base_currency, business_date)
                 values ($1, $2, $3, $4, 'Asia/Colombo', 'LKR', current_date)",
            )
            .bind(id)
            .bind(tenant.0)
            .bind(code)
            .bind(name)
            .execute(&superuser)
            .await
            .unwrap();
            ids.push(id);
        }
        Self { pool, superuser, tenant, owner, hotel: ids[0], other_hotel: ids[1] }
    }

    async fn tx(&self, actor: UserId) -> db::Tx {
        begin(&self.pool, Scope { tenant: Some(self.tenant), user: Some(actor) }).await.unwrap()
    }

    /// A member holding `grants`, invited by the owner and accepted as `acceptor`.
    async fn join(&self, email: &str, grants: &[GrantSpec], acceptor: Acceptor) -> UserId {
        let mut tx = self.tx(self.owner).await;
        let invitation = invite(&mut tx, self.tenant, self.owner, email, "Kamala", grants).await.unwrap();
        tx.commit().await.unwrap();
        let payload: serde_json::Value =
            sqlx::query_scalar("select payload from outbox where topic = $1 order by id desc limit 1")
                .bind(mail::SEND_TOPIC)
                .fetch_one(&self.superuser)
                .await
                .unwrap();
        let MailJob::Invitation { invitation_id, token, .. } = serde_json::from_value(payload).unwrap();
        assert_eq!(invitation_id, invitation.id);
        accept(&self.pool, &token, acceptor).await.unwrap().0
    }

    async fn member(&self, email: &str, grants: &[GrantSpec]) -> UserId {
        self.join(email, grants, Acceptor::NewUser { password: PASSWORD.into() }).await
    }

    /// A second owner: `owner` cannot be invited, so it is granted directly.
    async fn co_owner(&self, email: &str) -> UserId {
        let user = self.member(email, &[grant(Role::FrontDesk, None)]).await;
        sqlx::query(
            "insert into role_grant (id, tenant_id, user_id, property_id, role) values ($1, $2, $3, null, 'owner')",
        )
        .bind(Uuid::now_v7())
        .bind(self.tenant.0)
        .bind(user.0)
        .execute(&self.superuser)
        .await
        .unwrap();
        user
    }

    async fn members(&self) -> Vec<Member> {
        let mut tx = begin(&self.pool, Scope::tenant(self.tenant)).await.unwrap();
        let list = members(&mut tx).await.unwrap();
        tx.commit().await.unwrap();
        list
    }

    async fn find(&self, user: UserId) -> Option<Member> {
        self.members().await.into_iter().find(|m| m.user_id == user.0)
    }

    async fn set_as(&self, actor: UserId, user: UserId, grants: &[GrantSpec]) -> Result<Member, StaffError> {
        let version = self.find(user).await.map_or(1, |m| m.version);
        self.set_at(actor, user, version, grants).await
    }

    async fn set_at(
        &self,
        actor: UserId,
        user: UserId,
        version: i32,
        grants: &[GrantSpec],
    ) -> Result<Member, StaffError> {
        let mut tx = self.tx(actor).await;
        let member = set_grants(&mut tx, self.tenant, actor, user.0, version, grants).await?;
        tx.commit().await.unwrap();
        Ok(member)
    }

    async fn revoke_as(&self, actor: UserId, user: UserId) -> Result<(), StaffError> {
        let mut tx = self.tx(actor).await;
        revoke_member(&mut tx, self.tenant, actor, user.0).await?;
        tx.commit().await.unwrap();
        Ok(())
    }

    async fn audit(&self, action: &str) -> Vec<serde_json::Value> {
        sqlx::query_scalar("select data from audit_log where action = $1 order by at, id")
            .bind(action)
            .fetch_all(&self.superuser)
            .await
            .unwrap()
    }
}

fn forbidden(result: Result<impl std::fmt::Debug, StaffError>) -> String {
    match result {
        Err(StaffError::Forbidden(message)) => message,
        other => panic!("expected Forbidden, got {other:?}"),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn setting_grants_replaces_them_and_moves_the_version(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let kamala = t.member("kamala@example.com", &[grant(Role::FrontDesk, Some(t.hotel))]).await;
    let before = t.find(kamala).await.unwrap();
    assert_eq!((before.email.as_str(), before.version), ("kamala@example.com", 1));
    assert_eq!(before.grants, vec![grant(Role::FrontDesk, Some(t.hotel))]);

    let new = [grant(Role::Housekeeping, Some(t.other_hotel)), grant(Role::Accountant, None)];
    let after = t.set_at(t.owner, kamala, 1, &new).await.unwrap();

    assert_eq!(after.version, 2);
    assert_eq!(after.grants, vec![grant(Role::Accountant, None), grant(Role::Housekeeping, Some(t.other_hotel))]);
    assert_eq!(t.find(kamala).await.unwrap(), after);
    assert!(matches!(t.set_at(t.owner, kamala, 1, &new).await, Err(StaffError::VersionMismatch)));
    let audit = t.audit("membership.grants_changed").await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0]["previous"], serde_json::json!([{ "role": "front_desk", "property_id": t.hotel }]));
    assert_eq!(audit[0]["grants"], serde_json::to_value(&after.grants).unwrap());
    // Every member is listed, the owner included.
    let emails: Vec<String> = t.members().await.into_iter().map(|m| m.email).collect();
    assert_eq!(emails, vec!["kamala@example.com", "owner@example.com"]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn grants_are_validated(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let kamala = t.member("kamala@example.com", &[grant(Role::FrontDesk, None)]).await;

    for grants in [vec![], vec![grant(Role::Pos, Some(Uuid::now_v7()))], vec![grant(Role::Pos, None); 2]] {
        let result = t.set_as(t.owner, kamala, &grants).await;
        assert!(matches!(result, Err(StaffError::Invalid(_))), "{result:?}");
    }
    assert_eq!(
        forbidden(t.set_as(t.owner, kamala, &[grant(Role::Owner, None)]).await),
        "nobody can grant the owner role"
    );
    assert!(matches!(t.set_at(t.owner, UserId(Uuid::now_v7()), 1, &[]).await, Err(StaffError::NotFound(_))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_last_owner_cannot_be_demoted_or_removed(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let second = t.co_owner("second@example.com").await;
    // Another owner's grants can be edited around their owner grant.
    let kept = [grant(Role::Owner, None), grant(Role::Manager, Some(t.hotel))];
    assert_eq!(t.set_as(t.owner, second, &kept).await.unwrap().grants.len(), 2);
    // A transaction begun while the second owner was an owner authorizes against their grants after the lock, so
    // once demoted they can neither demote nor remove the last owner.
    let mut tx = t.tx(second).await;
    t.set_as(t.owner, second, &[grant(Role::Manager, None)]).await.unwrap();

    let version = t.find(t.owner).await.unwrap().version;
    let demoted = set_grants(&mut tx, t.tenant, second, t.owner.0, version, &[grant(Role::Manager, None)]).await;
    assert_eq!(forbidden(demoted), "only owners can manage a member with the owner role");
    let removed = revoke_member(&mut tx, t.tenant, second, t.owner.0).await;
    assert_eq!(forbidden(removed), "only owners can manage a member with the owner role");
    drop(tx);
    assert!(t.find(t.owner).await.unwrap().grants.contains(&grant(Role::Owner, None)));
    // With two owners, one may remove the other.
    let third = t.co_owner("third@example.com").await;
    t.revoke_as(t.owner, third).await.unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn two_owners_demoting_each_other_at_once_leave_one_owner(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let second = t.co_owner("second@example.com").await;
    let manager = [grant(Role::Manager, None)];
    let (first_version, second_version) =
        (t.find(t.owner).await.unwrap().version, t.find(second).await.unwrap().version);
    // Both requests are under way, each holding its own transaction, before either takes the staff lock.
    let mut first_tx = t.tx(t.owner).await;
    let mut second_tx = t.tx(second).await;

    let (first, second_result) = tokio::join!(
        async {
            let result = set_grants(&mut first_tx, t.tenant, t.owner, second.0, second_version, &manager).await;
            if result.is_ok() {
                first_tx.commit().await.unwrap();
            }
            result
        },
        async {
            let result = set_grants(&mut second_tx, t.tenant, second, t.owner.0, first_version, &manager).await;
            if result.is_ok() {
                second_tx.commit().await.unwrap();
            }
            result
        },
    );

    // The staff lock lets one through; the other then sees that it is no longer an owner.
    let results = [first.map(|_| ()), second_result.map(|_| ())];
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1, "{results:?}");
    let refused = results.into_iter().find(Result::is_err).unwrap();
    assert_eq!(forbidden(refused), "only owners can manage a member with the owner role");
    let owners: i64 = sqlx::query_scalar("select count(*) from role_grant where role = 'owner'")
        .fetch_one(&t.superuser)
        .await
        .unwrap();
    assert_eq!(owners, 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn nobody_acts_on_themself(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let manager = t.member("manager@example.com", &[grant(Role::Manager, None)]).await;

    for actor in [t.owner, manager] {
        assert_eq!(
            forbidden(t.set_as(actor, actor, &[grant(Role::Owner, None)]).await),
            "you cannot change your own grants"
        );
        assert_eq!(forbidden(t.revoke_as(actor, actor).await), "you cannot remove yourself");
    }
    assert!(t.find(manager).await.is_some());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn managers_act_only_on_junior_members_at_their_properties(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let manager = t.member("manager@example.com", &[grant(Role::Manager, Some(t.hotel))]).await;
    let other_manager = t.member("other@example.com", &[grant(Role::Manager, Some(t.other_hotel))]).await;
    let mixed = t
        .member(
            "mixed@example.com",
            &[grant(Role::FrontDesk, Some(t.hotel)), grant(Role::Manager, Some(t.other_hotel))],
        )
        .await;
    let elsewhere = t.member("elsewhere@example.com", &[grant(Role::FrontDesk, Some(t.other_hotel))]).await;
    let junior = t.member("junior@example.com", &[grant(Role::FrontDesk, Some(t.hotel))]).await;
    let at_hotel = [grant(Role::Housekeeping, Some(t.hotel))];
    let manager_role = "only owners can manage a member with the manager role";

    assert_eq!(forbidden(t.set_as(manager, other_manager, &at_hotel).await), manager_role);
    assert_eq!(forbidden(t.set_as(manager, mixed, &at_hotel).await), manager_role);
    assert_eq!(forbidden(t.revoke_as(manager, mixed).await), manager_role);
    let not_managed = format!("you do not manage property {}", t.other_hotel);
    assert_eq!(forbidden(t.set_as(manager, elsewhere, &at_hotel).await), not_managed);
    assert_eq!(forbidden(t.revoke_as(manager, elsewhere).await), not_managed);
    // The new grants follow the rules too.
    assert_eq!(forbidden(t.set_as(manager, junior, &[grant(Role::Manager, Some(t.hotel))]).await), manager_role);
    assert_eq!(forbidden(t.set_as(manager, junior, &[grant(Role::FrontDesk, Some(t.other_hotel))]).await), not_managed);
    assert_eq!(
        forbidden(t.set_as(manager, junior, &[grant(Role::Pos, None)]).await),
        "a member with a tenant-wide pos grant needs a tenant-wide manager"
    );

    assert_eq!(t.set_as(manager, junior, &at_hotel).await.unwrap().grants, at_hotel);
    t.revoke_as(manager, junior).await.unwrap();
    // The owner may do all of it.
    t.set_as(t.owner, mixed, &[grant(Role::Manager, None)]).await.unwrap();
    t.revoke_as(t.owner, other_manager).await.unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn revoking_ends_only_that_tenants_sessions(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let (kamala, her_tenant) = signup(&t.pool, signup_input("kamala@example.com", "Her Own Hotel")).await.unwrap();
    let (token, _) = create_session(&t.pool, kamala, None).await.unwrap();
    let session = resolve_session(&t.pool, &token).await.unwrap().unwrap();
    t.join("kamala@example.com", &[grant(Role::FrontDesk, None)], Acceptor::SignedIn(session)).await;
    let (here, _) = create_session(&t.pool, kamala, Some(t.tenant)).await.unwrap();
    let (hers, _) = create_session(&t.pool, kamala, Some(her_tenant)).await.unwrap();

    t.revoke_as(t.owner, kamala).await.unwrap();

    assert_eq!(resolve_session(&t.pool, &here).await.unwrap(), None);
    assert!(resolve_session(&t.pool, &hers).await.unwrap().is_some());
    assert!(t.find(kamala).await.is_none());
    let grants: i64 = sqlx::query_scalar("select count(*) from role_grant where tenant_id = $1 and user_id = $2")
        .bind(t.tenant.0)
        .bind(kamala.0)
        .fetch_one(&t.superuser)
        .await
        .unwrap();
    assert_eq!(grants, 0);
    let audit = t.audit("membership.revoked").await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0]["email"], "kamala@example.com");
    assert!(matches!(t.revoke_as(t.owner, kamala).await, Err(StaffError::NotFound(_))));
}

#[test]
fn the_matrix_lists_every_permission_with_its_roles() {
    let matrix = permission_matrix();

    assert_eq!(matrix.len(), ALL_PERMISSIONS.len());
    for (row, permission) in matrix.iter().zip(ALL_PERMISSIONS) {
        assert_eq!(row.permission, permission.name());
        assert_eq!(row.allows, describe(permission));
        let holders: Vec<Role> = ALL_ROLES
            .into_iter()
            .filter(|role| allows(&[Grant { property_id: None, role: *role }], permission, None))
            .collect();
        assert_eq!(row.roles, holders, "{}", row.permission);
    }
    let staff = matrix.iter().find(|row| row.permission == "StaffManage").unwrap();
    assert_eq!(staff.roles, vec![Role::Owner, Role::Manager]);
    let pos = matrix.iter().find(|row| row.permission == "PosPost").unwrap();
    assert!(pos.roles.contains(&Role::Pos));
}
