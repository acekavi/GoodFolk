use std::path::PathBuf;

use uuid::Uuid;

use crate::{MailError, Mailer, Message};

/// Development and tests: each message becomes `<dir>/<uuid>.json` (`{to, subject, text, html}`, with `attachments`
/// listing each attachment's `filename` and `content_type` when it has any), and each attachment
/// `<dir>/<uuid>-<filename>` beside it, written first. The ids are UUIDv7, so the files sort in the order they were
/// sent. Each file is written under another name and renamed into place, so a reader never sees half of one.
#[derive(Debug, Clone)]
pub struct DevMailer {
    pub dir: PathBuf,
}

#[async_trait::async_trait]
impl Mailer for DevMailer {
    async fn send(&self, message: &Message) -> Result<(), MailError> {
        let failed = |error: std::io::Error| MailError::Retryable(format!("writing to the mail outbox: {error}"));
        let json = serde_json::to_vec_pretty(message).map_err(|error| MailError::Permanent(error.to_string()))?;
        tokio::fs::create_dir_all(&self.dir).await.map_err(failed)?;
        let id = Uuid::now_v7();
        for attachment in &message.attachments {
            let name = format!("{id}-{}", safe_filename(&attachment.filename));
            let partial = self.dir.join(format!("{name}.partial"));
            tokio::fs::write(&partial, &attachment.bytes).await.map_err(failed)?;
            tokio::fs::rename(&partial, self.dir.join(name)).await.map_err(failed)?;
        }
        let partial = self.dir.join(format!("{id}.json.partial"));
        tokio::fs::write(&partial, json).await.map_err(failed)?;
        tokio::fs::rename(&partial, self.dir.join(format!("{id}.json"))).await.map_err(failed)?;
        tracing::info!(to = %message.to, subject = %message.subject, "mail written to the outbox");
        Ok(())
    }
}

/// `filename` with every character but ASCII letters, digits, `.`, `-` and `_` replaced by `_`, so it names a file in
/// the outbox and nowhere else.
pub(crate) fn safe_filename(filename: &str) -> String {
    let safe: String = filename
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect();
    if safe.trim_matches('.').is_empty() { "attachment".to_owned() } else { safe }
}
