//! A tenant's members: listing them, replacing a member's grants and revoking a membership, under the manager and
//! last-owner rules, plus the read-only permissions matrix.

use crate::invitations::{GrantSpec, StaffError, audit, lock_staff, validate_grants};
use crate::rbac::{ALL_PERMISSIONS, ALL_ROLES, Grant, Role, check_may_act_on, check_may_give, describe};
use db::{TenantId, Tx, UserId};
use serde::Serialize;
use sqlx::types::Json;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Member {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub grants: Vec<GrantSpec>,
    pub joined_at: OffsetDateTime,
    /// For `If-Match` on [`set_grants`]; bumps on every membership change.
    pub version: i32,
}

type MemberRow = (Uuid, String, String, Json<Vec<GrantSpec>>, OffsetDateTime, i32);

/// The members of `tx`'s tenant, or only `user`, by name. Grants are ordered by role, then property.
async fn fetch(tx: &mut Tx, user: Option<Uuid>) -> Result<Vec<Member>, sqlx::Error> {
    let rows: Vec<MemberRow> = sqlx::query_as(
        "select m.user_id, u.email::text, u.display_name,
                coalesce(jsonb_agg(jsonb_build_object('role', g.role, 'property_id', g.property_id)
                                   order by g.role, g.property_id) filter (where g.id is not null), '[]'),
                m.created_at, m.version
         from membership m
         join app_user u on u.id = m.user_id
         left join role_grant g on g.tenant_id = m.tenant_id and g.user_id = m.user_id
         where m.tenant_id = (select app.current_tenant()) and ($1::uuid is null or m.user_id = $1)
         group by m.user_id, u.email, u.display_name, m.created_at, m.version
         order by u.display_name, u.email",
    )
    .bind(user)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(user_id, email, display_name, Json(grants), joined_at, version)| Member {
            user_id,
            email,
            display_name,
            grants,
            joined_at,
            version,
        })
        .collect())
}

/// The tenant's members, by name.
pub async fn members(tx: &mut Tx) -> Result<Vec<Member>, StaffError> {
    Ok(fetch(tx, None).await?)
}

/// Serializes staff changes in the tenant, so two owners demoting each other cannot both succeed: the second sees
/// the first's change in the actor's grants it returns. Then locks `user`'s membership and returns its version and
/// current grants too.
async fn lock(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    user: Uuid,
) -> Result<(Vec<Grant>, i32, Vec<GrantSpec>), StaffError> {
    let actor_grants = lock_staff(tx, tenant, actor).await?;
    let version: i32 =
        sqlx::query_scalar("select version from membership where tenant_id = $1 and user_id = $2 for update")
            .bind(tenant.0)
            .bind(user)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(StaffError::NotFound("member"))?;
    let grants = fetch(tx, Some(user)).await?.pop().map(|m| m.grants).unwrap_or_default();
    Ok((actor_grants, version, grants))
}

fn as_grants(specs: &[GrantSpec]) -> Vec<Grant> {
    specs.iter().copied().map(Grant::from).collect()
}

fn holds_owner(grants: &[GrantSpec]) -> bool {
    grants.iter().any(|g| g.role == Role::Owner)
}

/// Whether `user` is the tenant's only owner.
async fn is_last_owner(tx: &mut Tx, user: Uuid) -> Result<bool, sqlx::Error> {
    let others: bool =
        sqlx::query_scalar("select exists (select 1 from role_grant where role = 'owner' and user_id <> $1)")
            .bind(user)
            .fetch_one(&mut **tx)
            .await?;
    Ok(!others)
}

