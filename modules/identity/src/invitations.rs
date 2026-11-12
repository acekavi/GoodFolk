//! Staff invitations: an owner or manager invites an email to the tenant with a set of grants, and the person accepts
//! through a single-use link. The link's token is stored only as its SHA-256; it leaves this module only in the
//! `mail-send` outbox row, never in an audit row, an error or a log line.

use crate::account::{hash_in_background, insert_user, password_is_valid};
use crate::rbac::{Grant, Role, check_may_give, load_grants};
use crate::session::{SessionInfo, new_token, token_hash};
use crate::verification::{email_verified, mark_verified};
use db::{Event, Scope, TenantId, Tx, UserId};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use sqlx::types::Json;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// How long an invitation's link works.
pub const INVITATION_TTL: Duration = Duration::days(7);

/// Most grants one invitation may give, or one member may hold.
const MAX_GRANTS: usize = 50;

/// Most invitation emails (new invitations and resends together) one tenant may send within a day.
pub const INVITATION_EMAILS_PER_TENANT_PER_DAY: i64 = 20;
/// Most invitation emails one person may send within an hour, in one tenant.
pub const INVITATION_EMAILS_PER_ACTOR_PER_HOUR: i64 = 10;
/// The shortest wait between two emails of one invitation, in seconds.
pub const INVITATION_RESEND_SECS: i64 = 60;

/// Cache key for a tenant's members and invitations.
pub fn staff_key(tenant: TenantId) -> String {
    format!("staff:{}", tenant.0)
}

/// A grant to give: a role, tenant-wide (`property_id: None`) or for one property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GrantSpec {
    pub role: Role,
    pub property_id: Option<Uuid>,
}

