//! `POST /mail`: an invitation's email, sent to the dev outbox while the invitation is pending, and never a token in
//! the logs; a weekly or monthly report's email, with its PDF from storage attached; and a booking site
//! reservation's confirmation, in the hotel's name with its manage link.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use common::hotel::{Hotel, Stay};
use common::{SECRET, http, jobs, serve, storage, svc};
use events::{PushEnvelope, PushMessage};
use jobs_svc::JobsSvc;
use mail::{DevMailer, MailJob, Message};
use sha2::{Digest, Sha256};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use storage::Bucket;
use time::macros::date;
use tracing_subscriber::fmt::MakeWriter;
use uuid::Uuid;

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

/// Installs, once, a global subscriber that records every test's logs in this binary. A per-test subscriber is
/// unreliable here: tests run in parallel and tasks move between threads.
fn captured_logs() -> &'static CapturedLogs {
    static LOGS: OnceLock<CapturedLogs> = OnceLock::new();
    LOGS.get_or_init(|| {
        let logs = CapturedLogs::default();
        tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::TRACE)
                .with_ansi(false)
                .with_writer(logs.clone())
                .finish(),
        )
        .expect("no other global subscriber in this test binary");
        logs
    })
}

/// The service with a dev mailer writing to a new directory; returns it with that directory.
async fn mailing(opts: PgConnectOptions) -> (JobsSvc, PathBuf) {
    let outbox = std::env::temp_dir().join(format!("goodfolk-jobs-svc-mail-{}", Uuid::now_v7()));
    let svc = JobsSvc {
        mailer: Arc::new(DevMailer { dir: outbox.clone() }),
        app_base_url: "https://app.example.com".to_owned(),
        ..svc(jobs(&opts).await, opts)
    };
    (svc, outbox)
}

/// An invitation of `hotel`'s tenant in `status` whose link carries `token`, from the hotel's user, as front desk at
/// the hotel and manager everywhere.
async fn invitation(hotel: &Hotel, status: &str, token: &str) -> Uuid {
    let id = Uuid::now_v7();
    let grants = serde_json::json!([
        { "role": "front_desk", "property_id": hotel.property },
        { "role": "manager", "property_id": null },
    ]);
    let mut tx = hotel.tx().await;
    sqlx::query(
        "insert into invitation (id, tenant_id, email, display_name, grants, token_hash, status, invited_by,
                                 expires_at, accepted_at)
         values ($1, $2, 'kamala@example.com', 'Kamala', $3, $4, $5, $6, now() + interval '7 days',
                 case when $5 = 'accepted' then now() end)",
    )
    .bind(id)
    .bind(hotel.tenant.0)
    .bind(grants)
    .bind(Sha256::digest(token.as_bytes()).to_vec())
    .bind(status)
    .bind(hotel.user.0)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

fn push(job: &MailJob) -> Vec<u8> {
    let id = Uuid::now_v7().to_string();
    let envelope = PushEnvelope {
        message: PushMessage {
            data: STANDARD.encode(serde_json::to_vec(job).unwrap()),
            message_id: id.clone(),
            attributes: BTreeMap::from([("message_id".to_owned(), id)]),
        },
        subscription: "push/mail-send".to_owned(),
    };
    serde_json::to_vec(&envelope).unwrap()
}

async fn post_mail(addr: std::net::SocketAddr, body: Vec<u8>, auth: Option<&str>) -> reqwest::StatusCode {
    let mut request = http().post(format!("http://{addr}/mail")).header("content-type", "application/json").body(body);
    if let Some(token) = auth {
        request = request.bearer_auth(token);
    }
    request.send().await.unwrap().status()
}

/// The messages in the dev outbox, oldest first; their attachments are the files beside them.
fn sent(outbox: &Path) -> Vec<Message> {
    let Ok(entries) = std::fs::read_dir(outbox) else { return vec![] };
    let mut files: Vec<PathBuf> = entries
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    files.iter().map(|file| serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap()).collect()
}

