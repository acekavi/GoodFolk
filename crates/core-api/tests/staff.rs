use core_api::staff::{AddStaff, AddStaffError, Outcome, add_staff, parse_args};
use db::testing::app_pool;
use identity::{Role, SignupInput, authenticate, signup};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;
use uuid::Uuid;

const PASSWORD: &str = "a staff password nobody logs";

fn owner_input(email: &str, tenant: &str) -> SignupInput {
    SignupInput {
        email: email.into(),
        password: "a long enough password".into(),
        display_name: "Owner".into(),
        tenant_name: tenant.into(),
    }
}

fn staff(role: Role) -> AddStaff {
    AddStaff {
        owner_email: "owner@example.com".into(),
        tenant: None,
        email: "Kamala@example.com".into(),
        password: PASSWORD.into(),
        display_name: "Kamala".into(),
        role,
        property: None,
    }
}

async fn add_property(superuser: &PgPool, tenant: Uuid, code: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("insert into property (id, tenant_id, code, name, timezone, base_currency, business_date) values ($1, $2, $3, $3, 'Asia/Colombo', 'LKR', current_date)")
        .bind(id)
        .bind(tenant)
        .bind(code)
        .execute(superuser)
        .await
        .unwrap();
    id
}

async fn counts(superuser: &PgPool) -> (i64, i64, i64) {
    sqlx::query_as(
        "select (select count(*) from app_user), (select count(*) from membership), (select count(*) from role_grant)",
    )
    .fetch_one(superuser)
    .await
    .unwrap()
}

/// A file holding `contents`, removed when dropped.
struct TempFile(std::path::PathBuf);

impl TempFile {
    fn new(contents: &str) -> Self {
        let path = std::env::temp_dir().join(format!("goodfolk-add-staff-{}", Uuid::now_v7()));
        std::fs::write(&path, contents).unwrap();
        TempFile(path)
    }

    fn path(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|a| (*a).to_owned()).collect()
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

/// Installs, once, a global subscriber that records every test's logs in this binary. A per-test
/// subscriber is unreliable here: tests run in parallel and tasks move between threads.
fn captured_logs() -> &'static CapturedLogs {
    static LOGS: OnceLock<CapturedLogs> = OnceLock::new();
    LOGS.get_or_init(|| {
        let logs = CapturedLogs::default();
        tracing::subscriber::set_global_default(tracing_subscriber::fmt().with_writer(logs.clone()).finish())
            .expect("no other global subscriber in this test binary");
        logs
    })
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn it_creates_the_user_the_membership_and_the_grant(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts.clone(), 1).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let (_, tenant) = signup(&pool, owner_input("owner@example.com", "Lagoon")).await.unwrap();
    let before = counts(&superuser).await;

    let outcome = add_staff(&pool, staff(Role::FrontDesk)).await.unwrap();

    assert_eq!(
        outcome,
        Outcome { tenant: "Lagoon".into(), user_created: true, membership_created: true, grant_created: true }
    );
    assert_eq!(counts(&superuser).await, (before.0 + 1, before.1 + 1, before.2 + 1));
    let (role, property): (String, Option<Uuid>) = sqlx::query_as(
        "select g.role, g.property_id from role_grant g join app_user u on u.id = g.user_id
         where u.email = 'kamala@example.com' and g.tenant_id = $1",
    )
    .bind(tenant.0)
    .fetch_one(&superuser)
    .await
    .unwrap();
    assert_eq!((role.as_str(), property), ("front_desk", None));
    // Hashed as sign-up does: the password signs in.
    assert!(authenticate(&pool, "kamala@example.com", PASSWORD).await.unwrap().is_some());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_rerun_adds_nothing_and_says_what_exists(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts.clone(), 1).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let (_, tenant) = signup(&pool, owner_input("owner@example.com", "Lagoon")).await.unwrap();
    let property = add_property(&superuser, tenant.0, "COL").await;
    let scoped = || AddStaff { property: Some("COL".into()), ..staff(Role::Housekeeping) };
    add_staff(&pool, scoped()).await.unwrap();
    let before = counts(&superuser).await;

    let outcome = add_staff(&pool, scoped()).await.unwrap();

    assert_eq!(
        outcome,
        Outcome { tenant: "Lagoon".into(), user_created: false, membership_created: false, grant_created: false }
    );
    assert_eq!(counts(&superuser).await, before);
    let audited: i64 = sqlx::query_scalar("select count(*) from audit_log where action = 'role.granted'")
        .fetch_one(&superuser)
        .await
        .unwrap();
    assert_eq!(audited, 1);
    // The grant is scoped to the property.
    let granted: Option<Uuid> = sqlx::query_scalar("select property_id from role_grant where role = 'housekeeping'")
        .fetch_one(&superuser)
        .await
        .unwrap();
    assert_eq!(granted, Some(property));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_second_role_adds_only_the_grant(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    signup(&pool, owner_input("owner@example.com", "Lagoon")).await.unwrap();
    add_staff(&pool, staff(Role::FrontDesk)).await.unwrap();

    let outcome = add_staff(&pool, staff(Role::Accountant)).await.unwrap();

    assert_eq!(
        outcome,
        Outcome { tenant: "Lagoon".into(), user_created: false, membership_created: false, grant_created: true }
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_unknown_property_is_not_found_and_adds_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts.clone(), 1).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    signup(&pool, owner_input("owner@example.com", "Lagoon")).await.unwrap();
    let before = counts(&superuser).await;

    let refused = add_staff(&pool, AddStaff { property: Some("NOPE".into()), ..staff(Role::Manager) }).await;

    assert!(matches!(refused, Err(AddStaffError::PropertyNotFound(code)) if code == "NOPE"));
    assert_eq!(counts(&superuser).await, before);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_non_owner_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts.clone(), 1).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    signup(&pool, owner_input("owner@example.com", "Lagoon")).await.unwrap();
    add_staff(&pool, staff(Role::Manager)).await.unwrap();
    let before = counts(&superuser).await;