impl From<GrantSpec> for Grant {
    fn from(spec: GrantSpec) -> Self {
        Grant { property_id: spec.property_id, role: spec.role }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum InvitationStatus {
    Pending,
    Accepted,
    Revoked,
    Expired,
}

impl InvitationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Revoked => "revoked",
            Self::Expired => "expired",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "pending" => Self::Pending,
            "accepted" => Self::Accepted,
            "revoked" => Self::Revoked,
            _ => Self::Expired,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Invitation {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub grants: Vec<GrantSpec>,
    pub status: InvitationStatus,
    pub invited_by: Option<Uuid>,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}

/// What the invitation page shows before accepting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub tenant_name: String,
    pub inviter: Option<String>,
    pub email: String,
    /// Each role with its property's name, or `None` for all properties.
    pub grants: Vec<(Role, Option<String>)>,
    /// Whether an account with the email exists, so the page offers "Sign in to accept" instead of a password.
    pub existing_account: bool,
}

/// Who accepts an invitation.
pub enum Acceptor {
    /// Nobody has the invitation's email yet: create the account with this password.
    NewUser { password: String },
    /// The signed-in session, whose user's email must be the invitation's. It is switched to the tenant on accepting.
    SignedIn(SessionInfo),
}

#[derive(Debug, thiserror::Error)]
pub enum StaffError {
    #[error("{0} not found")]
    NotFound(&'static str),
    /// The same answer for an unknown, used, expired or revoked token, so tokens cannot be probed.
    #[error("This invitation is no longer valid")]
    Gone,
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Invalid(String),
    #[error("the member was changed by someone else; reload and try again")]
    VersionMismatch,
    /// The actor has not verified their email address, so they may not send invitations.
    #[error("Verify your email address before inviting staff")]
    Unverified,
    /// Sending would exceed a limit on invitation emails; retry after `retry_after` seconds.
    #[error("{message}")]
    TooMany { message: String, retry_after: u64 },
    #[error(transparent)]
    Busy(#[from] crate::Busy),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

type InvitationRow = (Uuid, String, String, Json<Vec<GrantSpec>>, String, Option<Uuid>, OffsetDateTime, OffsetDateTime);

/// `$head`, an invitation's columns (in [`InvitationRow`]'s order), then `$rest`.
macro_rules! with_columns {
    ($head:literal, $rest:literal) => {
        concat!($head, " id, email::text, display_name, grants, status, invited_by, created_at, expires_at ", $rest)
    };
}

/// A pending invitation past its expiry reads as expired, whether or not the row has been marked yet.
fn from_row(row: InvitationRow) -> Invitation {
    let (id, email, display_name, Json(grants), status, invited_by, created_at, expires_at) = row;
    let mut status = InvitationStatus::parse(&status);
    if status == InvitationStatus::Pending && expires_at <= OffsetDateTime::now_utc() {
        status = InvitationStatus::Expired;
    }
    Invitation { id, email, display_name, grants, status, invited_by, created_at, expires_at }
}

/// Marks the pending invitations past their expiry (in `tx`'s tenant) `expired`, optionally for one email only. It
/// writes, so its callers (the GraphQL `invitations` read among them) run it in a read-write transaction: the
/// statuses it stores are what the status filter and the one-pending-per-email index see.
async fn expire_lapsed(tx: &mut Tx, email: Option<&str>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update invitation set status = 'expired'
         where status = 'pending' and expires_at <= now() and ($1::citext is null or email = $1::citext)",
    )
    .bind(email)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// The lowercased email, if it looks like an address: one `@`, a local part, and a dotted domain.
fn normalize_email(email: &str) -> Result<String, StaffError> {
    let email = email.trim().to_lowercase();
    let valid = email.len() <= 254
        && !email.chars().any(|c| c.is_whitespace() || c.is_control())
        && email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && local.len() <= 64
                && !domain.contains('@')
                && domain.split('.').count() >= 2
                && domain.split('.').all(|label| !label.is_empty())
        });
    if valid { Ok(email) } else { Err(StaffError::Invalid("the email is not a valid address".into())) }
}

/// Checks the grants' shape and that their properties are the tenant's; the manager rules are checked separately.
pub(crate) async fn validate_grants(tx: &mut Tx, grants: &[GrantSpec]) -> Result<(), StaffError> {
    if grants.is_empty() || grants.len() > MAX_GRANTS {
        return Err(StaffError::Invalid(format!("give 1 to {MAX_GRANTS} grants")));
    }
    if grants.iter().any(|g| g.role == Role::Owner && g.property_id.is_some()) {
        return Err(StaffError::Invalid("the owner role is always tenant-wide".into()));
    }
    for (index, grant) in grants.iter().enumerate() {
        if grants[..index].contains(grant) {
            return Err(StaffError::Invalid(format!("the {} grant is listed twice", grant.role.as_str())));
        }
    }
    let properties: Vec<Uuid> = grants.iter().filter_map(|g| g.property_id).collect();
    let known: Vec<Uuid> =
        sqlx::query_scalar("select id from property where id = any($1)").bind(&properties).fetch_all(&mut **tx).await?;
    match properties.iter().find(|p| !known.contains(p)) {
        Some(property) => Err(StaffError::Invalid(format!("property {property} is not in this tenant"))),
        None => Ok(()),
    }
}

/// Serializes staff changes in the tenant (invitations and membership changes alike), then returns `actor`'s grants
/// as they are now: each change authorizes against these, so one committed while it waited, such as the actor's own
/// demotion, is honoured.
pub(crate) async fn lock_staff(tx: &mut Tx, tenant: TenantId, actor: UserId) -> Result<Vec<Grant>, sqlx::Error> {
    sqlx::query("select pg_advisory_xact_lock(hashtextextended('staff:' || $1::text, 0))")
        .bind(tenant.0)
        .execute(&mut **tx)
        .await?;
    load_grants(tx, actor).await
}

fn check_rules(actor_grants: &[Grant], grants: &[GrantSpec]) -> Result<(), StaffError> {
    let given: Vec<Grant> = grants.iter().copied().map(Grant::from).collect();
    check_may_give(actor_grants, &given).map_err(StaffError::Forbidden)
}

/// Refuses an actor who has not verified their email address: invitations go out from the platform's own mail
/// identity, so only someone proven to read their address may send them.
async fn require_verified(tx: &mut Tx, actor: UserId) -> Result<(), StaffError> {
    if email_verified(tx, actor).await? { Ok(()) } else { Err(StaffError::Unverified) }
}

/// Refuses another invitation email once the tenant sent [`INVITATION_EMAILS_PER_TENANT_PER_DAY`] within a day, or
/// `actor` sent [`INVITATION_EMAILS_PER_ACTOR_PER_HOUR`] within an hour. Counted from the tenant's audit rows of
/// created and resent invitations, under the staff lock ([`lock_staff`]), so concurrent sends cannot pass together.
async fn check_send_limits(tx: &mut Tx, actor: UserId) -> Result<(), StaffError> {
    let (tenant_sent, tenant_frees, actor_sent, actor_frees): (i64, Option<i32>, i64, Option<i32>) = sqlx::query_as(
        "select count(*),
                ceil(extract(epoch from min(at) + interval '1 day' - now()))::int,
                count(*) filter (where actor_user_id = $1 and at > now() - interval '1 hour'),
                ceil(extract(epoch from min(at) filter (where actor_user_id = $1 and at > now() - interval '1 hour')
                                        + interval '1 hour' - now()))::int
         from audit_log
         where action in ('invitation.created', 'invitation.resent') and at > now() - interval '1 day'",
    )
    .bind(actor.0)
    .fetch_one(&mut **tx)
    .await?;
    let wait = |seconds: Option<i32>| u64::try_from(seconds.unwrap_or(1).max(1)).unwrap_or(1);
    if tenant_sent >= INVITATION_EMAILS_PER_TENANT_PER_DAY {
        return Err(StaffError::TooMany {
            message: format!(
                "this team sent {INVITATION_EMAILS_PER_TENANT_PER_DAY} invitations in the last day; try again later"
            ),
            retry_after: wait(tenant_frees),
        });
    }
    if actor_sent >= INVITATION_EMAILS_PER_ACTOR_PER_HOUR {
        return Err(StaffError::TooMany {
            message: format!(
                "you sent {INVITATION_EMAILS_PER_ACTOR_PER_HOUR} invitations in the last hour; try again later"
            ),
            retry_after: wait(actor_frees),
        });
    }
    Ok(())
}

/// Queues the invitation email. The token is carried only here.
async fn enqueue_mail(tx: &mut Tx, tenant: TenantId, invitation: Uuid, token: String) -> Result<(), StaffError> {
    let job = mail::MailJob::Invitation { invitation_id: invitation, tenant_id: tenant.0, token };
    let payload = serde_json::to_value(&job).expect("a mail job always serializes");
    events::enqueue(tx, mail::SEND_TOPIC, &payload).await?;
    Ok(())
}

/// Audits `action` on `entity` with `data` and notifies the tenant's staff key.
pub(crate) async fn audit(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    action: &str,
    entity: (&str, Uuid),
    data: serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into audit_log (id, tenant_id, actor_user_id, action, entity, entity_id, data)
         values ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(actor.0)
    .bind(action)
    .bind(entity.0)
    .bind(entity.1)
    .bind(data)
    .execute(&mut **tx)
    .await?;
    db::notify(tx, &Event { tenant_id: tenant, property_id: None, keys: vec![staff_key(tenant)] }).await
}

/// Audits `action` on `invitation` (never with its token) and notifies the tenant's staff key.
async fn record(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    action: &str,
    invitation: &Invitation,
) -> Result<(), sqlx::Error> {
    let data = serde_json::json!({
        "email": invitation.email,
        "display_name": invitation.display_name,
        "grants": invitation.grants,
    });
    audit(tx, tenant, actor, action, ("invitation", invitation.id), data).await
}

/// Invites `email` to the tenant with `grants`, which `actor` must be allowed to give (the manager rules).
/// Writes the `mail-send` outbox row in `tx`.
pub async fn invite(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    email: &str,
    display_name: &str,
    grants: &[GrantSpec],
) -> Result<Invitation, StaffError> {
    let email = normalize_email(email)?;
    let display_name = display_name.trim();
    if !(1..=200).contains(&display_name.chars().count()) {
        return Err(StaffError::Invalid("the display name is 1 to 200 characters".into()));
    }
    if grants.iter().any(|g| g.role == Role::Owner) {
        return Err(StaffError::Invalid("the owner role cannot be given by invitation".into()));
    }
    let actor_grants = lock_staff(tx, tenant, actor).await?;
    require_verified(tx, actor).await?;
    validate_grants(tx, grants).await?;
    check_rules(&actor_grants, grants)?;
    let member: bool = sqlx::query_scalar(
        "select exists (select 1 from membership m join app_user u on u.id = m.user_id
                        where m.tenant_id = $2 and u.email = $1::citext)",
    )
    .bind(&email)
    .bind(tenant.0)
    .fetch_one(&mut **tx)
    .await?;
    if member {
        return Err(StaffError::Conflict(format!("{email} is already a member")));
    }
    expire_lapsed(tx, Some(&email)).await?;
    check_send_limits(tx, actor).await?;

