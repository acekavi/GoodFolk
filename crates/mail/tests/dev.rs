//! The dev mailer's outbox, and choosing a mailer from the environment.

use mail::{Attachment, DevMailer, MailJob, Mailer, Message};
use uuid::Uuid;

fn message() -> Message {
    Message {
        to: "kamala@example.com".into(),
        from_name: None,
        reply_to: None,
        subject: "Nimal invited you to Lagoon on GoodFolk".into(),
        text: "Hello".into(),
        html: "<p>Hello</p>".into(),
        attachments: vec![],
    }
}

#[tokio::test]
async fn the_dev_mailer_writes_each_message_as_a_json_file() {
    let dir = std::env::temp_dir().join(format!("goodfolk-mail-test-{}", Uuid::now_v7()));
    let mailer = DevMailer { dir: dir.clone() };

    mailer.send(&message()).await.unwrap();

    let files: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|entry| entry.unwrap().path()).collect();
    assert_eq!(files.len(), 1, "one file, and no partial one left: {files:?}");
    assert_eq!(files[0].extension().unwrap(), "json");
    let written: serde_json::Value = serde_json::from_slice(&std::fs::read(&files[0]).unwrap()).unwrap();
    assert_eq!(
        written,
        serde_json::json!({
            "to": "kamala@example.com",
            "subject": "Nimal invited you to Lagoon on GoodFolk",
            "text": "Hello",
            "html": "<p>Hello</p>",
        })
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn dev_mailer_writes_attachment() {
    let dir = std::env::temp_dir().join(format!("goodfolk-mail-test-{}", Uuid::now_v7()));
    let mailer = DevMailer { dir: dir.clone() };
    let pdf = b"%PDF-1.7 the report".to_vec();
    let attachment = Attachment {
        filename: "weekly-report-2026-10-19.pdf".into(),
        content_type: "application/pdf".into(),
        bytes: pdf.clone(),
    };

    mailer.send(&Message { attachments: vec![attachment], ..message() }).await.unwrap();

    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|entry| entry.unwrap().path()).collect();
    files.sort();
    assert_eq!(files.len(), 2, "the message and its attachment, and no partial file: {files:?}");
    let json = files.iter().find(|file| file.extension().unwrap() == "json").unwrap();
    let written: serde_json::Value = serde_json::from_slice(&std::fs::read(json).unwrap()).unwrap();
    assert_eq!(
        written["attachments"],
        serde_json::json!([{ "filename": "weekly-report-2026-10-19.pdf", "content_type": "application/pdf" }])
    );
    let id = json.file_stem().unwrap().to_str().unwrap();
    let beside = dir.join(format!("{id}-weekly-report-2026-10-19.pdf"));
    assert_eq!(std::fs::read(beside).unwrap(), pdf);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn production_needs_ses_and_development_uses_the_outbox() {
    let none = |_: &str| None;
    let missing = mail::from_vars(none, true).err().expect("production refuses the dev mailer");
    assert!(missing.to_string().contains("is required in production"), "{missing}");

    let ses = |name: &str| match name {
        "MAIL_FROM" => Some("GoodFolk <hello@example.com>".to_owned()),
        "AWS_REGION" => Some("ap-south-1".to_owned()),
        "AWS_ACCESS_KEY_ID" => Some("AKIDEXAMPLE".to_owned()),
        "AWS_SECRET_ACCESS_KEY" => Some("secret".to_owned()),
        _ => None,
    };
    assert!(mail::from_vars(ses, true).is_ok());
    assert!(mail::from_vars(none, false).is_ok());
}

#[test]
fn each_missing_production_variable_is_named() {
    const SES: [(&str, &str); 4] = [
        ("MAIL_FROM", "GoodFolk <hello@example.com>"),
        ("AWS_REGION", "ap-south-1"),
        ("AWS_ACCESS_KEY_ID", "AKIDEXAMPLE"),
        ("AWS_SECRET_ACCESS_KEY", "secret"),
    ];
    for (missing, _) in SES {
        let var = |name: &str| {
            SES.iter().find(|(key, _)| *key == name && *key != missing).map(|(_, value)| (*value).to_owned())
        };
        let err = mail::from_vars(var, true).err().expect("a production variable is missing");
        assert_eq!(err.to_string(), format!("{missing} is required in production"));
    }
}

#[test]
fn a_mail_job_keeps_its_token_out_of_debug() {
    let job = MailJob::Invitation { invitation_id: Uuid::nil(), tenant_id: Uuid::nil(), token: "s3cr3t-token".into() };

    assert!(!format!("{job:?}").contains("s3cr3t-token"));
    let json = serde_json::to_string(&job).unwrap();
    assert_eq!(serde_json::from_str::<MailJob>(&json).unwrap(), job);
}

#[test]
fn report_mail_job_debug_has_no_secrets() {
    let (tenant_id, report_id, user_id) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let job = MailJob::Report { tenant_id, report_id, user_id };

    assert_eq!(
        format!("{job:?}"),
        format!("Report {{ tenant_id: {tenant_id}, report_id: {report_id}, user_id: {user_id} }}")
    );
    let json = serde_json::to_string(&job).unwrap();
    assert_eq!(serde_json::from_str::<MailJob>(&json).unwrap(), job);
}

#[test]
fn ibe_confirmation_mail_job_keeps_its_token_out_of_debug() {
    let job = MailJob::IbeConfirmation {
        tenant_id: Uuid::now_v7(),
        reservation_id: Uuid::now_v7(),
        token: "m4n4ge-token".into(),
    };

    assert!(!format!("{job:?}").contains("m4n4ge-token"));
    let json = serde_json::to_string(&job).unwrap();
    assert_eq!(serde_json::from_str::<MailJob>(&json).unwrap(), job);
}

#[test]
fn ibe_cancellation_mail_job_round_trips_and_carries_no_token() {
    let job = MailJob::IbeCancellation { tenant_id: Uuid::now_v7(), reservation_id: Uuid::now_v7() };

    let json = serde_json::to_string(&job).unwrap();
    assert!(!json.contains("token"));
    assert_eq!(serde_json::from_str::<MailJob>(&json).unwrap(), job);
}
