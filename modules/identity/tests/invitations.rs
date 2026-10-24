use db::testing::app_pool;
use db::{Scope, TenantId, UserId, begin};
use identity::{
    Acceptor, GrantSpec, Invitation, InvitationStatus, Role, SignupInput, StaffError, accept, authenticate,
    create_session, default_tenant, invite, list_invitations, load_grants, load_profile, preview, resend,
    revoke_invitation, signup,
};
use mail::MailJob;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

const PASSWORD: &str = "a long enough password";

struct Tenant {
    pool: PgPool,
    /// The test database's owner, which reads the outbox and moves the clock.
    superuser: PgPool,
    tenant: TenantId,
    owner: UserId,
    hotel: Uuid,
    other_hotel: Uuid,
}

fn grant(role: Role, property_id: Option<Uuid>) -> GrantSpec {
    GrantSpec { role, property_id }
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

    async fn invite_as(&self, actor: UserId, email: &str, grants: &[GrantSpec]) -> Result<Invitation, StaffError> {
        let mut tx = begin(&self.pool, Scope { tenant: Some(self.tenant), user: Some(actor) }).await.unwrap();
        let actor_grants = load_grants(&mut tx, actor).await.unwrap();
        let invitation = invite(&mut tx, self.tenant, actor, &actor_grants, email, "Kamala", grants).await?;
        tx.commit().await.unwrap();
        Ok(invitation)
    }

    async fn invite(&self, email: &str, grants: &[GrantSpec]) -> Invitation {
        self.invite_as(self.owner, email, grants).await.unwrap()
    }

    async fn resend(&self, id: Uuid) -> Invitation {
        let mut tx = begin(&self.pool, Scope { tenant: Some(self.tenant), user: Some(self.owner) }).await.unwrap();
        let grants = load_grants(&mut tx, self.owner).await.unwrap();
        let invitation = resend(&mut tx, self.tenant, self.owner, &grants, id).await.unwrap();
        tx.commit().await.unwrap();
        invitation
    }

    async fn revoke(&self, id: Uuid) -> Invitation {
        let mut tx = begin(&self.pool, Scope { tenant: Some(self.tenant), user: Some(self.owner) }).await.unwrap();
        let grants = load_grants(&mut tx, self.owner).await.unwrap();
        let invitation = revoke_invitation(&mut tx, self.tenant, self.owner, &grants, id).await.unwrap();
        tx.commit().await.unwrap();
        invitation
    }

    async fn list(&self) -> Vec<Invitation> {
        let mut tx = begin(&self.pool, Scope::tenant(self.tenant)).await.unwrap();
        let invitations = list_invitations(&mut tx, None).await.unwrap();
        tx.commit().await.unwrap();
        invitations
    }

    /// The `mail-send` jobs queued, oldest first.
    async fn mail_jobs(&self) -> Vec<MailJob> {
        let payloads: Vec<serde_json::Value> =
            sqlx::query_scalar("select payload from outbox where topic = $1 order by id")
                .bind(mail::SEND_TOPIC)
                .fetch_all(&self.superuser)
                .await
                .unwrap();
        payloads.into_iter().map(|payload| serde_json::from_value(payload).unwrap()).collect()
    }

    /// The newest token sent for `invitation`, as the email's link carries it.
    async fn token(&self, invitation: Uuid) -> String {
        self.mail_jobs()
            .await
            .into_iter()
            .rev()
            .find_map(|MailJob::Invitation { invitation_id, token, .. }| (invitation_id == invitation).then_some(token))
            .expect("a mail job for the invitation")
    }

    async fn accept_new(&self, token: &str) -> Result<(UserId, TenantId), StaffError> {
        accept(&self.pool, token, Acceptor::NewUser { password: PASSWORD.into() }).await
    }

    async fn grants_of(&self, user: UserId) -> Vec<GrantSpec> {
        let mut tx = begin(&self.pool, Scope::tenant(self.tenant)).await.unwrap();
        let mut grants: Vec<GrantSpec> = load_grants(&mut tx, user)
            .await
            .unwrap()
            .into_iter()
            .map(|g| GrantSpec { role: g.role, property_id: g.property_id })
            .collect();
        tx.commit().await.unwrap();
        grants.sort_by_key(|g| (g.role.as_str(), g.property_id));
        grants
    }

    /// A member holding `grants`, invited by the owner.
    async fn member(&self, email: &str, grants: &[GrantSpec]) -> UserId {
        let invitation = self.invite(email, grants).await;
        self.accept_new(&self.token(invitation.id).await).await.unwrap().0
    }
}

