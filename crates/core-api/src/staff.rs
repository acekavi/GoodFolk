//! `core-api add-staff`: an interim way to add a staff user with a role, until Phase 8 brings invitations.
//!
//! Runs as the schema owner, which row-level security still binds, so every write happens in a
//! transaction scoped to the owner's tenant.

use db::{Scope, TenantId, UserId};
use garde::Validate;
use identity::Role;
use sqlx::PgPool;
use std::fmt;
use uuid::Uuid;

/// What to add. The password is never shown: there is no `Debug`.
#[derive(Validate)]
pub struct AddStaff {
    #[garde(email)]
    pub owner_email: String,
    /// Names the tenant when the owner owns several.
    #[garde(skip)]
    pub tenant: Option<String>,
    #[garde(email)]
    pub email: String,
    #[garde(length(chars, min = 12, max = 128))]
    pub password: String,
    #[garde(length(chars, min = 1, max = 200))]
    pub display_name: String,
    #[garde(skip)]
    pub role: Role,
    /// A property code; the grant is tenant-wide when absent.
    #[garde(skip)]
    pub property: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum AddStaffError {
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("`{0}` is not an owner of a tenant")]
    NotOwner(String),
    #[error("`{0}` owns more than one tenant ({1}); name one with --tenant")]
    AmbiguousTenant(String, String),
    #[error("no property with code `{0}` in this tenant")]
    PropertyNotFound(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// What the command did, and what was already there.
#[derive(Debug, PartialEq, Eq)]
pub struct Outcome {
    pub tenant: String,
    pub user_created: bool,
    pub membership_created: bool,
    pub grant_created: bool,
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let say = |created: bool| if created { "created" } else { "already existed" };
        write!(
            f,
            "tenant `{}`: user {}, membership {}, role grant {}",
            self.tenant,
            say(self.user_created),
            say(self.membership_created),
            say(self.grant_created)
        )
    }
}

/// Parses the arguments after `add-staff`, reading the password from `--password-file`. An unknown
/// option is named without its value, so a mistyped `--password=...` is not echoed.
pub fn parse_args(args: &[String]) -> Result<AddStaff, AddStaffError> {
    let (mut owner_email, mut tenant, mut email, mut password_file) = (None, None, None, None);
    let (mut display_name, mut role, mut property) = (None, None, None);
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let slot = match flag.as_str() {
            "--owner-email" => &mut owner_email,
            "--tenant" => &mut tenant,
            "--email" => &mut email,
            "--password-file" => &mut password_file,
            "--display-name" => &mut display_name,
            "--role" => &mut role,
            "--property" => &mut property,
            other => {
                let name = other.split('=').next().unwrap_or_default();
                return Err(AddStaffError::Invalid(format!("unknown option `{name}`")));
            }
        };
        let value = args.next().ok_or_else(|| AddStaffError::Invalid(format!("{flag} needs a value")))?;
        *slot = Some(value.clone());
    }
    let required =
        |value: Option<String>, flag: &str| value.ok_or_else(|| AddStaffError::Invalid(format!("{flag} is required")));
    let role = required(role, "--role")?;
    let role = Role::parse(&role).ok_or_else(|| {
        AddStaffError::Invalid(format!(
            "unknown role `{role}`; expected owner, manager, front_desk, housekeeping, accountant or hk_supervisor"
        ))
    })?;
    let password_file = required(password_file, "--password-file")?;
    let password = std::fs::read_to_string(&password_file)
        .map_err(|err| AddStaffError::Invalid(format!("cannot read the password file `{password_file}`: {err}")))?;
    Ok(AddStaff {
        owner_email: required(owner_email, "--owner-email")?,
        tenant,
        email: required(email, "--email")?,
        // One trailing line break, as `echo` or an editor leaves; nothing else is trimmed.
        password: password
            .strip_suffix('\n')
            .map(|p| p.strip_suffix('\r').unwrap_or(p))
            .unwrap_or(&password)
            .to_owned(),
        display_name: required(display_name, "--display-name")?,
        role,
        property,
    })
}