    let token = new_token();
    let id = Uuid::now_v7();
    let inserted = sqlx::query_as::<_, InvitationRow>(with_columns!(
        "insert into invitation (id, tenant_id, email, display_name, grants, token_hash, status, invited_by, expires_at)
         values ($1, $2, $3, $4, $5, $6, 'pending', $7, now() + $8) returning",
        ""
    ))
    .bind(id)
    .bind(tenant.0)
    .bind(&email)
    .bind(display_name)
    .bind(Json(grants))
    .bind(token_hash(&token))
    .bind(actor.0)
    .bind(INVITATION_TTL)
    .fetch_one(&mut **tx)
    .await;
    let invitation = match inserted {
        Ok(row) => from_row(row),
        Err(err) if crate::violates(&err, "invitation_one_pending_idx") => {
            return Err(StaffError::Conflict(format!("{email} already has a pending invitation")));
        }
        Err(err) => return Err(err.into()),
    };
    enqueue_mail(tx, tenant, id, token).await?;
    record(tx, tenant, actor, "invitation.created", &invitation).await?;
    Ok(invitation)
}

/// The invitation `id`, locked for the rest of `tx`.
async fn lock(tx: &mut Tx, id: Uuid) -> Result<Invitation, StaffError> {
    sqlx::query_as::<_, InvitationRow>(with_columns!("select", "from invitation where id = $1 for update"))
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .map(from_row)
        .ok_or(StaffError::NotFound("invitation"))
}

/// Issues a new token for a pending or expired invitation, moves its expiry to [`INVITATION_TTL`] from now and sends
/// it again. The old link stops working. At most once every [`INVITATION_RESEND_SECS`], within the tenant's and the
/// actor's limits.
pub async fn resend(tx: &mut Tx, tenant: TenantId, actor: UserId, id: Uuid) -> Result<Invitation, StaffError> {
    let actor_grants = lock_staff(tx, tenant, actor).await?;
    require_verified(tx, actor).await?;
    let current = lock(tx, id).await?;
    check_rules(&actor_grants, &current.grants)?;
    if !matches!(current.status, InvitationStatus::Pending | InvitationStatus::Expired) {
        return Err(StaffError::Conflict(format!("the invitation is {}", current.status.as_str())));
    }
    // Every email of an invitation sets its expiry to INVITATION_TTL from then, so the last one went at this time.
    let last_sent = current.expires_at - INVITATION_TTL;
    let next_allowed = last_sent + Duration::seconds(INVITATION_RESEND_SECS) - OffsetDateTime::now_utc();
    if next_allowed.is_positive() {
        return Err(StaffError::TooMany {
            message: "this invitation was sent less than a minute ago; try again shortly".into(),
            retry_after: u64::try_from(next_allowed.whole_seconds().max(0) + 1).unwrap_or(1),
        });
    }
    check_send_limits(tx, actor).await?;
    let token = new_token();
    let updated = sqlx::query_as::<_, InvitationRow>(with_columns!(
        "update invitation set token_hash = $2, expires_at = now() + $3, status = 'pending' where id = $1 returning",
        ""
    ))
    .bind(id)
    .bind(token_hash(&token))
    .bind(INVITATION_TTL)
    .fetch_one(&mut **tx)
    .await;
    let invitation = match updated {
        Ok(row) => from_row(row),
        Err(err) if crate::violates(&err, "invitation_one_pending_idx") => {
            return Err(StaffError::Conflict(format!("{} already has a pending invitation", current.email)));
        }
        Err(err) => return Err(err.into()),
    };
    enqueue_mail(tx, tenant, id, token).await?;
    record(tx, tenant, actor, "invitation.resent", &invitation).await?;
    Ok(invitation)
}

/// Revokes a pending invitation, so its link answers [`StaffError::Gone`].
pub async fn revoke_invitation(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    id: Uuid,
) -> Result<Invitation, StaffError> {
    let actor_grants = lock_staff(tx, tenant, actor).await?;
    let current = lock(tx, id).await?;
    check_rules(&actor_grants, &current.grants)?;
    if current.status != InvitationStatus::Pending {
        return Err(StaffError::Conflict(format!("the invitation is {}", current.status.as_str())));
    }
    let row = sqlx::query_as::<_, InvitationRow>(with_columns!(
        "update invitation set status = 'revoked' where id = $1 returning",
        ""
    ))
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;
    let invitation = from_row(row);
    record(tx, tenant, actor, "invitation.revoked", &invitation).await?;
    Ok(invitation)
}

/// The tenant's invitations, newest first, optionally of one status. Marks lapsed ones `expired` first, so it writes.
pub async fn list_invitations(tx: &mut Tx, status: Option<InvitationStatus>) -> Result<Vec<Invitation>, StaffError> {
    expire_lapsed(tx, None).await?;
    let rows = sqlx::query_as::<_, InvitationRow>(with_columns!(
        "select",
        "from invitation where $1::text is null or status = $1 order by created_at desc, id"
    ))
    .bind(status.map(InvitationStatus::as_str))
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(from_row).collect())
}