fn signup_input(email: &str, tenant: &str) -> SignupInput {
    SignupInput {
        email: email.into(),
        password: PASSWORD.into(),
        display_name: "Nimal".into(),
        tenant_name: tenant.into(),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_new_user_gets_exactly_the_invited_grants_once(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let mut grants = vec![grant(Role::FrontDesk, Some(t.hotel)), grant(Role::Accountant, None)];
    let invitation = t.invite("Kamala@Example.com", &grants).await;
    let token = t.token(invitation.id).await;

    assert_eq!(invitation.email, "kamala@example.com");
    assert_eq!(invitation.status, InvitationStatus::Pending);
    let shown = preview(&t.pool, &token).await.unwrap();
    assert_eq!(shown.tenant_name, "Lagoon Hotels");
    assert_eq!(shown.inviter.as_deref(), Some("Nimal"));
    assert_eq!(shown.grants, vec![(Role::FrontDesk, Some("Galle Fort".into())), (Role::Accountant, None)]);
    assert!(!shown.existing_account);

    let (user, tenant) = t.accept_new(&token).await.unwrap();

    assert_eq!(tenant, t.tenant);
    grants.sort_by_key(|g| (g.role.as_str(), g.property_id));
    assert_eq!(t.grants_of(user).await, grants);
    assert_eq!(authenticate(&t.pool, "kamala@example.com", PASSWORD).await.unwrap(), Some(user));
    let version: i32 = sqlx::query_scalar("select version from membership where user_id = $1")
        .bind(user.0)
        .fetch_one(&t.superuser)
        .await
        .unwrap();
    assert_eq!(version, 1);
    assert_eq!(t.list().await[0].status, InvitationStatus::Accepted);
    assert!(matches!(t.accept_new(&token).await, Err(StaffError::Gone)));
    assert!(matches!(accept(&t.pool, &token, Acceptor::SignedIn(user)).await, Err(StaffError::Gone)));
    assert!(matches!(preview(&t.pool, &token).await, Err(StaffError::Gone)));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_expired_invitation_is_gone_and_listed_as_expired(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let invitation = t.invite("kamala@example.com", &[grant(Role::Housekeeping, Some(t.hotel))]).await;
    let token = t.token(invitation.id).await;
    sqlx::query("update invitation set expires_at = now() - interval '1 second'").execute(&t.superuser).await.unwrap();

    assert!(matches!(preview(&t.pool, &token).await, Err(StaffError::Gone)));
    assert!(matches!(t.accept_new(&token).await, Err(StaffError::Gone)));
    assert_eq!(t.list().await[0].status, InvitationStatus::Expired);
    let stored: String = sqlx::query_scalar("select status from invitation").fetch_one(&t.superuser).await.unwrap();
    assert_eq!(stored, "expired");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_resend_replaces_the_token(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let invitation = t.invite("kamala@example.com", &[grant(Role::Pos, Some(t.hotel))]).await;
    let old = t.token(invitation.id).await;

    let resent = t.resend(invitation.id).await;
    let new = t.token(invitation.id).await;

    assert_ne!(old, new);
    assert!(resent.expires_at >= invitation.expires_at);
    assert_eq!(t.mail_jobs().await.len(), 2);
    assert!(matches!(t.accept_new(&old).await, Err(StaffError::Gone)));
    assert!(t.accept_new(&new).await.is_ok());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_expired_invitation_can_be_resent(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let invitation = t.invite("kamala@example.com", &[grant(Role::Pos, Some(t.hotel))]).await;
    sqlx::query("update invitation set expires_at = now() - interval '1 second'").execute(&t.superuser).await.unwrap();
    t.list().await;

    let resent = t.resend(invitation.id).await;

    assert_eq!(resent.status, InvitationStatus::Pending);
    assert!(t.accept_new(&t.token(invitation.id).await).await.is_ok());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn revoked_and_unknown_tokens_are_gone(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let invitation = t.invite("kamala@example.com", &[grant(Role::FrontDesk, None)]).await;
    let token = t.token(invitation.id).await;

    assert_eq!(t.revoke(invitation.id).await.status, InvitationStatus::Revoked);

    assert!(matches!(t.accept_new(&token).await, Err(StaffError::Gone)));
    assert!(matches!(preview(&t.pool, &token).await, Err(StaffError::Gone)));
    assert!(matches!(t.accept_new("made-up-token").await, Err(StaffError::Gone)));
    assert!(matches!(preview(&t.pool, "made-up-token").await, Err(StaffError::Gone)));
    assert_eq!(StaffError::Gone.to_string(), "This invitation is no longer valid");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn concurrent_accepts_let_exactly_one_succeed(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let invitation = t.invite("kamala@example.com", &[grant(Role::FrontDesk, None)]).await;
    let token = t.token(invitation.id).await;

    let attempts = (0..4).map(|_| {
        let (pool, token) = (t.pool.clone(), token.clone());
        tokio::spawn(async move { accept(&pool, &token, Acceptor::NewUser { password: PASSWORD.into() }).await })
    });
    let results: Vec<_> = join_all(attempts).await;

    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(results.iter().filter(|r| r.is_err()).all(|r| matches!(r, Err(StaffError::Gone))), "{results:?}");
    let members: i64 = sqlx::query_scalar("select count(*) from membership").fetch_one(&t.superuser).await.unwrap();
    assert_eq!(members, 2);
}

async fn join_all<T>(handles: impl Iterator<Item = tokio::task::JoinHandle<T>>) -> Vec<T> {
    let mut results = Vec::new();
    for handle in handles.collect::<Vec<_>>() {
        results.push(handle.await.unwrap());
    }
    results
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_existing_user_must_be_signed_in_as_the_invited_email(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let (kamala, _) = signup(&t.pool, signup_input("kamala@example.com", "Her Own Hotel")).await.unwrap();
    let (stranger, _) = signup(&t.pool, signup_input("stranger@example.com", "Elsewhere")).await.unwrap();
    let invitation = t.invite("kamala@example.com", &[grant(Role::FrontDesk, Some(t.hotel))]).await;
    let token = t.token(invitation.id).await;

    assert!(preview(&t.pool, &token).await.unwrap().existing_account);
    match t.accept_new(&token).await {
        Err(StaffError::Conflict(message)) => assert_eq!(message, "Sign in as kamala@example.com to accept"),
        other => panic!("expected a conflict, got {other:?}"),
    }
    assert!(matches!(accept(&t.pool, &token, Acceptor::SignedIn(stranger)).await, Err(StaffError::Forbidden(_))));

    assert_eq!(accept(&t.pool, &token, Acceptor::SignedIn(kamala)).await.unwrap(), (kamala, t.tenant));
    assert_eq!(t.grants_of(kamala).await, vec![grant(Role::FrontDesk, Some(t.hotel))]);
    // Their password is unchanged.
    assert_eq!(authenticate(&t.pool, "kamala@example.com", PASSWORD).await.unwrap(), Some(kamala));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_new_users_password_follows_the_signup_rules(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let invitation = t.invite("kamala@example.com", &[grant(Role::FrontDesk, None)]).await;
    let token = t.token(invitation.id).await;

    let short = accept(&t.pool, &token, Acceptor::NewUser { password: "too short".into() }).await;

    assert!(matches!(short, Err(StaffError::Invalid(_))));
    assert!(t.accept_new(&token).await.is_ok());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn managers_give_only_junior_roles_at_their_properties(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let manager = t.member("manager@example.com", &[grant(Role::Manager, Some(t.hotel))]).await;
    let refused = [
        ("a@example.com", grant(Role::Manager, Some(t.hotel)), "only owners can grant the manager role".to_string()),
        (
            "b@example.com",
            grant(Role::FrontDesk, Some(t.other_hotel)),
            format!("you do not manage property {}", t.other_hotel),
        ),
        (
            "c@example.com",
            grant(Role::FrontDesk, None),
            "a tenant-wide front_desk grant needs a tenant-wide manager".into(),
        ),
    ];

    for (email, given, reason) in &refused {
        match t.invite_as(manager, email, &[*given]).await {
            Err(StaffError::Forbidden(message)) => assert_eq!(&message, reason),
            other => panic!("expected {reason}, got {other:?}"),
        }
    }
    assert!(t.invite_as(manager, "d@example.com", &[grant(Role::FrontDesk, Some(t.hotel))]).await.is_ok());
    for (email, given, _) in &refused {
        assert!(t.invite_as(t.owner, email, &[*given]).await.is_ok(), "{email}");
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_manager_cannot_resend_or_revoke_an_invitation_naming_manager(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let manager = t.member("manager@example.com", &[grant(Role::Manager, None)]).await;
    let invitation = t.invite("other-manager@example.com", &[grant(Role::Manager, Some(t.hotel))]).await;

    let mut tx = begin(&t.pool, Scope { tenant: Some(t.tenant), user: Some(manager) }).await.unwrap();
    let grants = load_grants(&mut tx, manager).await.unwrap();
    let resent = resend(&mut tx, t.tenant, manager, &grants, invitation.id).await;
    let revoked = revoke_invitation(&mut tx, t.tenant, manager, &grants, invitation.id).await;

    assert!(matches!(resent, Err(StaffError::Forbidden(_))));
    assert!(matches!(revoked, Err(StaffError::Forbidden(_))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn invitations_are_validated(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let (_, stranger_tenant) = signup(&t.pool, signup_input("stranger@example.com", "Elsewhere")).await.unwrap();
    let stranger_property = Uuid::now_v7();
    sqlx::query(
        "insert into property (id, tenant_id, code, name, timezone, base_currency, business_date)
         values ($1, $2, 'ELS', 'Elsewhere', 'Asia/Colombo', 'LKR', current_date)",
    )
    .bind(stranger_property)
    .bind(stranger_tenant.0)
    .execute(&t.superuser)
    .await
    .unwrap();
    let front_desk = [grant(Role::FrontDesk, None)];

    let invalid = [
        t.invite_as(t.owner, "not-an-email", &front_desk).await,
        t.invite_as(t.owner, "a@example.com", &[]).await,
        t.invite_as(t.owner, "a@example.com", &[grant(Role::Owner, None)]).await,
        t.invite_as(t.owner, "a@example.com", &[grant(Role::Pos, Some(stranger_property))]).await,
        t.invite_as(t.owner, "a@example.com", &[front_desk[0], front_desk[0]]).await,
        t.invite_as(t.owner, "a@example.com", &[grant(Role::Pos, None); 51]).await,
    ];
    for result in invalid {
        assert!(matches!(result, Err(StaffError::Invalid(_))), "{result:?}");
    }
    assert!(matches!(t.invite_as(t.owner, "OWNER@example.com", &front_desk).await, Err(StaffError::Conflict(_))));
    t.invite("a@example.com", &front_desk).await;
    assert!(matches!(t.invite_as(t.owner, "A@example.com", &front_desk).await, Err(StaffError::Conflict(_))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_outbox_row_is_written_in_the_invitations_transaction(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let mut tx = begin(&t.pool, Scope { tenant: Some(t.tenant), user: Some(t.owner) }).await.unwrap();
    let grants = load_grants(&mut tx, t.owner).await.unwrap();
    invite(&mut tx, t.tenant, t.owner, &grants, "kamala@example.com", "Kamala", &[grant(Role::Pos, None)])
        .await
        .unwrap();
    tx.rollback().await.unwrap();

    assert!(t.mail_jobs().await.is_empty());
    assert!(t.list().await.is_empty());

    let invitation = t.invite("kamala@example.com", &[grant(Role::Pos, None)]).await;
    let jobs = t.mail_jobs().await;
    assert_eq!(jobs.len(), 1);
    let MailJob::Invitation { invitation_id, tenant_id, .. } = &jobs[0];
    assert_eq!((*invitation_id, *tenant_id), (invitation.id, t.tenant.0));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn no_audit_row_or_stored_column_contains_a_token(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let first = t.invite("kamala@example.com", &[grant(Role::Pos, None)]).await;
    t.resend(first.id).await;
    let accepted = t.token(first.id).await;
    t.accept_new(&accepted).await.unwrap();
    let second = t.invite("sunil@example.com", &[grant(Role::Pos, None)]).await;
    t.revoke(second.id).await;
    let tokens: Vec<String> = t.mail_jobs().await.into_iter().map(|MailJob::Invitation { token, .. }| token).collect();

    let audit: Vec<(String, String)> = sqlx::query_as("select action, data::text from audit_log order by at, id")
        .fetch_all(&t.superuser)
        .await
        .unwrap();
    let invitations: Vec<String> =
        sqlx::query_scalar("select row_to_json(i)::text from invitation i").fetch_all(&t.superuser).await.unwrap();

    let actions: Vec<&str> = audit.iter().map(|(action, _)| action.as_str()).collect();
    for expected in ["invitation.created", "invitation.resent", "invitation.accepted", "invitation.revoked"] {
        assert!(actions.contains(&expected), "{actions:?}");
    }
    assert_eq!(tokens.len(), 3);
    for token in &tokens {
        assert!(audit.iter().all(|(_, data)| !data.contains(token.as_str())));
        assert!(invitations.iter().all(|row| !row.contains(token.as_str())));
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_user_with_no_memberships_can_sign_in(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let removed = t.member("kamala@example.com", &[grant(Role::FrontDesk, None)]).await;
    sqlx::query("delete from membership where user_id = $1").bind(removed.0).execute(&t.superuser).await.unwrap();

    let user = authenticate(&t.pool, "kamala@example.com", PASSWORD).await.unwrap();
    let tenant = default_tenant(&t.pool, removed).await.unwrap();
    let session = create_session(&t.pool, removed, tenant).await;
    let profile = load_profile(&t.pool, removed, tenant).await.unwrap();

    assert_eq!(user, Some(removed));
    assert_eq!(tenant, None);
    assert!(session.is_ok());
    assert!(profile.tenants.is_empty());
    assert!(profile.grants.is_empty());
    assert_eq!(profile.current_tenant, None);
}
