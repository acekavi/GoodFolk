//! Outgoing email. A [`Mailer`] sends one [`Message`]: [`SesMailer`] through Amazon SES in production, [`DevMailer`]
//! into a local outbox directory everywhere else. [`from_env`] picks one.
//!
//! **The SES client.** [`SesMailer`] calls the SES v2 `SendEmail` API itself, signing with `aws-sigv4` and sending
//! with the workspace's `reqwest`. The official `aws-sdk-sesv2` was compared (with only its `rustls` feature): it adds
//! twice the new crates (22 against 11, all of them `aws-*` support crates plus four small ones) and, worse, brings a
//! second, older TLS stack (rustls 0.21, hyper 0.14, h2 0.3) that fails `cargo deny check` with seven advisories. The
//! one API call this crate needs does not justify either.
//!
//! **Attachments.** A message with attachments goes to SES as raw MIME (see [`SesMailer`]); one without, as SES's
//! simple content.
//!
//! **The token in the outbox.** An invitation's link and a site booking's manage link must carry their token, and the
//! token is stored only as a hash, so [`MailJob::Invitation`] and [`MailJob::IbeConfirmation`] carry the token itself
//! through the outbox to the sender. The undelivered outbox row is the one place it is kept: the relay drops it from
//! the row once the row is delivered or refused (migrations 0038 and 0048).
//! Nothing here logs a job or a message body: a send logs the recipient and the subject only, and [`MailJob`]'s
//! `Debug` hides the token.

mod dev;
mod mime;
mod ses;

use std::sync::Arc;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use dev::DevMailer;
pub use ses::{Credentials, SesMailer, sign};

/// The outbox topic that carries [`MailJob`]s to jobs-svc's `POST /mail`.
pub const SEND_TOPIC: &str = "mail-send";

/// Where [`DevMailer`] writes when `MAIL_OUTBOX_DIR` is not set.
pub const DEFAULT_OUTBOX_DIR: &str = "target/mail-outbox";

/// One email to one recipient, in text and HTML, with its attachments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub to: String,
    /// The sender's display name in place of `MAIL_FROM`'s own, such as a hotel's name; the address stays the same.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_name: Option<String>,
    /// Where replies go, such as a hotel's contact email, instead of the sender.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    pub subject: String,
    pub text: String,
    pub html: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
}

/// A file attached to a [`Message`]. Its bytes are not serialized (the dev mailer writes them to a file of their
/// own), and `Debug` shows only their length.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    /// The name the recipient sees, such as `weekly-report-2026-10-19.pdf`.
    pub filename: String,
    /// Its media type, such as `application/pdf`.
    pub content_type: String,
    #[serde(skip)]
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for Attachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Attachment")
            .field("filename", &self.filename)
            .field("content_type", &self.content_type)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

#[async_trait::async_trait]
pub trait Mailer: Send + Sync {
    async fn send(&self, message: &Message) -> Result<(), MailError>;
}

/// A failed send. The detail never contains the message's body.
#[derive(Debug, thiserror::Error)]
pub enum MailError {
    /// Worth trying again later: a throttle, a server error or a failed connection.
    #[error("{0}")]
    Retryable(String),
    /// Trying again cannot help: the provider refused this message.
    #[error("{0}")]
    Permanent(String),
}

/// The `mail-send` payload the outbox carries.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MailJob {
    /// Send invitation `invitation_id`'s email; `token` is what its link carries.
    Invitation { invitation_id: Uuid, tenant_id: Uuid, token: String },
    /// Send weekly or monthly report `report_id`, its PDF attached, to `user_id`.
    Report { tenant_id: Uuid, report_id: Uuid, user_id: Uuid },
    /// Send the booker of booking-site reservation `reservation_id` its confirmation; `token` is what its manage link
    /// carries.
    IbeConfirmation { tenant_id: Uuid, reservation_id: Uuid, token: String },
    /// Tell the booker of booking-site reservation `reservation_id` it is cancelled. The email carries no link.
    IbeCancellation { tenant_id: Uuid, reservation_id: Uuid },
}

impl std::fmt::Debug for MailJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invitation { invitation_id, tenant_id, token: _ } => f
                .debug_struct("Invitation")
                .field("invitation_id", invitation_id)
                .field("tenant_id", tenant_id)
                .field("token", &"<redacted>")
                .finish(),
            Self::Report { tenant_id, report_id, user_id } => f
                .debug_struct("Report")
                .field("tenant_id", tenant_id)
                .field("report_id", report_id)
                .field("user_id", user_id)
                .finish(),
            Self::IbeConfirmation { tenant_id, reservation_id, token: _ } => f
                .debug_struct("IbeConfirmation")
                .field("tenant_id", tenant_id)
                .field("reservation_id", reservation_id)
                .field("token", &"<redacted>")
                .finish(),
            Self::IbeCancellation { tenant_id, reservation_id } => f
                .debug_struct("IbeCancellation")
                .field("tenant_id", tenant_id)
                .field("reservation_id", reservation_id)
                .finish(),
        }
    }
}

/// In production (`production`), a [`SesMailer`] from `MAIL_FROM`, `AWS_REGION`, `AWS_ACCESS_KEY_ID`,
/// `AWS_SECRET_ACCESS_KEY` and `AWS_SESSION_TOKEN` (optional), each but the last required; the dev mailer is never
/// used there. Elsewhere a [`DevMailer`] writing to `MAIL_OUTBOX_DIR` (default [`DEFAULT_OUTBOX_DIR`]).
pub fn from_env(production: bool) -> anyhow::Result<Arc<dyn Mailer>> {
    from_vars(|name| std::env::var(name).ok(), production)
}

/// As [`from_env`], reading variables through `var`.
pub fn from_vars(var: impl Fn(&str) -> Option<String>, production: bool) -> anyhow::Result<Arc<dyn Mailer>> {
    if !production {
        let dir = var("MAIL_OUTBOX_DIR").filter(|dir| !dir.is_empty()).unwrap_or_else(|| DEFAULT_OUTBOX_DIR.into());
        return Ok(Arc::new(DevMailer { dir: dir.into() }));
    }
    let required = |name: &str| {
        var(name).filter(|value| !value.is_empty()).with_context(|| format!("{name} is required in production"))
    };
    let credentials = Credentials {
        access_key_id: required("AWS_ACCESS_KEY_ID")?,
        secret_access_key: required("AWS_SECRET_ACCESS_KEY")?,
        session_token: var("AWS_SESSION_TOKEN").filter(|token| !token.is_empty()),
    };
    Ok(Arc::new(SesMailer::new(&required("AWS_REGION")?, &required("MAIL_FROM")?, credentials)?))
}
