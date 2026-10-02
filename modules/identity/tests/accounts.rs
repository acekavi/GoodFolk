use db::testing::app_pool;
use db::{Scope, begin};
use identity::{
    Grant, Permission, Role, SignupError, SignupInput, TenantAccess, allows, authenticate, create_session,
    default_tenant, delete_session, load_grants, load_profile, resolve_session, resolve_tenant_access, signup,
    switch_tenant,
};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;
use uuid::Uuid;

fn input(email: &str, tenant: &str) -> SignupInput {
    SignupInput {
        email: email.into(),
        password: "a long enough password".into(),
        display_name: "Nimal".into(),
        tenant_name: tenant.into(),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn signup_creates_an_owner_of_a_new_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;

    let (user, tenant) = signup(&pool, input("owner@example.com", "Lagoon Hotels")).await.unwrap();
    let profile = load_profile(&pool, user, Some(tenant)).await.unwrap();

    assert_eq!(profile.email, "owner@example.com");
    assert_eq!(profile.tenants.len(), 1);
    assert_eq!(profile.tenants[0].name, "Lagoon Hotels");
    assert!(allows(&profile.grants, Permission::PropertiesCreate, None));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn emails_are_unique_ignoring_case(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    signup(&pool, input("owner@example.com", "A")).await.unwrap();

    let second = signup(&pool, input("OWNER@example.com", "B")).await;

    assert!(matches!(second, Err(SignupError::EmailTaken)));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn authenticate_accepts_only_the_right_password(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (user, _) = signup(&pool, input("owner@example.com", "A")).await.unwrap();

    assert_eq!(authenticate(&pool, "Owner@Example.com", "a long enough password").await.unwrap(), Some(user));
    assert_eq!(authenticate(&pool, "owner@example.com", "wrong password").await.unwrap(), None);
    assert_eq!(authenticate(&pool, "nobody@example.com", "a long enough password").await.unwrap(), None);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_session_resolves_until_deleted(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (user, tenant) = signup(&pool, input("owner@example.com", "A")).await.unwrap();
    assert_eq!(default_tenant(&pool, user).await.unwrap(), Some(tenant));

    let (token, _) = create_session(&pool, user, Some(tenant)).await.unwrap();
    let session = resolve_session(&pool, &token).await.unwrap().unwrap();
    delete_session(&pool, &token).await.unwrap();

    assert_eq!(session.user, user);
    assert_eq!(session.tenant, Some(tenant));
    assert_eq!(resolve_session(&pool, &token).await.unwrap(), None);
    assert_eq!(resolve_session(&pool, "made-up-token").await.unwrap(), None);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_session_can_switch_only_to_tenants_the_user_belongs_to(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (user, tenant) = signup(&pool, input("owner@example.com", "A")).await.unwrap();
    let (_, stranger_tenant) = signup(&pool, input("other@example.com", "B")).await.unwrap();
    let (token, _) = create_session(&pool, user, None).await.unwrap();
    let session = resolve_session(&pool, &token).await.unwrap().unwrap();

    assert!(!switch_tenant(&pool, &session, stranger_tenant).await.unwrap());
    assert!(switch_tenant(&pool, &session, tenant).await.unwrap());
    assert_eq!(resolve_session(&pool, &token).await.unwrap().unwrap().tenant, Some(tenant));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn tenant_access_is_the_session_membership_and_grants_in_one_query(_: PgPoolOptions, opts: PgConnectOptions) {
    let superuser = PgPool::connect_with(opts.clone()).await.unwrap();
    let pool = app_pool(opts, 1).await;
    let (user, tenant) = signup(&pool, input("owner@example.com", "A")).await.unwrap();
    let (token, _) = create_session(&pool, user, Some(tenant)).await.unwrap();
    let session = resolve_session(&pool, &token).await.unwrap().unwrap();
    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    let grants = load_grants(&mut tx, user).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(resolve_tenant_access(&pool, &token).await.unwrap(), TenantAccess::Member { session, tenant, grants });

    // The settings it makes end with the query: the one pooled connection carries none to its next user.
    let leaked: Option<String> =
        sqlx::query_scalar("select current_setting('app.tenant_id', true)").fetch_one(&pool).await.unwrap();
    assert!(leaked.is_none_or(|value| value.is_empty()));

    sqlx::query("delete from membership").execute(&superuser).await.unwrap();
    assert_eq!(resolve_tenant_access(&pool, &token).await.unwrap(), TenantAccess::NotMember);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn tenant_access_refuses_unknown_expired_and_tenantless_sessions(_: PgPoolOptions, opts: PgConnectOptions) {
    let superuser = PgPool::connect_with(opts.clone()).await.unwrap();
    let pool = app_pool(opts, 1).await;
    let (user, tenant) = signup(&pool, input("owner@example.com", "A")).await.unwrap();
    let (token, _) = create_session(&pool, user, Some(tenant)).await.unwrap();
    let (tenantless, _) = create_session(&pool, user, None).await.unwrap();

    assert_eq!(resolve_tenant_access(&pool, "made-up-token").await.unwrap(), TenantAccess::NoSession);
    assert_eq!(resolve_tenant_access(&pool, &tenantless).await.unwrap(), TenantAccess::NoTenant);
    sqlx::query("update session set expires_at = now() - interval '1 second'").execute(&superuser).await.unwrap();
    assert_eq!(resolve_tenant_access(&pool, &token).await.unwrap(), TenantAccess::NoSession);
}

/// Collects log output so a test can check what was logged.
#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl CapturedLogs {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl std::io::Write for CapturedLogs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for CapturedLogs {
    type Writer = CapturedLogs;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_grant_with_an_unknown_role_is_skipped_with_a_warning(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts.clone(), 1).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let (user, tenant) = signup(&pool, input("owner@example.com", "A")).await.unwrap();
    // A role written by a newer release (or by hand) that this build does not know.
    sqlx::query("alter table role_grant drop constraint role_grant_role_check").execute(&superuser).await.unwrap();
    sqlx::query("insert into role_grant (id, tenant_id, user_id, role) values ($1, $2, $3, 'night_auditor')")
        .bind(Uuid::now_v7())
        .bind(tenant.0)
        .bind(user.0)
        .execute(&superuser)
        .await
        .unwrap();
    let logs = CapturedLogs::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::fmt().with_writer(logs.clone()).finish());

    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    let grants = load_grants(&mut tx, user).await.unwrap();

    assert_eq!(grants, vec![Grant { property_id: None, role: Role::Owner }]);
    let logged = logs.text();
    assert!(logged.contains("WARN") && logged.contains("night_auditor"), "logged: {logged}");
}