/// Replaces `user`'s grants with `grants` if the membership is still at `expected_version`, and bumps the version.
/// Nobody changes their own grants; the manager rules apply to both the current grants and the new ones; the owner
/// role is never added, and the tenant's last owner keeps it.
pub async fn set_grants(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    user: Uuid,
    expected_version: i32,
    grants: &[GrantSpec],
) -> Result<Member, StaffError> {
    let (actor_grants, version, previous) = lock(tx, tenant, actor, user).await?;
    let actor_grants = &actor_grants[..];
    if version != expected_version {
        return Err(StaffError::VersionMismatch);
    }
    if user == actor.0 {
        return Err(StaffError::Forbidden("you cannot change your own grants".into()));
    }
    validate_grants(tx, grants).await?;
    check_may_act_on(actor_grants, &as_grants(&previous)).map_err(StaffError::Forbidden)?;
    check_may_act_on(actor_grants, &as_grants(grants)).map_err(StaffError::Forbidden)?;
    // A grant the member already holds is kept, not given, so an owner's grants can be edited around their `owner`.
    let added: Vec<Grant> = grants.iter().filter(|g| !previous.contains(g)).copied().map(Grant::from).collect();
    check_may_give(actor_grants, &added).map_err(StaffError::Forbidden)?;
    if holds_owner(&previous) && !holds_owner(grants) && is_last_owner(tx, user).await? {
        return Err(StaffError::Conflict("the tenant's last owner keeps the owner role".into()));
    }

    sqlx::query("delete from role_grant where tenant_id = $1 and user_id = $2")
        .bind(tenant.0)
        .bind(user)
        .execute(&mut **tx)
        .await?;
    for grant in grants {
        sqlx::query("insert into role_grant (id, tenant_id, user_id, property_id, role) values ($1, $2, $3, $4, $5)")
            .bind(Uuid::now_v7())
            .bind(tenant.0)
            .bind(user)
            .bind(grant.property_id)
            .bind(grant.role.as_str())
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query("update membership set version = version + 1 where tenant_id = $1 and user_id = $2")
        .bind(tenant.0)
        .bind(user)
        .execute(&mut **tx)
        .await?;
    let member = fetch(tx, Some(user)).await?.pop().ok_or(StaffError::NotFound("member"))?;
    let data = serde_json::json!({ "email": member.email, "previous": previous, "grants": member.grants });
    audit(tx, tenant, actor, "membership.grants_changed", ("membership", user), data).await?;
    Ok(member)
}

/// Deletes the membership (grants cascade) and the user's sessions whose active_tenant_id is this tenant.
/// Nobody revokes themself, the manager rules apply to the member's grants, and the last owner is never revoked.
pub async fn revoke_member(tx: &mut Tx, tenant: TenantId, actor: UserId, user: Uuid) -> Result<(), StaffError> {
    let (actor_grants, _, grants) = lock(tx, tenant, actor, user).await?;
    let actor_grants = &actor_grants[..];
    if user == actor.0 {
        return Err(StaffError::Forbidden("you cannot remove yourself".into()));
    }
    check_may_act_on(actor_grants, &as_grants(&grants)).map_err(StaffError::Forbidden)?;
    if holds_owner(&grants) && is_last_owner(tx, user).await? {
        return Err(StaffError::Conflict("the tenant's last owner cannot be removed".into()));
    }
    let email: String =
        sqlx::query_scalar("select email::text from app_user where id = $1").bind(user).fetch_one(&mut **tx).await?;

    // `session` has no row-level security; the app role deletes from it directly, as sign-out does.
    sqlx::query("delete from session where user_id = $1 and active_tenant_id = $2")
        .bind(user)
        .bind(tenant.0)
        .execute(&mut **tx)
        .await?;
    sqlx::query("delete from membership where tenant_id = $1 and user_id = $2")
        .bind(tenant.0)
        .bind(user)
        .execute(&mut **tx)
        .await?;
    let data = serde_json::json!({ "email": email, "grants": grants });
    audit(tx, tenant, actor, "membership.revoked", ("membership", user), data).await?;
    Ok(())
}

/// One permission of the matrix: its name, what it allows, and the roles that hold it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct MatrixRow {
    pub permission: &'static str,
    pub allows: &'static str,
    pub roles: Vec<Role>,
}

/// Every permission, in declaration order, with the roles that hold it, generated from the role table so it cannot
/// drift. Nothing in it is tenant-specific.
pub fn permission_matrix() -> Vec<MatrixRow> {
    ALL_PERMISSIONS
        .into_iter()
        .map(|permission| MatrixRow {
            permission: permission.name(),
            allows: describe(permission),
            roles: ALL_ROLES.into_iter().filter(|role| role.permits(permission)).collect(),
        })
        .collect()
}
