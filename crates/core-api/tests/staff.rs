//! Staff: the interim `add-staff` command, and invitations, members and revocation over HTTP.

mod common;

use axum::http::{Method, StatusCode, header};
use common::{TestApp, TestResponse, session_cookie};
use core_api::staff::{AddStaff, AddStaffError, Outcome, add_staff, parse_args};
use db::testing::app_pool;
use identity::{Role, SignupInput, authenticate, signup};
use mail::MailJob;
use serde_json::{Value, json};
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

/// A tenant with one property (Galle) and its owner, over HTTP.
struct Tenant {
    app: TestApp,
    superuser: PgPool,
    owner: String,
    tenant: Uuid,
    property: Uuid,
}

impl Tenant {
    async fn new(opts: PgConnectOptions) -> Self {
        let app = TestApp::new(opts.clone()).await;
        let superuser = PgPool::connect_with(opts).await.unwrap();
        let owner = app.signup_owner("owner@example.com", "Lagoon").await;
        let property = json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"});
        let created = app
            .send_with(
                Method::POST,
                "/api/v1/properties",
                Some(&owner),
                Some(property),
                &[("x-goodfolk-csrf", "1"), ("idempotency-key", "property-0001")],
            )
            .await;
        assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
        let me = app.send(Method::GET, "/api/v1/me", Some(&owner), None).await;
        let tenant = common::uuid(&me.body["current_tenant"]);
        Self { app, superuser, owner, tenant, property: common::uuid(&created.body["id"]) }
    }

    async fn invite_as(&self, cookie: &str, email: &str, grants: Value) -> TestResponse {
        let body = json!({"email": email, "display_name": "Kamala", "grants": grants});
        let key = Uuid::now_v7().to_string();
        self.app
            .send_with(
                Method::POST,
                "/api/v1/invitations",
                Some(cookie),
                Some(body),
                &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)],
            )
            .await
    }

    /// Invites `email` as the owner, as a front-desk clerk at Galle, and returns the invitation's id.
    async fn invite(&self, email: &str) -> Uuid {
        let invited =
            self.invite_as(&self.owner, email, json!([{"role": "front_desk", "property_id": self.property}])).await;
        assert_eq!(invited.status, StatusCode::CREATED, "{:?}", invited.body);
        common::uuid(&invited.body["id"])
    }

    /// The newest token mailed for `invitation`, as the email's link carries it: only the outbox has it.
    async fn token(&self, invitation: Uuid) -> String {
        let payloads: Vec<Value> =
            sqlx::query_scalar("select payload from outbox where topic = $1 order by created_at desc, id desc")
                .bind(mail::SEND_TOPIC)
                .fetch_all(&self.superuser)
                .await
                .unwrap();
        payloads
            .into_iter()
            .map(|payload| serde_json::from_value::<MailJob>(payload).unwrap())
            .find_map(|MailJob::Invitation { invitation_id, token, .. }| (invitation_id == invitation).then_some(token))
            .expect("the invitation was mailed")
    }

    async fn preview(&self, token: &str) -> TestResponse {
        self.app.send(Method::POST, "/api/v1/invitations/preview", None, Some(json!({"token": token}))).await
    }

    async fn accept(&self, cookie: Option<&str>, token: &str, password: Option<&str>) -> TestResponse {
        let body = json!({"token": token, "password": password});
        self.app.send(Method::POST, "/api/v1/invitations/accept", cookie, Some(body)).await
    }

    /// Invites `email` and accepts as a new user; returns their session cookie and user id.
    async fn member(&self, email: &str) -> (String, Uuid) {
        let token = self.token(self.invite(email).await).await;
        let accepted = self.accept(None, &token, Some(PASSWORD)).await;
        assert_eq!(accepted.status, StatusCode::OK, "{:?}", accepted.body);
        (session_cookie(&accepted.headers), common::uuid(&accepted.body["user_id"]))
    }

    async fn graphql(&self, cookie: &str, query: &str) -> Value {
        let response = self.app.send(Method::POST, "/graphql", Some(cookie), Some(json!({"query": query}))).await;
        assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
        response.body
    }

    async fn set_grants(&self, cookie: &str, user: Uuid, version: Option<&str>, grants: Value) -> TestResponse {
        let mut headers = vec![("x-goodfolk-csrf", "1")];
        if let Some(version) = version {
            headers.push(("if-match", version));
        }
        let path = format!("/api/v1/members/{user}/grants");
        self.app.send_with(Method::PUT, &path, Some(cookie), Some(json!({"grants": grants})), &headers).await
    }
}

const GONE: &str = "This invitation is no longer valid";

