//! `POST /mail`: a `mail-send` push carrying a [`MailJob`]. An invitation's email is rendered from the invitation as
//! it is now, read through `app.invitation_by_id`, and sent with the service's [`mail::Mailer`].
//!
//! The answer is 204 once the email is sent, and also when there is nothing to send (the invitation is no longer
//! pending) or the provider refused it for good, which is logged as an error: delivering the job again cannot help.
//! It is 503 when the send may succeed later, so the bus delivers the job again, and 400 for a body that is not a
//! job. The job carries the invitation's token, so nothing here logs the job, the body or a decoding error (which may
//! quote the payload).

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use db::{Scope, TenantId};
use events::PushEnvelope;
use identity::Role;
use mail::{MailError, MailJob, Message};
use serde::Deserialize;
use sqlx::types::Json;
use time::OffsetDateTime;
use time::macros::format_description;
use uuid::Uuid;

use crate::JobsSvc;

pub(crate) async fn mail(State(svc): State<JobsSvc>, body: Bytes) -> StatusCode {
    let Ok(envelope) = serde_json::from_slice::<PushEnvelope>(&body) else {
        tracing::error!("a mail-send push is not a push envelope");
        return StatusCode::BAD_REQUEST;
    };
    let Ok(job) = envelope.decode::<MailJob>() else {
        tracing::error!(id = envelope.dedupe_id(), "a mail-send message is not a mail job");
        return StatusCode::BAD_REQUEST;
    };
    let MailJob::Invitation { invitation_id, tenant_id, token } = job;
    let message = match invitation_message(&svc, TenantId(tenant_id), invitation_id, &token).await {
        Ok(Some(message)) => message,
        Ok(None) => return StatusCode::NO_CONTENT,
        Err(error) => {
            tracing::warn!(invitation = %invitation_id, %error, "reading the invitation failed; it will be sent again");
            return StatusCode::SERVICE_UNAVAILABLE;
        }
    };
    match svc.mailer.send(&message).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(MailError::Retryable(error)) => {
            tracing::warn!(invitation = %invitation_id, to = %message.to, %error, "the email will be sent again");
            StatusCode::SERVICE_UNAVAILABLE
        }
        Err(MailError::Permanent(error)) => {
            tracing::error!(invitation = %invitation_id, to = %message.to, %error, "the email was refused");
            StatusCode::NO_CONTENT
        }
    }
}

#[derive(sqlx::FromRow)]
struct InvitationRow {
    tenant_id: Uuid,
    email: String,
    display_name: String,
    grants: Json<Vec<Grant>>,
    status: String,
    expires_at: OffsetDateTime,
    tenant_name: String,
    inviter_name: Option<String>,
}

#[derive(Deserialize)]
struct Grant {
    role: String,
    property_id: Option<Uuid>,
}

/// Invitation `id`'s email, or `None` when it is not `tenant`'s or no longer pending.
async fn invitation_message(
    svc: &JobsSvc,
    tenant: TenantId,
    id: Uuid,
    token: &str,
) -> Result<Option<Message>, sqlx::Error> {
    let mut tx = db::begin(&svc.pool, Scope::tenant(tenant)).await?;
    let row: Option<InvitationRow> = sqlx::query_as(
        "select tenant_id, email::text as email, display_name, grants, status, expires_at, tenant_name, inviter_name
         from app.invitation_by_id($1)",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tracing::warn!(invitation = %id, "no such invitation; nothing sent");
        return Ok(None);
    };
    if row.tenant_id != tenant.0 {
        tracing::error!(invitation = %id, "the invitation belongs to another tenant; nothing sent");
        return Ok(None);
    }
    if row.status != "pending" || row.expires_at <= OffsetDateTime::now_utc() {
        tracing::info!(invitation = %id, status = row.status, "the invitation is no longer pending; nothing sent");
        return Ok(None);
    }
    let ids: Vec<Uuid> = row.grants.iter().filter_map(|grant| grant.property_id).collect();
    let properties: HashMap<Uuid, String> =
        sqlx::query_as::<_, (Uuid, String)>("select id, name from property where id = any($1)")
            .bind(&ids)
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .collect();
    tx.commit().await?;
    Ok(Some(render(&row, &properties, &svc.app_base_url, token)))
}

fn render(row: &InvitationRow, properties: &HashMap<Uuid, String>, base_url: &str, token: &str) -> Message {
    let inviter = row.inviter_name.as_deref().unwrap_or("Someone");
    let subject = format!("{inviter} invited you to {} on GoodFolk", row.tenant_name);
    let grants: Vec<String> = row
        .grants
        .iter()
        .map(|grant| {
            let role = Role::parse(&grant.role).map_or(grant.role.as_str(), |role| role_words(role));
            match grant.property_id {
                None => format!("{role} at all properties"),
                Some(id) => match properties.get(&id) {
                    Some(name) => format!("{role} at {name}"),
                    None => format!("{role} at a property that has since been removed"),
                },
            }
        })
        .collect();
    let link = format!("{}/invite/{token}", base_url.trim_end_matches('/'));
    let expires = row
        .expires_at
        .to_offset(time::UtcOffset::UTC)
        .format(format_description!("[day padding:none] [month repr:long] [year] at [hour]:[minute] UTC"))
        .unwrap_or_else(|_| row.expires_at.to_string());

    let mut text =
        format!("Hello {},\n\n{inviter} invited you to join {} on GoodFolk as:\n\n", row.display_name, row.tenant_name);
    for grant in &grants {
        text.push_str(&format!("- {grant}\n"));
    }
    text.push_str(&format!("\nAccept the invitation here:\n{link}\n\nThe invitation expires on {expires}.\n"));

    let mut html = format!(
        "<p>Hello {},</p>\n<p>{} invited you to join {} on GoodFolk as:</p>\n<ul>\n",
        escape(&row.display_name),
        escape(inviter),
        escape(&row.tenant_name)
    );
    for grant in &grants {
        html.push_str(&format!("<li>{}</li>\n", escape(grant)));
    }
    html.push_str(&format!(
        "</ul>\n<p><a href=\"{0}\">Accept the invitation</a></p>\n<p>The invitation expires on {1}.</p>\n",
        escape(&link),
        escape(&expires)
    ));
    Message { to: row.email.clone(), subject, text, html }
}

/// The role as the email names it.
fn role_words(role: Role) -> &'static str {
    match role {
        Role::Owner => "Owner",
        Role::Manager => "Manager",
        Role::FrontDesk => "Front desk",
        Role::Housekeeping => "Housekeeping",
        Role::Accountant => "Accountant",
        Role::HkSupervisor => "Housekeeping supervisor",
        Role::Pos => "Point of sale",
    }
}

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            c => escaped.push(c),
        }
    }
    escaped
}
