//! Email verification. A person who signs up is sent a link to confirm their address, and cannot invite staff until
//! they have followed it: invitations go out from the platform's own mail identity, so only someone proven to read
//! the address they signed up with may send them. Accepting an invitation proves the same, and everyone with an
//! account before verification existed counts as verified (migration 0049).
//!
//! The link's token is stored only as its SHA-256 (`email_verification`); it leaves this module only in the
//! `mail-send` outbox row, never in an error or a log line. Only the newest link of a user works, for
//! [`VERIFICATION_TTL`].

use crate::session::{new_token, token_hash};
use db::{Scope, Tx, UserId};
use sqlx::PgPool;
use time::Duration;
use uuid::Uuid;

/// How long a verification link works.
pub const VERIFICATION_TTL: Duration = Duration::hours(24);
/// The shortest wait between two verification emails to one person, in seconds.
pub const VERIFICATION_RESEND_SECS: i32 = 60;
/// Most verification emails one person may be sent within a day, the one sent at sign-up included.
pub const VERIFICATION_EMAILS_PER_DAY: i64 = 5;

#[derive(Debug, thiserror::Error)]
pub enum VerificationError {
    /// The same answer for an unknown, replaced or expired link, so tokens cannot be probed.
    #[error("This verification link is no longer valid")]
    Gone,
    #[error("your email address is already verified")]
    AlreadyVerified,
    /// Another email would come too soon after the last, or exceed the day's; retry after `retry_after` seconds.
    #[error("{message}")]
    TooMany { message: String, retry_after: u64 },
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Issues a new verification link for `user` and queues its email in `tx`. The token is carried only by the
/// `mail-send` outbox row.
pub(crate) async fn send_verification(tx: &mut Tx, user: UserId) -> Result<(), sqlx::Error> {
    let token = new_token();
    sqlx::query("insert into email_verification (id, user_id, token_hash, expires_at) values ($1, $2, $3, now() + $4)")
        .bind(Uuid::now_v7())
        .bind(user.0)
        .bind(token_hash(&token))
        .bind(VERIFICATION_TTL)
        .execute(&mut **tx)
        .await?;
    let job = mail::MailJob::EmailVerification { user_id: user.0, token };
    let payload = serde_json::to_value(&job).expect("a mail job always serializes");
    events::enqueue(tx, mail::SEND_TOPIC, &payload).await?;
    Ok(())
}

/// Sends `user` a new verification link; earlier links stop working. [`VerificationError::AlreadyVerified`] when
/// there is nothing to verify, and [`VerificationError::TooMany`] within [`VERIFICATION_RESEND_SECS`] of the last
/// email or once [`VERIFICATION_EMAILS_PER_DAY`] were sent in the last day.
pub async fn resend_verification(pool: &PgPool, user: UserId) -> Result<(), VerificationError> {
    let mut tx = db::begin(pool, Scope::user(user)).await?;
    // The user's row lock serializes two resends, so both cannot pass the limits.
    let verified: bool =
        sqlx::query_scalar("select email_verified_at is not null from app_user where id = $1 for update")
            .bind(user.0)
            .fetch_one(&mut *tx)
            .await?;
    if verified {
        return Err(VerificationError::AlreadyVerified);
    }
    let (sent, next_allowed, day_frees): (i64, Option<i32>, Option<i32>) = sqlx::query_as(
        "select count(*),
                ceil(extract(epoch from max(created_at) + make_interval(secs => $2) - now()))::int,
                ceil(extract(epoch from min(created_at) + interval '1 day' - now()))::int
         from email_verification where user_id = $1 and created_at > now() - interval '1 day'",
    )
    .bind(user.0)
    .bind(VERIFICATION_RESEND_SECS)
    .fetch_one(&mut *tx)
    .await?;
    let wait = |seconds: Option<i32>| u64::try_from(seconds.unwrap_or(1).max(1)).unwrap_or(1);
    if next_allowed.is_some_and(|seconds| seconds > 0) {
        return Err(VerificationError::TooMany {
            message: "a verification email was sent less than a minute ago; try again shortly".into(),
            retry_after: wait(next_allowed),
        });
    }
    if sent >= VERIFICATION_EMAILS_PER_DAY {
        return Err(VerificationError::TooMany {
            message: format!(
                "{VERIFICATION_EMAILS_PER_DAY} verification emails were sent in the last day; try again tomorrow"
            ),
            retry_after: wait(day_frees),
        });
    }
    send_verification(&mut tx, user).await?;
    tx.commit().await?;
    Ok(())
}

/// Verifies the email of the user whose newest, unexpired link carries `token`, and forgets their links.
/// [`VerificationError::Gone`] for any other token.
pub async fn verify_email(pool: &PgPool, token: &str) -> Result<(), VerificationError> {
    let mut tx = db::begin(pool, Scope::default()).await?;
    let user: Option<Uuid> = sqlx::query_scalar(
        "select v.user_id from email_verification v
         where v.token_hash = $1 and v.expires_at > now()
           and v.id = (select w.id from email_verification w where w.user_id = v.user_id
                       order by w.created_at desc, w.id desc limit 1)",
    )
    .bind(token_hash(token))
    .fetch_optional(&mut *tx)
    .await?;
    let Some(user) = user else { return Err(VerificationError::Gone) };
    mark_verified(&mut tx, UserId(user)).await?;
    tx.commit().await?;
    Ok(())
}

/// Records that `user` reads their email (they followed a link sent there), and forgets their verification links.
pub(crate) async fn mark_verified(tx: &mut Tx, user: UserId) -> Result<(), sqlx::Error> {
    sqlx::query("update app_user set email_verified_at = coalesce(email_verified_at, now()) where id = $1")
        .bind(user.0)
        .execute(&mut **tx)
        .await?;
    sqlx::query("delete from email_verification where user_id = $1").bind(user.0).execute(&mut **tx).await?;
    Ok(())
}

/// Whether `user` has verified their email: needed to invite staff and to turn a booking site on, both of which make
/// the platform send email.
pub async fn email_verified(tx: &mut Tx, user: UserId) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("select email_verified_at is not null from app_user where id = $1")
        .bind(user.0)
        .fetch_one(&mut **tx)
        .await
}