/// Adds the user, their membership and the grant, creating only what is missing.
pub async fn add_staff(pool: &PgPool, input: AddStaff) -> Result<Outcome, AddStaffError> {
    input.validate().map_err(|report| AddStaffError::Invalid(report.to_string()))?;
    let (owner, tenant, tenant_name) = find_owned_tenant(pool, &input).await?;

    let existing: Option<Uuid> = sqlx::query_scalar("select id from app_user where email = $1::citext")
        .bind(&input.email)
        .fetch_optional(pool)
        .await?;
    // Hashed only for a new user, and with the same function as sign-up.
    let new_account = match existing {
        Some(_) => None,
        None => {
            let password = input.password.clone();
            let hash = tokio::task::spawn_blocking(move || identity::hash_password(&password))
                .await
                .expect("hashing task does not panic");
            Some((UserId(Uuid::now_v7()), hash))
        }
    };

    let mut tx = db::begin(pool, Scope { tenant: Some(tenant), user: Some(owner) }).await?;
    let property = match &input.property {
        None => None,
        Some(code) => Some(
            sqlx::query_scalar::<_, Uuid>("select id from property where tenant_id = $1 and code = $2")
                .bind(tenant.0)
                .bind(code)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| AddStaffError::PropertyNotFound(code.clone()))?,
        ),
    };
    let mut user_created = false;
    if let Some((id, hash)) = &new_account {
        user_created = sqlx::query(
            "insert into app_user (id, email, password_hash, display_name) values ($1, $2, $3, $4)
             on conflict (email) do nothing",
        )
        .bind(id.0)
        .bind(&input.email)
        .bind(hash)
        .bind(&input.display_name)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
    }
    // Also covers a user created by someone else since the lookup above.
    let user: Uuid = sqlx::query_scalar("select id from app_user where email = $1::citext")
        .bind(&input.email)
        .fetch_one(&mut *tx)
        .await?;
    let membership_created = sqlx::query(
        "insert into membership (tenant_id, user_id) values ($1, $2) on conflict (tenant_id, user_id) do nothing",
    )
    .bind(tenant.0)
    .bind(user)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    let grant = Uuid::now_v7();
    let grant_created = sqlx::query(
        "insert into role_grant (id, tenant_id, user_id, property_id, role) values ($1, $2, $3, $4, $5)
         on conflict (tenant_id, user_id, property_id, role) do nothing",
    )
    .bind(grant)
    .bind(tenant.0)
    .bind(user)
    .bind(property)
    .bind(input.role.as_str())
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if grant_created {
        sqlx::query(
            "insert into audit_log (id, tenant_id, actor_user_id, action, entity, entity_id, data)
             values ($1, $2, $3, 'role.granted', 'role_grant', $4, $5)",
        )
        .bind(Uuid::now_v7())
        .bind(tenant.0)
        .bind(owner.0)
        .bind(grant)
        .bind(serde_json::json!({"user_id": user, "role": input.role.as_str(), "property_id": property}))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    tracing::info!(tenant = %tenant.0, user = %user, role = input.role.as_str(), user_created, membership_created, grant_created, "staff added");
    Ok(Outcome { tenant: tenant_name, user_created, membership_created, grant_created })
}

/// The one tenant the owner holds a tenant-wide owner grant in (the one named by `--tenant`, if given).
async fn find_owned_tenant(pool: &PgPool, input: &AddStaff) -> Result<(UserId, TenantId, String), AddStaffError> {
    let not_owner = || AddStaffError::NotOwner(input.owner_email.clone());
    let owner: Uuid = sqlx::query_scalar("select id from app_user where email = $1::citext")
        .bind(&input.owner_email)
        .fetch_optional(pool)
        .await?
        .ok_or_else(not_owner)?;
    let owner = UserId(owner);

    let mut tx = db::begin(pool, Scope::user(owner)).await?;
    let memberships: Vec<(Uuid, String)> = sqlx::query_as(
        "select t.id, t.name from membership m join tenant t on t.id = m.tenant_id
         where m.user_id = $1 order by t.name, t.id",
    )
    .bind(owner.0)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let mut owned = Vec::new();
    for (id, name) in memberships {
        if input.tenant.as_ref().is_some_and(|wanted| *wanted != name) {
            continue;
        }
        let mut tx = db::begin(pool, Scope { tenant: Some(TenantId(id)), user: Some(owner) }).await?;
        let is_owner: bool = sqlx::query_scalar(
            "select exists (select 1 from role_grant
                            where tenant_id = $1 and user_id = $2 and role = 'owner' and property_id is null)",
        )
        .bind(id)
        .bind(owner.0)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        if is_owner {
            owned.push((TenantId(id), name));
        }
    }
    match owned.len() {
        0 => Err(not_owner()),
        1 => {
            let (tenant, name) = owned.remove(0);
            Ok((owner, tenant, name))
        }
        _ => {
            let names: Vec<String> = owned.iter().map(|(_, name)| format!("`{name}`")).collect();
            Err(AddStaffError::AmbiguousTenant(input.owner_email.clone(), names.join(", ")))
        }
    }
}