#[tokio::test]
async fn mail_without_a_token_is_401() {
    let pool = PgPoolOptions::new().connect_lazy_with(PgConnectOptions::new());
    let addr = serve(svc(pool, PgConnectOptions::new())).await;
    let job = MailJob::Invitation { invitation_id: Uuid::now_v7(), tenant_id: Uuid::now_v7(), token: "t".into() };

    assert_eq!(post_mail(addr, push(&job), None).await, 401);
    assert_eq!(post_mail(addr, push(&job), Some("not-the-secret")).await, 401);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_pending_invitation_sends_one_email_with_the_link(_: PgPoolOptions, opts: PgConnectOptions) {
    let logs = captured_logs();
    let hotel = Hotel::new(opts.clone()).await;
    let (svc, outbox) = mailing(opts).await;
    let addr = serve(svc).await;
    let token = "dG9rZW4tdGhhdC1tdXN0LW5ldmVyLWJlLWxvZ2dlZC0xMjM0";
    let id = invitation(&hotel, "pending", token).await;
    let job = MailJob::Invitation { invitation_id: id, tenant_id: hotel.tenant.0, token: token.into() };

    assert_eq!(post_mail(addr, push(&job), Some(SECRET)).await, 204);

    let messages = sent(&outbox);
    assert_eq!(messages.len(), 1);
    let message = &messages[0];
    assert_eq!(message.to, "kamala@example.com");
    assert_eq!(message.subject, "You are invited to join a team on GoodFolk", "no free text in the subject");
    let link = format!("https://app.example.com/invite/{token}");
    for body in [&message.text, &message.html] {
        assert!(body.contains(&link), "{body}");
        assert!(body.contains("Front desk at Hotel GAL"), "{body}");
        assert!(body.contains("Manager at all properties"), "{body}");
        assert!(body.contains("The invitation expires on "), "{body}");
        assert!(body.contains("U invited you to join T on GoodFolk"), "the names stay in the body: {body}");
        assert_eq!(body.matches(token).count(), 1, "the token is only in the link: {body}");
    }
    assert!(message.html.contains(&format!("<a href=\"{link}\">")), "{}", message.html);

    let logs = logs.text();
    assert!(!logs.contains(token), "a log line carries the token");
    // The fixture's own SQL names the address, so only the send's fields are checked.
    assert!(logs.contains("to_domain=\"example.com\""), "{logs}");
    assert!(!logs.contains("to=kamala@") && !logs.contains("to=\"kamala@"), "a recipient address: {logs}");
    std::fs::remove_dir_all(outbox).unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_invitation_no_longer_pending_sends_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let (svc, outbox) = mailing(opts).await;
    let addr = serve(svc).await;

    for status in ["revoked", "accepted", "expired"] {
        let id = invitation(&hotel, status, status).await;
        let job = MailJob::Invitation { invitation_id: id, tenant_id: hotel.tenant.0, token: status.into() };
        assert_eq!(post_mail(addr, push(&job), Some(SECRET)).await, 204, "{status}");
    }
    let unknown = MailJob::Invitation { invitation_id: Uuid::now_v7(), tenant_id: hotel.tenant.0, token: "t".into() };
    assert_eq!(post_mail(addr, push(&unknown), Some(SECRET)).await, 204);

    assert!(sent(&outbox).is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_job_whose_token_a_resend_replaced_sends_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let (svc, outbox) = mailing(opts).await;
    let addr = serve(svc).await;
    let id = invitation(&hotel, "pending", "old-token").await;
    // The resend gave the invitation a new token; its own job carries that one.
    let mut tx = hotel.tx().await;
    sqlx::query("update invitation set token_hash = $2 where id = $1")
        .bind(id)
        .bind(Sha256::digest(b"new-token").to_vec())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let job = |token: &str| MailJob::Invitation { invitation_id: id, tenant_id: hotel.tenant.0, token: token.into() };

    // The bus retries the first job after the resend.
    assert_eq!(post_mail(addr, push(&job("old-token")), Some(SECRET)).await, 204);
    assert!(sent(&outbox).is_empty(), "the superseded link was mailed");

    assert_eq!(post_mail(addr, push(&job("new-token")), Some(SECRET)).await, 204);
    let messages = sent(&outbox);
    assert_eq!(messages.len(), 1);
    assert!(messages[0].text.contains("/invite/new-token"), "{}", messages[0].text);
    std::fs::remove_dir_all(outbox).unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_invitation_of_another_tenant_sends_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let (svc, outbox) = mailing(opts).await;
    let addr = serve(svc).await;
    let id = invitation(&hotel, "pending", "token").await;

    let job = MailJob::Invitation { invitation_id: id, tenant_id: Uuid::now_v7(), token: "token".into() };
    assert_eq!(post_mail(addr, push(&job), Some(SECRET)).await, 204);
    assert!(sent(&outbox).is_empty());
}

/// A verification link for `hotel`'s user carrying `token`, sent `ago` before now.
async fn verification(hotel: &Hotel, token: &str, ago: &str) {
    sqlx::query(
        "insert into email_verification (id, user_id, token_hash, created_at, expires_at)
         values ($1, $2, $3, now() - $4::interval, now() - $4::interval + interval '1 day')",
    )
    .bind(Uuid::now_v7())
    .bind(hotel.user.0)
    .bind(Sha256::digest(token.as_bytes()).to_vec())
    .bind(ago)
    .execute(&hotel.pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_email_verification_sends_its_link_in_the_fragment_under_a_fixed_subject(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let logs = captured_logs();
    let hotel = Hotel::new(opts.clone()).await;
    let (svc, outbox) = mailing(opts).await;
    let addr = serve(svc).await;
    let token = "dmVyaWZpY2F0aW9uLXRva2VuLW5ldmVyLWxvZ2dlZC0xMjM0";
    verification(&hotel, token, "0 seconds").await;
    let email: String = sqlx::query_scalar("select email::text from app_user where id = $1")
        .bind(hotel.user.0)
        .fetch_one(&hotel.pool)
        .await
        .unwrap();
    let job = MailJob::EmailVerification { user_id: hotel.user.0, token: token.into() };

    assert_eq!(post_mail(addr, push(&job), Some(SECRET)).await, 204);

    let messages = sent(&outbox);
    assert_eq!(messages.len(), 1);
    let message = &messages[0];
    assert_eq!(message.to, email);
    assert_eq!(message.subject, "Confirm your email address for GoodFolk");
    let link = format!("https://app.example.com/verify-email#{token}");
    for body in [&message.text, &message.html] {
        assert!(body.contains(&link), "{body}");
        assert_eq!(body.matches(token).count(), 1, "the token is only in the link: {body}");
    }
    assert!(!format!("{job:?}").contains(token), "Debug hides the token");
    assert!(!logs.text().contains(token), "a log line carries the token");
    std::fs::remove_dir_all(outbox).unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_replaced_or_used_verification_link_sends_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let (svc, outbox) = mailing(opts).await;
    let addr = serve(svc).await;
    verification(&hotel, "older", "2 minutes").await;
    verification(&hotel, "newer", "1 minute").await;
    let job = |token: &str| MailJob::EmailVerification { user_id: hotel.user.0, token: token.into() };

    assert_eq!(post_mail(addr, push(&job("older")), Some(SECRET)).await, 204);
    assert!(sent(&outbox).is_empty(), "a replaced link was mailed");
    sqlx::query("update app_user set email_verified_at = now() where id = $1")
        .bind(hotel.user.0)
        .execute(&hotel.pool)
        .await
        .unwrap();
    assert_eq!(post_mail(addr, push(&job("newer")), Some(SECRET)).await, 204);
    assert!(sent(&outbox).is_empty(), "a verified user was mailed");
    let unknown = MailJob::EmailVerification { user_id: Uuid::now_v7(), token: "newer".into() };
    assert_eq!(post_mail(addr, push(&unknown), Some(SECRET)).await, 204);
    assert!(sent(&outbox).is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_body_that_is_not_a_job_is_400(_: PgPoolOptions, opts: PgConnectOptions) {
    let (svc, _) = mailing(opts).await;
    let addr = serve(svc).await;

    assert_eq!(post_mail(addr, b"not json".to_vec(), Some(SECRET)).await, 400);
    let mut envelope: serde_json::Value = serde_json::from_slice(&push(&MailJob::Invitation {
        invitation_id: Uuid::nil(),
        tenant_id: Uuid::nil(),
        token: "t".into(),
    }))
    .unwrap();
    envelope["message"]["data"] = STANDARD.encode(br#"{"Other":{}}"#).into();
    assert_eq!(post_mail(addr, serde_json::to_vec(&envelope).unwrap(), Some(SECRET)).await, 400);
}

/// Gives `hotel`'s user exactly `grants`, `(role, property)`, a `None` property being every property.
async fn grant(hotel: &Hotel, grants: &[(&str, Option<Uuid>)]) {
    let mut tx = hotel.tx().await;
    sqlx::query("delete from role_grant where user_id = $1").bind(hotel.user.0).execute(&mut *tx).await.unwrap();
    for (role, property) in grants {
        sqlx::query(
            "insert into role_grant (id, tenant_id, user_id, property_id, role)
             values (gen_random_uuid(), $1, $2, $3, $4)",
        )
        .bind(hotel.tenant.0)
        .bind(hotel.user.0)
        .bind(property)
        .bind(role)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
}

/// A rendered `kind` report of `hotel` for `from` to `to`, its PDF `pdf` in storage when given; `hotel`'s user is a
/// member of its tenant and its owner.
async fn ready_report(hotel: &Hotel, kind: &str, from: time::Date, to: time::Date, pdf: Option<&[u8]>) -> Uuid {
    let id = Uuid::now_v7();
    let key = storage::report_pdf_key(hotel.tenant.0, id);
    let headline = serde_json::json!([
        { "label": "Occupancy", "value": "72.5%", "last_year": null },
        { "label": "ADR", "value": "Rs 12,000", "last_year": null },
        { "label": "RevPAR", "value": "Rs 8,700", "last_year": null },
        { "label": "Room nights", "value": "36", "last_year": null },
        { "label": "Total revenue", "value": "Rs 1,234,567", "last_year": null },
    ]);
    let data = serde_json::json!({ "template": "summary", "data": { "title": "Weekly report", "headline": headline } });
    let mut tx = hotel.tx().await;
    sqlx::query("insert into membership (tenant_id, user_id) values ($1, $2) on conflict do nothing")
        .bind(hotel.tenant.0)
        .bind(hotel.user.0)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "insert into report (id, tenant_id, property_id, kind, period_start, period_end, data, status, pdf_key,
                             rendered_at)
         values ($1, $2, $3, $4, $5, $6, $7, 'ready', $8, now())",
    )
    .bind(id)
    .bind(hotel.tenant.0)
    .bind(hotel.property)
    .bind(kind)
    .bind(from)
    .bind(to)
    .bind(data)
    .bind(&key)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    grant(hotel, &[("owner", None)]).await;
    if let Some(pdf) = pdf {
        storage().await.put(Bucket::Private, &key, pdf.to_vec(), "application/pdf", None).await.unwrap();
    }
    id
}

fn report_job(hotel: &Hotel, report_id: Uuid) -> MailJob {
    MailJob::Report { tenant_id: hotel.tenant.0, report_id, user_id: hotel.user.0 }
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_report_email_carries_the_headline_and_attaches_the_pdf(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let (svc, outbox) = mailing(opts).await;
    let addr = serve(JobsSvc { storage: Some(storage().await), ..svc }).await;
    let pdf = format!("%PDF-1.7 weekly {}", Uuid::now_v7()).into_bytes();
    let weekly = ready_report(&hotel, "weekly", date!(2026 - 10 - 19), date!(2026 - 10 - 25), Some(&pdf)).await;
    let monthly = ready_report(&hotel, "monthly", date!(2026 - 10 - 01), date!(2026 - 10 - 31), Some(b"%PDF-")).await;

    assert_eq!(post_mail(addr, push(&report_job(&hotel, weekly)), Some(SECRET)).await, 204);
    assert_eq!(post_mail(addr, push(&report_job(&hotel, monthly)), Some(SECRET)).await, 204);

    let messages = sent(&outbox);
    assert_eq!(messages.len(), 2);
    let (message, month) = (&messages[0], &messages[1]);
    assert_eq!(message.to, format!("{}@example.com", hotel.user.0));
    assert_eq!(message.subject, "Hotel GAL weekly report, 19–25 Oct 2026");
    for line in ["Occupancy: 72.5%", "ADR: Rs 12,000", "RevPAR: Rs 8,700", "Total revenue: Rs 1,234,567"] {
        assert!(message.text.contains(line), "{line} in {}", message.text);
        assert!(message.html.contains(line), "{line} in {}", message.html);
    }
    assert!(!message.text.contains("Room nights"), "{}", message.text);
    assert!(message.text.contains("You can turn this email off on the Reports page."), "{}", message.text);
    assert!(message.html.contains("You can turn this email off on the Reports page."), "{}", message.html);
    assert_eq!(message.attachments.len(), 1);
    assert_eq!(
        (message.attachments[0].filename.as_str(), message.attachments[0].content_type.as_str()),
        ("weekly-report-2026-10-19.pdf", "application/pdf")
    );
    let beside = std::fs::read_dir(&outbox)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.to_string_lossy().ends_with("-weekly-report-2026-10-19.pdf"))
        .unwrap();
    assert_eq!(std::fs::read(beside).unwrap(), pdf);
    assert_eq!(month.subject, "Hotel GAL monthly report, October 2026");
    assert_eq!(month.attachments[0].filename, "monthly-report-2026-10-01.pdf");
    std::fs::remove_dir_all(outbox).unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_report_whose_pdf_cannot_be_read_is_retried(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let (svc, outbox) = mailing(opts).await;
    let without_storage = serve(svc.clone()).await;
    let addr = serve(JobsSvc { storage: Some(storage().await), ..svc }).await;
    let report = ready_report(&hotel, "weekly", date!(2026 - 10 - 19), date!(2026 - 10 - 25), None).await;

    assert_eq!(post_mail(addr, push(&report_job(&hotel, report)), Some(SECRET)).await, 503);
    assert_eq!(post_mail(without_storage, push(&report_job(&hotel, report)), Some(SECRET)).await, 503);
    assert!(sent(&outbox).is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_report_job_for_no_such_report_or_member_sends_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let (svc, outbox) = mailing(opts).await;
    let addr = serve(JobsSvc { storage: Some(storage().await), ..svc }).await;
    let report = ready_report(&hotel, "weekly", date!(2026 - 10 - 19), date!(2026 - 10 - 25), Some(b"%PDF-")).await;

    let unknown = report_job(&hotel, Uuid::now_v7());
    let stranger = MailJob::Report { tenant_id: hotel.tenant.0, report_id: report, user_id: Uuid::now_v7() };
    let other_tenant = MailJob::Report { tenant_id: Uuid::now_v7(), report_id: report, user_id: hotel.user.0 };
    for job in [unknown, stranger, other_tenant] {
        assert_eq!(post_mail(addr, push(&job), Some(SECRET)).await, 204, "{job:?}");
    }
    assert!(sent(&outbox).is_empty());
}

/// The recipient is checked again when the email is sent: an owner or manager of the property then, or an owner or
/// manager of every property. Anyone else, such as someone whose grant was revoked since the PDF was ready, gets
/// nothing.
#[sqlx::test(migrations = "../../migrations")]
async fn a_report_email_goes_only_to_an_owner_or_manager_of_the_property_now(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let (svc, outbox) = mailing(opts).await;
    let addr = serve(JobsSvc { storage: Some(storage().await), ..svc }).await;
    let report = ready_report(&hotel, "weekly", date!(2026 - 10 - 19), date!(2026 - 10 - 25), Some(b"%PDF-")).await;
    let other = hotel.add_property("OTH").await;

    for grants in [vec![], vec![("accountant", Some(hotel.property))], vec![("manager", Some(other))]] {
        grant(&hotel, &grants).await;
        assert_eq!(post_mail(addr, push(&report_job(&hotel, report)), Some(SECRET)).await, 204, "{grants:?}");
        assert!(sent(&outbox).is_empty(), "{grants:?}");
    }
    for grants in [vec![("manager", Some(hotel.property))], vec![("manager", None)]] {
        grant(&hotel, &grants).await;
        assert_eq!(post_mail(addr, push(&report_job(&hotel, report)), Some(SECRET)).await, 204, "{grants:?}");
    }
    assert_eq!(sent(&outbox).len(), 2);
    std::fs::remove_dir_all(outbox).unwrap();
}

/// A booking-site reservation of `hotel` booked by Kamala Perera (kamala@example.com) for 2 nights from D + 10 at 10 000
/// a night with 2 000 breakfast (before taxes), under one night's cancellation penalty and a 50% deposit due 7 days before arrival,
/// whose manage link carries `token`; the hotel's site has a contact phone and email.
async fn site_booking(hotel: &Hotel, token: &str) -> Uuid {
    let terms = serde_json::json!({
        "rules": [{"days_before_arrival": 3, "penalty": {"kind": "nights", "value": 1}}],
        "no_show": {"kind": "nights", "value": 1},
    });
    let deposit =
        serde_json::json!({"kind": "percent", "value": 5000, "due": {"type": "days_before_arrival", "days": 7}});
    let stay = Stay {
        source: "ibe",
        meal_amount: 2_000,
        terms: Some(terms),
        deposit_terms: Some(deposit),
        first_name: "Kamala",
        last_name: "Perera",
        booked: 0,
        ..Stay::new(hotel.property, "confirmed", 10, 12)
    };
    let booked = hotel.stay(stay).await;
    let mut tx = hotel.tx().await;
    sqlx::query(
        "update guest set email = 'kamala@example.com'
         where id = (select booker_guest_id from reservation where id = $1)",
    )
    .bind(booked.reservation)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "insert into ibe_site (tenant_id, property_id, enabled, public_code, contact_phone, contact_email)
         values ($1, $2, true, 'GAL', '+94 91 222 3333', 'desk@gallefort.example')",
    )
    .bind(hotel.tenant.0)
    .bind(hotel.property)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "insert into reservation_access_token (tenant_id, reservation_id, sha256, expires_at)
         values ($1, $2, $3, now() + interval '30 days')",
    )
    .bind(hotel.tenant.0)
    .bind(booked.reservation)
    .bind(Sha256::digest(token.as_bytes()).to_vec())
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    booked.reservation
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_ibe_confirmation_is_sent_in_the_hotels_name_with_the_manage_link(_: PgPoolOptions, opts: PgConnectOptions) {
    let logs = captured_logs();
    let hotel = Hotel::new(opts.clone()).await;
    let (svc, outbox) = mailing(opts).await;
    let addr = serve(JobsSvc { ibe_public_origin: "https://book.example.lk".into(), ..svc }).await;
    let token = "bWFuYWdlLXRva2VuLXRoYXQtbXVzdC1uZXZlci1iZS1sb2dnZWQ";
    let reservation = site_booking(&hotel, token).await;
    let job = MailJob::IbeConfirmation { tenant_id: hotel.tenant.0, reservation_id: reservation, token: token.into() };

    assert_eq!(post_mail(addr, push(&job), Some(SECRET)).await, 204);
    let stale =
        MailJob::IbeConfirmation { tenant_id: hotel.tenant.0, reservation_id: reservation, token: "old".into() };
    assert_eq!(post_mail(addr, push(&stale), Some(SECRET)).await, 204, "a token no longer the booking's");

    let messages = sent(&outbox);
    assert_eq!(messages.len(), 1);
    let message = &messages[0];
    let confirmation_no: String = sqlx::query_scalar("select confirmation_no from reservation where id = $1")
        .bind(reservation)
        .fetch_one(&mut *hotel.tx().await)
        .await
        .unwrap();
    assert_eq!(message.to, "kamala@example.com");
    assert_eq!(message.subject, format!("Your booking at Hotel GAL: {confirmation_no}"));
    assert_eq!(message.from_name.as_deref(), Some("Hotel GAL"));
    assert_eq!(message.reply_to.as_deref(), Some("desk@gallefort.example"));
    let link = format!("https://book.example.lk/GAL/manage?token={token}");
    // The property's tax rules apply, as the booking's search priced it.
    let confirmation = ibe::confirmation(&mut hotel.tx().await, reservation, token).await.unwrap().unwrap();
    let total = ibe::money_text("LKR", confirmation.totals["LKR"]);
    assert!(confirmation.totals["LKR"] > 24_000, "taxed: {:?}", confirmation.totals);
    let due =
        hotel.day(3).format(time::macros::format_description!("[day padding:none] [month repr:long] [year]")).unwrap();
    for body in [&message.text, &message.html] {
        assert!(body.contains(&link), "{body}");
        assert!(body.contains("Kamala Perera"), "{body}");
        assert!(body.contains("Total, taxes included: ") && body.contains(&total), "{body}");
        assert!(body.contains("bed and breakfast"), "{body}");
        assert!(
            body.contains("Free cancellation until 3 days before arrival; after that, one night is charged"),
            "{body}"
        );
        assert!(
            body.contains("Deposit (before taxes): LKR 120.00 by ") && body.contains(&due),
            "the deposit and its due date: {body}"
        );
        assert!(body.contains("The hotel will contact you about the deposit"), "{body}");
        assert!(body.contains("+94 91 222 3333") && body.contains("desk@gallefort.example"), "{body}");
        assert_eq!(body.matches(token).count(), 1, "the token is only in the link: {body}");
    }
    assert!(message.html.contains(&format!("<a href=\"{link}\">")), "{}", message.html);
    assert!(!logs.text().contains(token), "a log line carries the token");
    std::fs::remove_dir_all(outbox).unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_ibe_cancellation_names_the_charge_and_carries_no_link(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let (svc, outbox) = mailing(opts).await;
    let addr = serve(JobsSvc { ibe_public_origin: "https://book.example.lk".into(), ..svc }).await;
    let token = "bWFuYWdlLXRva2VuLXRoYXQtbXVzdC1uZXZlci1iZS1sb2dnZWQ";
    let reservation = site_booking(&hotel, token).await;
    let rooms: Vec<Uuid> = sqlx::query_scalar("select id from reservation_room where reservation_id = $1")
        .bind(reservation)
        .fetch_all(&mut *hotel.tx().await)
        .await
        .unwrap();
    let job = MailJob::IbeCancellation { tenant_id: hotel.tenant.0, reservation_id: reservation, rooms };

    assert_eq!(post_mail(addr, push(&job), Some(SECRET)).await, 204);
    assert!(sent(&outbox).is_empty(), "a booking that is not cancelled has no cancellation to send");

    let mut tx = hotel.tx().await;
    sqlx::query(
        "update reservation_room set status = 'cancelled', cancelled_at = now(), cancellation_penalty = 10000,
                cancelled_business_date = $2
         where reservation_id = $1",
    )
    .bind(reservation)
    .bind(hotel.day(0))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(post_mail(addr, push(&job), Some(SECRET)).await, 204);

    let messages = sent(&outbox);
    assert_eq!(messages.len(), 1);
    let message = &messages[0];
    let confirmation_no: String = sqlx::query_scalar("select confirmation_no from reservation where id = $1")
        .bind(reservation)
        .fetch_one(&mut *hotel.tx().await)
        .await
        .unwrap();
    assert_eq!(message.to, "kamala@example.com");
    assert_eq!(message.subject, format!("Your booking at Hotel GAL is cancelled: {confirmation_no}"));
    assert_eq!(message.from_name.as_deref(), Some("Hotel GAL"));
    assert_eq!(message.reply_to.as_deref(), Some("desk@gallefort.example"));
    for body in [&message.text, &message.html] {
        assert!(body.contains("Kamala Perera") && body.contains(&confirmation_no), "{body}");
        assert!(body.contains("The cancellation charge under your booking terms is LKR 100.00"), "{body}");
        assert!(body.contains("+94 91 222 3333") && body.contains("desk@gallefort.example"), "{body}");
        assert!(!body.contains(token) && !body.contains("manage"), "no link: {body}");
    }
    std::fs::remove_dir_all(outbox).unwrap();
}