/// A pending, unexpired invitation found by its token.
struct Live {
    id: Uuid,
    tenant: TenantId,
    email: String,
    display_name: String,
    grants: Vec<GrantSpec>,
    tenant_name: String,
    inviter: Option<String>,
}

type TokenRow = (Uuid, Uuid, String, String, Json<Vec<GrantSpec>>, String, OffsetDateTime, String, Option<String>);

/// Every token is hashed and looked up, valid or not; anything but a live invitation is [`StaffError::Gone`].
async fn find_live(pool: &PgPool, token: &str) -> Result<Live, StaffError> {
    let row: Option<TokenRow> = sqlx::query_as(
        "select id, tenant_id, email::text, display_name, grants, status, expires_at, tenant_name, inviter_name
         from app.invitation_by_token($1)",
    )
    .bind(token_hash(token))
    .fetch_optional(pool)
    .await?;
    match row {
        Some((id, tenant, email, display_name, Json(grants), status, expires_at, tenant_name, inviter))
            if status == "pending" && expires_at > OffsetDateTime::now_utc() =>
        {
            Ok(Live { id, tenant: TenantId(tenant), email, display_name, grants, tenant_name, inviter })
        }
        _ => Err(StaffError::Gone),
    }
}

async fn account_exists(pool: &PgPool, email: &str) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar("select id from app_user where email = $1::citext").bind(email).fetch_optional(pool).await
}