fn is_gone(response: &TestResponse) -> bool {
    response.status == StatusCode::GONE
        && response.body == json!({"type": "about:blank", "title": "Gone", "status": 410, "detail": GONE})
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_invited_clerk_sets_a_password_and_is_signed_in_with_the_grants(_: PgPoolOptions, opts: PgConnectOptions) {
    let logs = captured_logs();
    let t = Tenant::new(opts).await;
    let grants = json!([{"role": "front_desk", "property_id": t.property}]);
    let body = json!({"email": "Kamala@Example.com", "display_name": "Kamala", "grants": grants});
    let headers = [("x-goodfolk-csrf", "1"), ("idempotency-key", "invite-0001")];
    let path = "/api/v1/invitations";

    let invited = t.app.send_with(Method::POST, path, Some(&t.owner), Some(body.clone()), &headers).await;
    let replayed = t.app.send_with(Method::POST, path, Some(&t.owner), Some(body), &headers).await;
    let token = t.token(common::uuid(&invited.body["id"])).await;
    let preview = t.preview(&token).await;
    let accepted = t.accept(None, &token, Some(PASSWORD)).await;
    let cookie = session_cookie(&accepted.headers);
    let me = t.app.send(Method::GET, "/api/v1/me", Some(&cookie), None).await;
    let again = t.accept(None, &token, Some(PASSWORD)).await;
    let previewed_again = t.preview(&token).await;
    let unknown = t.accept(None, "not-a-token", Some(PASSWORD)).await;
    let listed = t.graphql(&t.owner, "{ invitations { email status grants { role propertyId } } }").await;

    assert_eq!(invited.status, StatusCode::CREATED, "{:?}", invited.body);
    assert_eq!(
        (invited.body["email"].as_str(), invited.body["status"].as_str()),
        (Some("kamala@example.com"), Some("pending"))
    );
    assert_eq!(replayed.body, invited.body, "a retry replays the invitation and sends nothing more");
    let mailed: i64 = sqlx::query_scalar("select count(*) from outbox where topic = $1")
        .bind(mail::SEND_TOPIC)
        .fetch_one(&t.superuser)
        .await
        .unwrap();
    assert_eq!(mailed, 1);
    assert_eq!(preview.status, StatusCode::OK, "{:?}", preview.body);
    assert_eq!(
        preview.body,
        json!({"tenant_name": "Lagoon", "inviter": "Owner", "email": "kamala@example.com",
               "grants": [{"role": "front_desk", "property_name": "Galle"}], "existing_account": false})
    );
    assert_eq!(accepted.status, StatusCode::OK, "{:?}", accepted.body);
    assert_eq!(me.status, StatusCode::OK, "{:?}", me.body);
    assert_eq!(me.body["email"], "kamala@example.com");
    assert_eq!(me.body["current_tenant"], json!(t.tenant));
    assert_eq!(me.body["grants"], json!([{"property_id": t.property, "role": "front_desk"}]));
    assert!(is_gone(&again), "a token works once: {} {:?}", again.status, again.body);
    assert!(is_gone(&previewed_again), "{} {:?}", previewed_again.status, previewed_again.body);
    assert!(is_gone(&unknown), "an unknown token gets the same answer: {} {:?}", unknown.status, unknown.body);
    assert_eq!(listed["data"]["invitations"][0]["status"], "ACCEPTED", "{listed:?}");

    // The token is never in a response, and never in the logs.
    for response in [&invited, &replayed, &preview, &accepted, &me, &again] {
        assert!(!response.body.to_string().contains(&token), "{:?}", response.body);
        assert!(!format!("{:?}", response.headers).contains(&token));
    }
    assert!(!listed.to_string().contains(&token));
    assert!(!logs.text().contains(&token), "the token was logged");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_existing_user_accepts_while_signed_in_and_their_session_switches(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let kamala = t.app.signup_owner("kamala@example.com", "Kamala's own").await;
    let token = t.token(t.invite("kamala@example.com").await).await;

    let preview = t.preview(&token).await;
    let signed_out = t.accept(None, &token, Some(PASSWORD)).await;
    let someone_else = t.app.signup_owner("other@example.com", "Other").await;
    let wrong_user = t.accept(Some(&someone_else), &token, None).await;
    let accepted = t.accept(Some(&kamala), &token, None).await;
    let me = t.app.send(Method::GET, "/api/v1/me", Some(&kamala), None).await;

    assert_eq!(preview.body["existing_account"], true, "{:?}", preview.body);
    assert_eq!(signed_out.status, StatusCode::CONFLICT, "{:?}", signed_out.body);
    assert_eq!(signed_out.body["detail"], "Sign in as kamala@example.com to accept");
    assert_eq!(wrong_user.status, StatusCode::FORBIDDEN, "{:?}", wrong_user.body);
    assert_eq!(accepted.status, StatusCode::OK, "{:?}", accepted.body);
    assert!(accepted.headers.get(header::SET_COOKIE).is_none(), "the session is kept, not replaced");
    assert_eq!(me.body["current_tenant"], json!(t.tenant), "{:?}", me.body);
    assert_eq!(me.body["tenants"].as_array().unwrap().len(), 2);
    // The password was not changed.
    let pool = &t.app.pool;
    assert!(authenticate(pool, "kamala@example.com", "a long enough password").await.unwrap().is_some());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn resent_and_revoked_invitations_refuse_their_old_links(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let invitation = t.invite("kamala@example.com").await;
    let first = t.token(invitation).await;

    let resent =
        t.app.send(Method::POST, &format!("/api/v1/invitations/{invitation}/resend"), Some(&t.owner), None).await;
    let second = t.token(invitation).await;
    let old_link = t.preview(&first).await;
    let new_link = t.preview(&second).await;
    let revoked =
        t.app.send(Method::POST, &format!("/api/v1/invitations/{invitation}/revoke"), Some(&t.owner), None).await;
    let after_revoke = t.accept(None, &second, Some(PASSWORD)).await;
    let unknown =
        t.app.send(Method::POST, &format!("/api/v1/invitations/{}/revoke", Uuid::now_v7()), Some(&t.owner), None).await;

    assert_eq!(resent.status, StatusCode::OK, "{:?}", resent.body);
    assert_ne!(first, second);
    assert!(is_gone(&old_link), "{} {:?}", old_link.status, old_link.body);
    assert_eq!(new_link.status, StatusCode::OK, "{:?}", new_link.body);
    assert_eq!((revoked.status, revoked.body["status"].as_str()), (StatusCode::OK, Some("revoked")));
    assert!(is_gone(&after_revoke), "{} {:?}", after_revoke.status, after_revoke.body);
    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{:?}", unknown.body);
    for response in [&resent, &revoked] {
        assert!(!response.body.to_string().contains(&first) && !response.body.to_string().contains(&second));
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn grants_change_with_if_match_and_a_revoked_member_is_signed_out(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let (cookie, user) = t.member("kamala@example.com").await;
    let members = t.graphql(&t.owner, "{ members { userId email version grants { role propertyId } } }").await;
    let version =
        members["data"]["members"].as_array().unwrap().iter().find(|m| m["userId"] == json!(user)).unwrap()["version"]
            .as_i64()
            .unwrap();
    let both = json!([{"role": "front_desk", "property_id": t.property}, {"role": "accountant", "property_id": null}]);

    let missing = t.set_grants(&t.owner, user, None, both.clone()).await;
    let changed = t.set_grants(&t.owner, user, Some(&format!("\"{version}\"")), both.clone()).await;
    let stale = t.set_grants(&t.owner, user, Some(&format!("\"{version}\"")), both).await;
    let me = t.app.send(Method::GET, "/api/v1/me", Some(&cookie), None).await;
    let owner_id = t.app.send(Method::GET, "/api/v1/me", Some(&t.owner), None).await.body["user_id"].clone();
    let themself = t
        .app
        .send(Method::DELETE, &format!("/api/v1/members/{}", owner_id.as_str().unwrap()), Some(&t.owner), None)
        .await;
    let removed = t.app.send(Method::DELETE, &format!("/api/v1/members/{user}"), Some(&t.owner), None).await;
    let after = t.app.send(Method::GET, "/api/v1/me", Some(&cookie), None).await;

    assert_eq!(missing.status, StatusCode::PRECONDITION_REQUIRED, "{:?}", missing.body);
    assert_eq!(changed.status, StatusCode::OK, "{:?}", changed.body);
    assert_eq!(changed.headers[header::ETAG], format!("\"{}\"", version + 1));
    assert_eq!(changed.body["grants"].as_array().unwrap().len(), 2);
    assert_eq!(stale.status, StatusCode::PRECONDITION_FAILED, "{:?}", stale.body);
    assert_eq!(me.body["grants"].as_array().unwrap().len(), 2, "grants are re-read on the next request");
    assert_eq!(themself.status, StatusCode::FORBIDDEN, "an owner cannot remove themself: {:?}", themself.body);
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{:?}", removed.body);
    assert_eq!(after.status, StatusCode::UNAUTHORIZED, "the revoked member's old cookie: {:?}", after.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn managers_act_only_on_junior_staff_and_front_desk_on_nobody(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;
    let manager = t.app.staff(&t.superuser, &t.owner, "manager@example.com", "manager").await;
    let desk = t.app.staff(&t.superuser, &t.owner, "desk@example.com", "front_desk").await;
    let (_, clerk) = t.member("kamala@example.com").await;
    let owner_invitation =
        t.invite_as(&t.owner, "boss@example.com", json!([{"role": "manager", "property_id": null}])).await;
    let owner_invitation = common::uuid(&owner_invitation.body["id"]);

    let as_manager =
        t.invite_as(&manager, "deputy@example.com", json!([{"role": "manager", "property_id": null}])).await;
    let junior =
        t.invite_as(&manager, "porter@example.com", json!([{"role": "housekeeping", "property_id": t.property}])).await;
    let resend =
        t.app.send(Method::POST, &format!("/api/v1/invitations/{owner_invitation}/resend"), Some(&manager), None).await;
    let owner_id = common::uuid(&t.app.send(Method::GET, "/api/v1/me", Some(&t.owner), None).await.body["user_id"]);
    let remove_owner = t.app.send(Method::DELETE, &format!("/api/v1/members/{owner_id}"), Some(&manager), None).await;
    let as_owner = t.invite_as(&t.owner, "deputy@example.com", json!([{"role": "owner", "property_id": null}])).await;
    let existing = t.invite_as(&t.owner, "desk@example.com", json!([{"role": "pos", "property_id": null}])).await;

    assert_eq!(as_manager.status, StatusCode::FORBIDDEN, "{:?}", as_manager.body);
    assert!(as_manager.body["detail"].as_str().unwrap().contains("manager"), "the refusal names the role");
    assert_eq!(junior.status, StatusCode::CREATED, "{:?}", junior.body);
    assert_eq!(resend.status, StatusCode::FORBIDDEN, "{:?}", resend.body);
    assert_eq!(remove_owner.status, StatusCode::FORBIDDEN, "{:?}", remove_owner.body);
    assert_eq!(as_owner.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", as_owner.body);
    assert_eq!(existing.status, StatusCode::CONFLICT, "{:?}", existing.body);

    // Front desk: every staff route and read is refused; the permissions matrix is open to every member.
    let grants = json!([{"role": "pos", "property_id": t.property}]);
    let refused = [
        t.invite_as(&desk, "x@example.com", grants.clone()).await,
        t.app.send(Method::POST, &format!("/api/v1/invitations/{owner_invitation}/resend"), Some(&desk), None).await,
        t.app.send(Method::POST, &format!("/api/v1/invitations/{owner_invitation}/revoke"), Some(&desk), None).await,
        t.set_grants(&desk, clerk, Some("\"1\""), grants).await,
        t.app.send(Method::DELETE, &format!("/api/v1/members/{clerk}"), Some(&desk), None).await,
    ];
    for response in refused {
        assert_eq!(response.status, StatusCode::FORBIDDEN, "{:?}", response.body);
    }
    for query in ["{ members { userId } }", "{ invitations(status: PENDING) { id } }"] {
        let read = t.graphql(&desk, query).await;
        assert!(read["errors"][0]["message"].as_str().unwrap().contains("permission"), "{read:?}");
    }
    let matrix = t.graphql(&desk, "{ permissionMatrix { permission allows roles } }").await;
    let rows = matrix["data"]["permissionMatrix"].as_array().unwrap();
    let staff = rows.iter().find(|row| row["permission"] == "StaffManage").unwrap();
    assert_eq!(staff["roles"], json!(["OWNER", "MANAGER"]));
    let pending = t.graphql(&manager, "{ invitations(status: PENDING) { email } }").await;
    let emails: Vec<&str> =
        pending["data"]["invitations"].as_array().unwrap().iter().map(|i| i["email"].as_str().unwrap()).collect();
    assert_eq!(emails, ["porter@example.com", "boss@example.com"]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_token_is_throttled_like_a_sign_in(_: PgPoolOptions, opts: PgConnectOptions) {
    let t = Tenant::new(opts).await;

    let mut answers = Vec::new();
    for _ in 0..identity::MAX_LOGIN_FAILURES {
        answers.push(t.accept(None, "guessed-token", Some(PASSWORD)).await.status);
    }
    let throttled = t.preview("guessed-token").await;
    let other = t.preview("another-guess").await;

    assert!(answers.iter().all(|status| *status == StatusCode::GONE), "{answers:?}");
    assert_eq!(throttled.status, StatusCode::TOO_MANY_REQUESTS, "{:?}", throttled.body);
    assert_eq!(other.status, StatusCode::GONE, "each token has its own count");
    let keys: Vec<String> =
        sqlx::query_scalar("select distinct email::text from login_failure").fetch_all(&t.superuser).await.unwrap();
    assert!(keys.iter().all(|key| key.starts_with("invitation:") && !key.contains("guess")), "{keys:?}");
}
