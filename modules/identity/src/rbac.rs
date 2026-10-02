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

    /// Anything not listed is denied, so a new permission is granted only where it is added.
    fn permits(self, permission: Permission) -> bool {
        use Permission::*;
        match self {
            Role::Owner => true,
            Role::Manager => matches!(
                permission,
                PropertiesView
                    | PropertiesManage
                    | RoomsView
                    | RoomsManage
                    | InventoryView
                    | InventoryBlock
                    | RatesView
                    | RatesManage
                    | ReservationsView
                    | ReservationsManage
                    | FrontDeskCheckIn
            ),
            Role::FrontDesk => matches!(
                permission,
                PropertiesView
                    | RoomsView
                    | InventoryView
                    | InventoryBlock
                    | RatesView
                    | ReservationsView
                    | ReservationsManage
                    | FrontDeskCheckIn
            ),
            Role::Housekeeping | Role::Accountant => {
                matches!(permission, PropertiesView | RoomsView | InventoryView | RatesView | ReservationsView)
            }
        }
    }
}

/// Permissions are added here as each phase introduces new actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    PropertiesView,
    PropertiesCreate,
    /// Change a property's settings (name, check-in and check-out times).
    PropertiesManage,
    /// See room types, rooms, sections and block reasons.
    RoomsView,
    /// Create, change, deactivate and reorder room types, rooms, sections and block reasons.
    RoomsManage,
    /// See inventory counts and room blocks.
    InventoryView,
    /// Block rooms and release or shorten blocks.
    InventoryBlock,
    /// See rate plans, prices, restrictions, meal supplements and cancellation policies, and quote stays.
    RatesView,
    /// Create and change rate plans, prices, restrictions, meal supplements and cancellation policies.
    RatesManage,
    /// See reservations, what is free to sell, and guests.
    ReservationsView,
    /// Create guests and reservations, change guests, cancel reservation rooms, and assign and unassign rooms.
    ReservationsManage,
    /// Check a reservation room in, undo a same-day check-in, and check it out.
    FrontDeskCheckIn,
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
    Ok(grants_from_rows(user, rows))
}

/// The grants in `rows` (`property_id`, `role`), skipping roles this build does not know.
pub(crate) fn grants_from_rows(user: UserId, rows: Vec<(Option<Uuid>, String)>) -> Vec<Grant> {
    rows.into_iter()
        .filter_map(|(property_id, role)| match Role::parse(&role) {
            Some(role) => Some(Grant { property_id, role }),
            None => {
                tracing::warn!(role, user = %user.0, "skipping a role grant with a role this build does not know");
                None
            }
        })
        .collect()
}