/// What the invitation page shows. [`StaffError::Gone`] for anything but a live invitation.
pub async fn preview(pool: &PgPool, token: &str) -> Result<Preview, StaffError> {
    let live = find_live(pool, token).await?;
    let properties: Vec<Uuid> = live.grants.iter().filter_map(|g| g.property_id).collect();
    let mut tx = db::begin(pool, Scope::tenant(live.tenant)).await?;
    let names: Vec<(Uuid, String)> = sqlx::query_as("select id, name from property where id = any($1)")
        .bind(&properties)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    // A granted property deleted since the invitation was sent: accepting would answer Gone, so the preview does too.
    let mut grants = Vec::with_capacity(live.grants.len());
    for grant in &live.grants {
        let name = match grant.property_id {
            None => None,
            Some(p) => Some(names.iter().find(|(id, _)| *id == p).map(|(_, n)| n.clone()).ok_or(StaffError::Gone)?),
        };
        grants.push((grant.role, name));
    }
    Ok(Preview {
        existing_account: account_exists(pool, &live.email).await?.is_some(),
        tenant_name: live.tenant_name,
        inviter: live.inviter,
        email: live.email,
        grants,
    })
}

/// Accepts the invitation: creates the account for a [`Acceptor::NewUser`], then the membership (version 1) and the
/// invited grants, marks the invitation accepted, switches a [`Acceptor::SignedIn`] session to the tenant and audits
/// `invitation.accepted`, in one transaction. A token works
/// once, however many accepts race for it. Returns the user and the tenant they joined.
pub async fn accept(pool: &PgPool, token: &str, acceptor: Acceptor) -> Result<(UserId, TenantId), StaffError> {
    let live = find_live(pool, token).await?;
    let sign_in = || StaffError::Conflict(format!("Sign in as {} to accept", live.email));
    let (user, new_account, session) = match acceptor {
        Acceptor::NewUser { password } => {
            if !password_is_valid(&password) {
                return Err(StaffError::Invalid("the password is 12 to 128 characters".into()));
            }
            // An existing account is found by `insert_user` after the liveness re-check below, so a token used by a
            // racing accept is always Gone, never "Sign in as …".
            (UserId(Uuid::now_v7()), Some(hash_in_background(password).await?), None)
        }
        Acceptor::SignedIn(session) => {
            let user = session.user;
            let email: Option<String> = sqlx::query_scalar("select email::text from app_user where id = $1")
                .bind(user.0)
                .fetch_optional(pool)
                .await?;
            if !email.is_some_and(|email| email.eq_ignore_ascii_case(&live.email)) {
                return Err(StaffError::Forbidden(format!("this invitation is for {}", live.email)));
            }
            (user, None, Some(session.id))
        }
    };

    let tenant = live.tenant;
    let mut tx = db::begin(pool, Scope { tenant: Some(tenant), user: Some(user) }).await?;
    // A concurrent accept waits here, then sees the invitation no longer pending.
    let still_live: Option<Uuid> = sqlx::query_scalar(
        "select id from invitation where id = $1 and status = 'pending' and expires_at > now() for update",
    )
    .bind(live.id)
    .fetch_optional(&mut *tx)
    .await?;
    if still_live.is_none() {
        return Err(StaffError::Gone);
    }
    match &new_account {
        Some(hash) => {
            if !insert_user(&mut tx, user, &live.email, hash, &live.display_name).await? {
                return Err(sign_in());
            }
        }
        None => {
            let member: bool =
                sqlx::query_scalar("select exists (select 1 from membership where tenant_id = $1 and user_id = $2)")
                    .bind(tenant.0)
                    .bind(user.0)
                    .fetch_one(&mut *tx)
                    .await?;
            if member {
                return Err(StaffError::Conflict("you are already a member of this tenant".into()));
            }
        }
    }
    sqlx::query("insert into membership (tenant_id, user_id, version) values ($1, $2, 1)")
        .bind(tenant.0)
        .bind(user.0)
        .execute(&mut *tx)
        .await?;
    for grant in &live.grants {
        let inserted = sqlx::query(
            "insert into role_grant (id, tenant_id, user_id, property_id, role) values ($1, $2, $3, $4, $5)",
        )
        .bind(Uuid::now_v7())
        .bind(tenant.0)
        .bind(user.0)
        .bind(grant.property_id)
        .bind(grant.role.as_str())
        .execute(&mut *tx)
        .await;
        match inserted {
            Ok(_) => {}
            // The property was deleted after the invitation was sent, so it can no longer be honoured as issued.
            Err(err) if crate::violates(&err, "role_grant_property_id_fkey") => return Err(StaffError::Gone),
            Err(err) => return Err(err.into()),
        }
    }
    let accepted = sqlx::query(
        "update invitation set status = 'accepted', accepted_at = now(), accepted_user_id = $2
         where id = $1 and status = 'pending' and expires_at > now()",
    )
    .bind(live.id)
    .bind(user.0)
    .execute(&mut *tx)
    .await?;
    if accepted.rows_affected() != 1 {
        return Err(StaffError::Gone);
    }
    // The invitation reached its email, so following it proves the address is theirs.
    mark_verified(&mut tx, user).await?;
    if let Some(session) = session {
        // `session` has no row-level security; the app role updates it directly, as switching tenants does.
        sqlx::query("update session set active_tenant_id = $1 where id = $2")
            .bind(tenant.0)
            .bind(session)
            .execute(&mut *tx)
            .await?;
    }
    let data = serde_json::json!({ "email": live.email, "grants": live.grants, "new_account": new_account.is_some() });
    audit(&mut tx, tenant, user, "invitation.accepted", ("invitation", live.id), data).await?;
    tx.commit().await?;
    Ok((user, tenant))
}
