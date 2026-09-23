use db::{Tx, UserId};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Manager,
    FrontDesk,
    Housekeeping,
    Accountant,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Manager => "manager",
            Role::FrontDesk => "front_desk",
            Role::Housekeeping => "housekeeping",
            Role::Accountant => "accountant",
        }
    }

    pub fn parse(value: &str) -> Option<Role> {
        [Role::Owner, Role::Manager, Role::FrontDesk, Role::Housekeeping, Role::Accountant]
            .into_iter()
            .find(|role| role.as_str() == value)
    }

    fn permits(self, permission: Permission) -> bool {
        use Permission::*;
        match self {
            Role::Owner => true,
            Role::Manager | Role::FrontDesk | Role::Housekeeping | Role::Accountant => {
                matches!(permission, PropertiesView)
            }
        }
    }
}

/// Permissions are added here as each phase introduces new actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    PropertiesView,
    PropertiesCreate,
}

/// A role held tenant-wide (`property_id: None`) or for one property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Grant {
    pub property_id: Option<Uuid>,
    pub role: Role,
}

/// `property: None` asks for a tenant-wide permission, which only tenant-wide grants give.
pub fn allows(grants: &[Grant], permission: Permission, property: Option<Uuid>) -> bool {
    grants.iter().any(|grant| {
        let in_scope = grant.property_id.is_none() || grant.property_id == property;
        in_scope && grant.role.permits(permission)
    })
}

/// Loads `user`'s grants in the tenant that `tx` is scoped to.
pub async fn load_grants(tx: &mut Tx, user: UserId) -> Result<Vec<Grant>, sqlx::Error> {
    let rows: Vec<(Option<Uuid>, String)> =
        sqlx::query_as("select property_id, role from role_grant where user_id = $1")
            .bind(user.0)
            .fetch_all(&mut **tx)
            .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(property_id, role)| Role::parse(&role).map(|role| Grant { property_id, role }))
        .collect())
}