    let as_manager = AddStaff {
        owner_email: "kamala@example.com".into(),
        email: "other@example.com".into(),
        ..staff(Role::Housekeeping)
    };
    let as_stranger = AddStaff {
        owner_email: "nobody@example.com".into(),
        email: "other@example.com".into(),
        ..staff(Role::Housekeeping)
    };

    assert!(matches!(add_staff(&pool, as_manager).await, Err(AddStaffError::NotOwner(_))));
    assert!(matches!(add_staff(&pool, as_stranger).await, Err(AddStaffError::NotOwner(_))));
    assert_eq!(counts(&superuser).await, before);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_owner_of_two_tenants_must_name_one(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts.clone(), 1).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let (owner, _) = signup(&pool, owner_input("owner@example.com", "Lagoon")).await.unwrap();
    // A second tenant owned by the same user.
    let second = Uuid::now_v7();
    sqlx::query("insert into tenant (id, name) values ($1, 'Hills')").bind(second).execute(&superuser).await.unwrap();
    sqlx::query("insert into membership (tenant_id, user_id) values ($1, $2)")
        .bind(second)
        .bind(owner.0)
        .execute(&superuser)
        .await
        .unwrap();
    sqlx::query("insert into role_grant (id, tenant_id, user_id, role) values ($1, $2, $3, 'owner')")
        .bind(Uuid::now_v7())
        .bind(second)
        .bind(owner.0)
        .execute(&superuser)
        .await
        .unwrap();
    let before = counts(&superuser).await;

    let refused = add_staff(&pool, staff(Role::Manager)).await.unwrap_err();
    assert!(matches!(refused, AddStaffError::AmbiguousTenant(..)));
    assert!(refused.to_string().contains("--tenant") && refused.to_string().contains("`Hills`"), "{refused}");
    assert_eq!(counts(&superuser).await, before);

    let named = add_staff(&pool, AddStaff { tenant: Some("Hills".into()), ..staff(Role::Manager) }).await.unwrap();
    assert_eq!(named.tenant, "Hills");
    let unowned = add_staff(&pool, AddStaff { tenant: Some("Nowhere".into()), ..staff(Role::Manager) }).await;
    assert!(matches!(unowned, Err(AddStaffError::NotOwner(_))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_password_never_appears_in_the_logs_or_errors(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    signup(&pool, owner_input("owner@example.com", "Lagoon")).await.unwrap();
    let logs = captured_logs();

    let outcome = add_staff(&pool, staff(Role::Manager)).await.unwrap();
    let refused = add_staff(&pool, AddStaff { owner_email: "nobody@example.com".into(), ..staff(Role::Manager) }).await;

    let logged = logs.text();
    assert!(logged.contains("staff added"), "logged: {logged}");
    for text in [logged, outcome.to_string(), refused.unwrap_err().to_string()] {
        assert!(!text.contains(PASSWORD), "leaked: {text}");
    }
}

#[test]
fn the_password_is_read_from_a_file_without_its_line_break() {
    let file = TempFile::new(&format!("{PASSWORD}\n"));

    let input = parse_args(&args(&[
        "--owner-email",
        "owner@example.com",
        "--email",
        "kamala@example.com",
        "--password-file",
        &file.path(),
        "--display-name",
        "Kamala",
        "--role",
        "hk_supervisor",
        "--property",
        "COL",
    ]))
    .unwrap();

    assert_eq!(input.password, PASSWORD);
    assert_eq!(input.role, Role::HkSupervisor);
    assert_eq!(input.property.as_deref(), Some("COL"));
}

#[test]
fn an_unknown_role_is_refused() {
    let file = TempFile::new(PASSWORD);

    let refused = parse_args(&args(&[
        "--owner-email",
        "owner@example.com",
        "--email",
        "kamala@example.com",
        "--password-file",
        &file.path(),
        "--display-name",
        "Kamala",
        "--role",
        "root",
    ]));

    assert!(matches!(refused, Err(AddStaffError::Invalid(message)) if message.contains("unknown role `root`")));
}

#[test]
fn a_password_on_the_command_line_is_refused_and_not_echoed() {
    let file = TempFile::new(PASSWORD);

    let refused = parse_args(&args(&[
        "--owner-email",
        "owner@example.com",
        "--email",
        "kamala@example.com",
        "--password-file",
        &file.path(),
        "--display-name",
        "Kamala",
        "--role",
        "manager",
        &format!("--password={PASSWORD}"),
    ]))
    .err()
    .unwrap();

    assert!(refused.to_string().contains("unknown option `--password`"), "{refused}");
    assert!(!refused.to_string().contains(PASSWORD));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_short_password_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    signup(&pool, owner_input("owner@example.com", "Lagoon")).await.unwrap();

    let refused = add_staff(&pool, AddStaff { password: "short".into(), ..staff(Role::Manager) }).await;

    assert!(matches!(refused, Err(AddStaffError::Invalid(_))));
}
