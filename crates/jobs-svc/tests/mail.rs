//! `POST /mail`: an invitation's email, sent to the dev outbox while the invitation is pending, and never a token in
//! the logs; and a weekly or monthly report's email, with its PDF from storage attached.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use common::hotel::Hotel;
use common::{SECRET, http, serve, storage, svc};
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
            tracing_subscriber::fmt().with_max_level(tracing::Level::TRACE).with_writer(logs.clone()).finish(),
        )
        .expect("no other global subscriber in this test binary");
        logs
    })
}

/// The service with a dev mailer writing to a new directory; returns it with that directory.
fn mailing(hotel: &Hotel, opts: PgConnectOptions) -> (JobsSvc, PathBuf) {
    let outbox = std::env::temp_dir().join(format!("goodfolk-jobs-svc-mail-{}", Uuid::now_v7()));
    let svc = JobsSvc {
        mailer: Arc::new(DevMailer { dir: outbox.clone() }),
        app_base_url: "https://app.example.com".to_owned(),
        ..svc(hotel.pool.clone(), opts)
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
    let (svc, outbox) = mailing(&hotel, opts);
    let addr = serve(svc).await;
    let token = "dG9rZW4tdGhhdC1tdXN0LW5ldmVyLWJlLWxvZ2dlZC0xMjM0";
    let id = invitation(&hotel, "pending", token).await;
    let job = MailJob::Invitation { invitation_id: id, tenant_id: hotel.tenant.0, token: token.into() };

    assert_eq!(post_mail(addr, push(&job), Some(SECRET)).await, 204);

    let messages = sent(&outbox);
    assert_eq!(messages.len(), 1);
    let message = &messages[0];
    assert_eq!(message.to, "kamala@example.com");
    assert_eq!(message.subject, "U invited you to T on GoodFolk");
    let link = format!("https://app.example.com/invite/{token}");
    for body in [&message.text, &message.html] {
        assert!(body.contains(&link), "{body}");
        assert!(body.contains("Front desk at Hotel GAL"), "{body}");
        assert!(body.contains("Manager at all properties"), "{body}");
        assert!(body.contains("The invitation expires on "), "{body}");
        assert_eq!(body.matches(token).count(), 1, "the token is only in the link: {body}");
    }
    assert!(message.html.contains(&format!("<a href=\"{link}\">")), "{}", message.html);

    let logs = logs.text();
    assert!(logs.contains("kamala@example.com") && logs.contains("U invited you to T on GoodFolk"), "{logs}");
    assert!(!logs.contains(token), "a log line carries the token");
    std::fs::remove_dir_all(outbox).unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_invitation_no_longer_pending_sends_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let (svc, outbox) = mailing(&hotel, opts);
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
    let (svc, outbox) = mailing(&hotel, opts);
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
    let (svc, outbox) = mailing(&hotel, opts);
    let addr = serve(svc).await;
    let id = invitation(&hotel, "pending", "token").await;

    let job = MailJob::Invitation { invitation_id: id, tenant_id: Uuid::now_v7(), token: "token".into() };
    assert_eq!(post_mail(addr, push(&job), Some(SECRET)).await, 204);
    assert!(sent(&outbox).is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_body_that_is_not_a_job_is_400(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let (svc, _) = mailing(&hotel, opts);
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

/// A rendered `kind` report of `hotel` for `from` to `to`, its PDF `pdf` in storage when given; `hotel`'s user is a
/// member of its tenant.
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
    let (svc, outbox) = mailing(&hotel, opts);
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
    let (svc, outbox) = mailing(&hotel, opts);
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
    let (svc, outbox) = mailing(&hotel, opts);
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
