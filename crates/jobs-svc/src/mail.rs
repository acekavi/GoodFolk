//! `POST /mail`: a `mail-send` push carrying a [`MailJob`], sent with the service's [`mail::Mailer`].
//!
//! - An invitation's email is rendered from the invitation as it is now, read through `app.invitation_by_id`. There is
//!   nothing to send when the invitation is no longer pending, or a resend has replaced the job's token, so its link
//!   would not work.
//! - A weekly or monthly report's email carries its headline figures, as its PDF prints them, and the PDF itself, read
//!   from storage. There is nothing to send when the report is gone, or the recipient is no longer an owner or manager
//!   of its property (or of every property); a PDF that cannot be read yet is tried again.
//!
//! - A booking site reservation's confirmation is rendered from the booking as it is now, read by
//!   `ibe::confirmation`: the stay, its rooms and totals with the taxes on the bill, the cancellation terms it kept,
//!   any advisory deposit, the hotel's contact details and the manage link. It is sent in the hotel's name, with
//!   replies going to the hotel's contact email. There is nothing to send when the reservation is gone or its manage
//!   token is no longer the job's.
//!
//! - A booking site reservation's cancellation is rendered from the booking as it is now, read by `ibe::cancellation`:
//!   the stay, and what cancelling cost. It carries no link. There is nothing to send when the reservation is gone or
//!   a room of it is not cancelled.
//!
//! - An email verification asks the person who signed up to confirm their address, with a link carrying the token in
//!   its fragment, which browsers never send to a server. There is nothing to send when the user is gone or already
//!   verified, or the job's token is no longer their newest unexpired link.
//!
//! No subject of an email to someone outside the tenant carries text the tenant chose (its name or an inviter's),
//! except a hotel's own name on its guests' booking emails: a subject is what a phishing message would forge, so the
//! platform's own emails keep fixed ones. A report's subject names its property, but it goes to the tenant's own staff.
//!
//! The answer is 204 once the email is sent, also when there is nothing to send or the provider refused it for good,
//! which is logged as an error: delivering the job again cannot help. It is 503 when the send may succeed later, so the
//! bus delivers the job again, and 400 for a body that is not a job. An invitation's, a booking's and a verification's
//! job carry a token, so nothing here logs the job's fields (its `Debug` hides the token), the body or a decoding error
//! (which may quote the payload). A recipient is logged by its domain only.

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use db::{Scope, TenantId};
use events::PushEnvelope;
use identity::Role;
use mail::{Attachment, MailError, MailJob, Message};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::types::Json;
use storage::Bucket;
use time::format_description::BorrowedFormatItem;
use time::macros::format_description;
use time::{Date, OffsetDateTime};
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
    let message = match &job {
        MailJob::Invitation { invitation_id, tenant_id, token } => {
            invitation_message(&svc, TenantId(*tenant_id), *invitation_id, token).await.map_err(|e| e.to_string())
        }
        MailJob::Report { tenant_id, report_id, user_id } => {
            report_message(&svc, TenantId(*tenant_id), *report_id, *user_id).await
        }
        MailJob::IbeConfirmation { tenant_id, reservation_id, token } => {
            confirmation_message(&svc, TenantId(*tenant_id), *reservation_id, token).await
        }
        MailJob::IbeCancellation { tenant_id, reservation_id, rooms } => {
            cancellation_message(&svc, TenantId(*tenant_id), *reservation_id, rooms).await
        }
        MailJob::EmailVerification { user_id, token } => {
            verification_message(&svc, *user_id, token).await.map_err(|e| e.to_string())
        }
    };
    let message = match message {
        Ok(Some(message)) => message,
        Ok(None) => return StatusCode::NO_CONTENT,
        Err(error) => {
            tracing::warn!(?job, error, "building the email failed; it will be sent again");
            return StatusCode::SERVICE_UNAVAILABLE;
        }
    };
    match svc.mailer.send(&message).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(MailError::Retryable(error)) => {
            let to_domain = mail::recipient_domain(&message.to);
            tracing::warn!(?job, to_domain, %error, "the email will be sent again");
            StatusCode::SERVICE_UNAVAILABLE
        }
        Err(MailError::Permanent(error)) => {
            let to_domain = mail::recipient_domain(&message.to);
            tracing::error!(?job, to_domain, %error, "the email was refused");
            StatusCode::NO_CONTENT
        }
    }
}

