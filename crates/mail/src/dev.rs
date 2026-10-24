use std::path::PathBuf;

use uuid::Uuid;

use crate::{MailError, Mailer, Message};

/// Development and tests: each message becomes `<dir>/<uuid>.json` (`{to, subject, text, html}`). The ids are UUIDv7,
/// so the files sort in the order they were sent. Each file is written under another name and renamed into place, so
/// a reader never sees half of one.
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
        let partial = self.dir.join(format!("{id}.json.partial"));
        tokio::fs::write(&partial, json).await.map_err(failed)?;
        tokio::fs::rename(&partial, self.dir.join(format!("{id}.json"))).await.map_err(failed)?;
        tracing::info!(to = %message.to, subject = %message.subject, "mail written to the outbox");
        Ok(())
    }
}