#[derive(sqlx::FromRow)]
struct InvitationRow {
    email: String,
    display_name: String,
    grants: Json<Vec<Grant>>,
    status: String,
    expires_at: OffsetDateTime,
    tenant_name: String,
    inviter_name: Option<String>,
    token_hash: Vec<u8>,
}

#[derive(Deserialize)]
struct Grant {
    role: String,
    property_id: Option<Uuid>,
}

/// Invitation `id`'s email, or `None` when it is not `tenant`'s, is no longer pending, or no longer has `token`.
async fn invitation_message(
    svc: &JobsSvc,
    tenant: TenantId,
    id: Uuid,
    token: &str,
) -> Result<Option<Message>, sqlx::Error> {
    let mut tx = db::begin(&svc.pool, Scope::tenant(tenant)).await?;
    let row: Option<InvitationRow> = sqlx::query_as(
        "select email::text as email, display_name, grants, status, expires_at, tenant_name, inviter_name, token_hash
         from app.invitation_by_id($1)",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    // `invitation_by_id` finds only the transaction's tenant's invitations.
    let Some(row) = row else {
        tracing::warn!(invitation = %id, "no such invitation in the tenant; nothing sent");
        return Ok(None);
    };
    if Sha256::digest(token.as_bytes()).as_slice() != row.token_hash.as_slice() {
        tracing::info!(invitation = %id, "the invitation was resent with a new link since; nothing sent");
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
    let subject = INVITATION_SUBJECT.to_owned();
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
    Message { to: row.email.clone(), from_name: None, reply_to: None, subject, text, html, attachments: vec![] }
}

/// An invitation's subject. The team's and the inviter's names are free text anyone who signs up chooses, so they stay
/// in the body, never in the subject.
const INVITATION_SUBJECT: &str = "You are invited to join a team on GoodFolk";

/// A verification email's subject.
const VERIFICATION_SUBJECT: &str = "Confirm your email address for GoodFolk";

/// User `user`'s verification email, or `None` when the user is gone or already verified, or `token` is not their
/// newest unexpired link.
async fn verification_message(svc: &JobsSvc, user: Uuid, token: &str) -> Result<Option<Message>, sqlx::Error> {
    // `app_user` and `email_verification` are global, with no row-level security: no tenant scope is needed.
    let row: Option<(String, bool, Option<Vec<u8>>)> = sqlx::query_as(
        "select u.email::text, u.email_verified_at is not null,
                (select v.token_hash from email_verification v where v.user_id = u.id and v.expires_at > now()
                 order by v.created_at desc, v.id desc limit 1)
         from app_user u where u.id = $1",
    )
    .bind(user)
    .fetch_optional(&svc.pool)
    .await?;
    let Some((email, verified, newest)) = row else {
        tracing::warn!(%user, "no such user; nothing sent");
        return Ok(None);
    };
    if verified {
        tracing::info!(%user, "the email is already verified; nothing sent");
        return Ok(None);
    }
    if newest.is_none_or(|hash| Sha256::digest(token.as_bytes()).as_slice() != hash.as_slice()) {
        tracing::info!(%user, "a newer verification link was sent since, or this one expired; nothing sent");
        return Ok(None);
    }
    Ok(Some(render_verification(&email, &svc.app_base_url, token)))
}

/// The verification email. It names neither the person nor a team, which whoever signs up chooses: someone may sign
/// up with an address that is not theirs, and its owner then gets only this fixed text.
fn render_verification(email: &str, base_url: &str, token: &str) -> Message {
    let link = format!("{}/verify-email#{token}", base_url.trim_end_matches('/'));
    let text = format!(
        "Hello,\n\nSomeone created a GoodFolk account with this email address. If it was you, confirm the address \
         here:\n{link}\n\nThe link works for 24 hours. If it was not you, you can ignore this email.\n"
    );
    let html = format!(
        "<p>Hello,</p>\n<p>Someone created a GoodFolk account with this email address. If it was you, confirm the \
         address:</p>\n<p><a href=\"{0}\">Confirm my email address</a></p>\n<p>The link works for 24 hours. If it was \
         not you, you can ignore this email.</p>\n",
        escape(&link)
    );
    Message {
        to: email.to_owned(),
        from_name: None,
        reply_to: None,
        subject: VERIFICATION_SUBJECT.to_owned(),
        text,
        html,
        attachments: vec![],
    }
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

#[derive(sqlx::FromRow)]
struct ReportRow {
    kind: String,
    period_start: Date,
    period_end: Date,
    pdf_key: Option<String>,
    headline: Option<Json<Vec<Figure>>>,
    property_name: String,
    email: String,
    display_name: String,
}

/// One of a report's headline figures as its PDF prints it.
#[derive(Deserialize)]
struct Figure {
    label: String,
    value: String,
}

/// The figures a report's email carries, of the headline's.
const EMAILED_FIGURES: [&str; 4] = ["Occupancy", "ADR", "RevPAR", "Total revenue"];

/// Report `id`'s email to `user`, with its PDF attached, or `None` when the report is not `tenant`'s, is not weekly or
/// monthly, or `user` no longer holds an owner or manager grant for its property or for every property (a grant
/// revoked since the report was ready). `Err` (try again) when the PDF cannot be read.
async fn report_message(svc: &JobsSvc, tenant: TenantId, id: Uuid, user: Uuid) -> Result<Option<Message>, String> {
    let mut tx = db::begin(&svc.pool, Scope::tenant(tenant)).await.map_err(|e| e.to_string())?;
    let row: Option<ReportRow> = sqlx::query_as(
        "select r.kind, r.period_start, r.period_end, r.pdf_key, r.data->'data'->'headline' as headline,
                p.name as property_name, u.email::text as email, u.display_name
         from report r
         join property p on p.id = r.property_id
         join membership m on m.tenant_id = r.tenant_id and m.user_id = $2
         join app_user u on u.id = m.user_id
         where r.id = $1 and r.kind in ('weekly', 'monthly')
           and exists (select 1 from role_grant g
                       where g.tenant_id = r.tenant_id and g.user_id = $2 and g.role in ('owner', 'manager')
                         and (g.property_id is null or g.property_id = r.property_id))",
    )
    .bind(id)
    .bind(user)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    let Some(row) = row else {
        tracing::warn!(report = %id, %user, "no such report, or no owner or manager of its property; nothing sent");
        return Ok(None);
    };
    let key = row.pdf_key.as_deref().ok_or("the report has no PDF yet")?;
    let storage = svc.storage.as_ref().ok_or("no object storage to read the PDF from")?;
    let pdf = storage.get(Bucket::Private, key).await.map_err(|e| format!("reading the PDF: {e}"))?;
    Ok(Some(render_report(&row, pdf.to_vec())))
}

fn render_report(row: &ReportRow, pdf: Vec<u8>) -> Message {
    let (kind, period) = if row.kind == "monthly" {
        ("monthly", month_name(row.period_start))
    } else {
        ("weekly", date_range(row.period_start, row.period_end))
    };
    // A control character in the name would break the subject header; it reads as a space.
    let property: String = row.property_name.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let subject = format!("{property} {kind} report, {period}");
    let figures: Vec<(&str, &str)> = EMAILED_FIGURES
        .iter()
        .filter_map(|label| {
            let figures = row.headline.as_ref()?;
            let figure = figures.iter().find(|figure| figure.label == *label)?;
            Some((figure.label.as_str(), figure.value.as_str()))
        })
        .collect();

    let mut text =
        format!("Hello {},\n\nHere is {}'s {kind} report for {period}.\n\n", row.display_name, row.property_name);
    for (label, value) in &figures {
        text.push_str(&format!("{label}: {value}\n"));
    }
    text.push_str("\nThe full report is attached as a PDF. You can turn this email off on the Reports page.\n");

    let mut html = format!(
        "<p>Hello {},</p>\n<p>Here is {}'s {kind} report for {}.</p>\n<ul>\n",
        escape(&row.display_name),
        escape(&row.property_name),
        escape(&period)
    );
    for (label, value) in &figures {
        html.push_str(&format!("<li>{}: {}</li>\n", escape(label), escape(value)));
    }
    html.push_str(
        "</ul>\n<p>The full report is attached as a PDF. You can turn this email off on the Reports page.</p>\n",
    );
    let attachment = Attachment {
        filename: format!("{kind}-report-{}.pdf", row.period_start),
        content_type: "application/pdf".to_owned(),
        bytes: pdf,
    };
    Message {
        to: row.email.clone(),
        from_name: None,
        reply_to: None,
        subject,
        text,
        html,
        attachments: vec![attachment],
    }
}

/// Site booking `reservation`'s confirmation, or `None` when it is not `tenant`'s, its manage token is no longer
/// `token`, or its booker has no email.
async fn confirmation_message(
    svc: &JobsSvc,
    tenant: TenantId,
    reservation: Uuid,
    token: &str,
) -> Result<Option<Message>, String> {
    let mut tx = db::begin(&svc.pool, Scope::tenant(tenant)).await.map_err(|e| e.to_string())?;
    let confirmation = ibe::confirmation(&mut tx, reservation, token).await.map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    let Some(confirmation) = confirmation else {
        tracing::warn!(%reservation, "no such site booking, or its manage link has changed; nothing sent");
        return Ok(None);
    };
    let link = format!("{}/{}/manage?token={token}", svc.ibe_public_origin, confirmation.code);
    Ok(Some(render_confirmation(&confirmation, &link)))
}

fn render_confirmation(booking: &ibe::Confirmation, link: &str) -> Message {
    // A control character in the name would break the subject and sender headers; it reads as a space.
    let property: String = booking.property_name.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let subject = format!("Your booking at {property}: {}", booking.confirmation_no);
    let day = |date: Date| {
        date.format(format_description!("[weekday], [day padding:none] [month repr:long] [year]"))
            .unwrap_or_else(|_| date.to_string())
    };
    let rooms: Vec<String> = booking
        .rooms
        .iter()
        .map(|room| {
            let children = match room.children {
                0 => String::new(),
                1 => ", 1 child".to_owned(),
                n => format!(", {n} children"),
            };
            let adults = if room.adults == 1 { "1 adult".to_owned() } else { format!("{} adults", room.adults) };
            format!(
                "{}, {} ({}), {adults}{children}: {}",
                room.room_type,
                room.plan,
                meal_plan_words(room.meal_plan),
                ibe::money_text(&room.currency, room.total)
            )
        })
        .collect();
    let totals: Vec<String> =
        booking.totals.iter().map(|(currency, amount)| ibe::money_text(currency, *amount)).collect();
    let deposits: Vec<String> = booking
        .deposits
        .iter()
        .map(|deposit| format!("{} by {}", ibe::money_text(&deposit.currency, deposit.amount), day(deposit.due_date)))
        .collect();
    let contact: Vec<&str> =
        [booking.contact_phone.as_deref(), booking.contact_email.as_deref()].into_iter().flatten().collect();

    let mut text = format!(
        "Dear {},\n\nThank you for booking with {}. Your booking is confirmed, and you pay at the hotel.\n\n\
         Confirmation number: {}\nCheck-in: {}\nCheck-out: {}\n\n",
        booking.booker_name,
        booking.property_name,
        booking.confirmation_no,
        day(booking.check_in),
        day(booking.check_out)
    );
    for room in &rooms {
        text.push_str(&format!("- {room}\n"));
    }
    text.push_str(&format!(
        "\nTotal, taxes included: {}\n\nCancellation: {}\n",
        totals.join(" and "),
        booking.cancellation.join(" ")
    ));
    if !deposits.is_empty() {
        text.push_str(&format!(
            "\nDeposit (before taxes): {}. The hotel will contact you about the deposit.\n",
            deposits.join(" and ")
        ));
    }
    text.push_str(&format!("\nView or cancel your booking:\n{link}\n"));
    if !contact.is_empty() {
        text.push_str(&format!(
            "\nTo change your booking, contact {}: {}.\n",
            booking.property_name,
            contact.join(", ")
        ));
    }

    let mut html = format!(
        "<p>Dear {},</p>\n<p>Thank you for booking with {}. Your booking is confirmed, and you pay at the hotel.</p>\n\
         <p>Confirmation number: <strong>{}</strong><br>\nCheck-in: {}<br>\nCheck-out: {}</p>\n<ul>\n",
        escape(&booking.booker_name),
        escape(&booking.property_name),
        escape(&booking.confirmation_no),
        day(booking.check_in),
        day(booking.check_out)
    );
    for room in &rooms {
        html.push_str(&format!("<li>{}</li>\n", escape(room)));
    }
    html.push_str(&format!(
        "</ul>\n<p>Total, taxes included: <strong>{}</strong></p>\n<p>Cancellation: {}</p>\n",
        escape(&totals.join(" and ")),
        escape(&booking.cancellation.join(" "))
    ));
    if !deposits.is_empty() {
        html.push_str(&format!(
            "<p>Deposit (before taxes): {}. The hotel will contact you about the deposit.</p>\n",
            escape(&deposits.join(" and "))
        ));
    }
    html.push_str(&format!("<p><a href=\"{}\">View or cancel your booking</a></p>\n", escape(link)));
    if !contact.is_empty() {
        html.push_str(&format!(
            "<p>To change your booking, contact {}: {}.</p>\n",
            escape(&booking.property_name),
            escape(&contact.join(", "))
        ));
    }
    Message {
        to: booking.booker_email.clone(),
        from_name: Some(property),
        reply_to: booking.contact_email.clone(),
        subject,
        text,
        html,
        attachments: vec![],
    }
}

/// Site booking `reservation`'s cancellation, charging the `rooms` it closed, or `None` when it is not `tenant`'s, a
/// room of it is not cancelled, or its booker has no email.
async fn cancellation_message(
    svc: &JobsSvc,
    tenant: TenantId,
    reservation: Uuid,
    rooms: &[Uuid],
) -> Result<Option<Message>, String> {
    let mut tx = db::begin(&svc.pool, Scope::tenant(tenant)).await.map_err(|e| e.to_string())?;
    let cancellation = ibe::cancellation(&mut tx, reservation, rooms).await.map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    let Some(cancellation) = cancellation else {
        tracing::warn!(%reservation, "no cancelled site booking; nothing sent");
        return Ok(None);
    };
    Ok(Some(render_cancellation(&cancellation)))
}

fn render_cancellation(booking: &ibe::Cancellation) -> Message {
    let property: String = booking.property_name.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let subject = format!("Your booking at {property} is cancelled: {}", booking.confirmation_no);
    let day = |date: Date| {
        date.format(format_description!("[weekday], [day padding:none] [month repr:long] [year]"))
            .unwrap_or_else(|_| date.to_string())
    };
    let fee = if booking.penalty.is_empty() {
        "There is no cancellation charge.".to_owned()
    } else {
        let amounts: Vec<String> =
            booking.penalty.iter().map(|(currency, amount)| ibe::money_text(currency, *amount)).collect();
        format!("The cancellation charge under your booking terms is {}, payable to the hotel.", amounts.join(" and "))
    };
    let contact: Vec<&str> =
        [booking.contact_phone.as_deref(), booking.contact_email.as_deref()].into_iter().flatten().collect();

    let mut text = format!(
        "Dear {},\n\nYour booking with {} is cancelled.\n\nConfirmation number: {}\nCheck-in: {}\nCheck-out: {}\n\n{fee}\n",
        booking.booker_name,
        booking.property_name,
        booking.confirmation_no,
        day(booking.check_in),
        day(booking.check_out)
    );
    let mut html = format!(
        "<p>Dear {},</p>\n<p>Your booking with {} is cancelled.</p>\n\
         <p>Confirmation number: <strong>{}</strong><br>\nCheck-in: {}<br>\nCheck-out: {}</p>\n<p>{}</p>\n",
        escape(&booking.booker_name),
        escape(&booking.property_name),
        escape(&booking.confirmation_no),
        day(booking.check_in),
        day(booking.check_out),
        escape(&fee)
    );
    if !contact.is_empty() {
        text.push_str(&format!("\nFor any question, contact {}: {}.\n", booking.property_name, contact.join(", ")));
        html.push_str(&format!(
            "<p>For any question, contact {}: {}.</p>\n",
            escape(&booking.property_name),
            escape(&contact.join(", "))
        ));
    }
    Message {
        to: booking.booker_email.clone(),
        from_name: Some(property),
        reply_to: booking.contact_email.clone(),
        subject,
        text,
        html,
        attachments: vec![],
    }
}

/// A meal plan as the email names it.
fn meal_plan_words(meal_plan: rates::MealPlan) -> &'static str {
    match meal_plan {
        rates::MealPlan::Ro => "room only",
        rates::MealPlan::Bb => "bed and breakfast",
        rates::MealPlan::Hb => "half board",
        rates::MealPlan::Fb => "full board",
    }
}

/// A month as the monthly email names it: "October 2026".
fn month_name(date: Date) -> String {
    date.format(format_description!("[month repr:long] [year]")).unwrap_or_else(|_| date.to_string())
}

/// A week as the weekly email names it: "19–25 Oct 2026", "28 Sep – 4 Oct 2026" or "29 Dec 2025 – 4 Jan 2026".
fn date_range(from: Date, to: Date) -> String {
    let print = |date: Date, description: &[BorrowedFormatItem<'_>]| {
        date.format(description).unwrap_or_else(|_| date.to_string())
    };
    let end = print(to, format_description!("[day padding:none] [month repr:short] [year]"));
    if from.year() != to.year() {
        format!("{} – {end}", print(from, format_description!("[day padding:none] [month repr:short] [year]")))
    } else if from.month() != to.month() {
        format!("{} – {end}", print(from, format_description!("[day padding:none] [month repr:short]")))
    } else {
        format!("{}–{end}", from.day())
    }
}

#[cfg(test)]
mod tests {
    use super::{ReportRow, date_range, month_name, render_report};
    use time::macros::date;

    #[test]
    fn a_control_character_in_the_property_name_reads_as_a_space() {
        let row = ReportRow {
            kind: "weekly".into(),
            period_start: date!(2026 - 10 - 19),
            period_end: date!(2026 - 10 - 25),
            pdf_key: None,
            headline: None,
            property_name: "Hotel\r\nGAL\t".into(),
            email: "kamala@example.com".into(),
            display_name: "Kamala".into(),
        };

        let message = render_report(&row, b"%PDF-".to_vec());

        assert_eq!(message.subject, "Hotel  GAL  weekly report, 19–25 Oct 2026");
    }

    #[test]
    fn periods_print_as_the_subject_names_them() {
        assert_eq!(date_range(date!(2026 - 10 - 19), date!(2026 - 10 - 25)), "19–25 Oct 2026");
        assert_eq!(date_range(date!(2026 - 9 - 28), date!(2026 - 10 - 4)), "28 Sep – 4 Oct 2026");
        assert_eq!(date_range(date!(2025 - 12 - 29), date!(2026 - 1 - 4)), "29 Dec 2025 – 4 Jan 2026");
        assert_eq!(month_name(date!(2026 - 10 - 1)), "October 2026");
    }
}
