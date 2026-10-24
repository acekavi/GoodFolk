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
    HkSupervisor,
    Pos,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Manager => "manager",
            Role::FrontDesk => "front_desk",
            Role::Housekeeping => "housekeeping",
            Role::Accountant => "accountant",
            Role::HkSupervisor => "hk_supervisor",
            Role::Pos => "pos",
        }
    }

    pub fn parse(value: &str) -> Option<Role> {
        ALL_ROLES.into_iter().find(|role| role.as_str() == value)
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
                    | HousekeepingView
                    | HousekeepingUpdate
                    | HousekeepingSupervise
                    | RoomIssuesReport
                    | MediaManage
                    | GuestDocumentsManage
                    | FolioView
                    | FolioPost
                    | FolioAdjust
                    | InvoiceIssue
                    | FxOverride
                    | PosPost
                    | NightAudit
                    | LinenView
                    | LinenMove
                    | LinenManage
                    | LaundryOrders
                    | LaundryPricesManage
                    | StaffManage
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
                    | HousekeepingView
                    | RoomIssuesReport
                    | GuestDocumentsManage
                    | FolioView
                    | FolioPost
                    | InvoiceIssue
                    | NightAudit
                    | LaundryOrders
            ),
            Role::Housekeeping => matches!(
                permission,
                PropertiesView
                    | RoomsView
                    | InventoryView
                    | RatesView
                    | ReservationsView
                    | HousekeepingView
                    | HousekeepingUpdate
                    | RoomIssuesReport
                    | LinenView
                    | LinenMove
                    | LaundryOrders
            ),
            Role::HkSupervisor => matches!(
                permission,
                PropertiesView
                    | RoomsView
                    | InventoryView
                    | RatesView
                    | ReservationsView
                    | HousekeepingView
                    | HousekeepingUpdate
                    | HousekeepingSupervise
                    | RoomIssuesReport
                    | LinenView
                    | LinenMove
                    | LinenManage
                    | LaundryOrders
            ),
            Role::Pos => matches!(permission, PosPost),
            Role::Accountant => {
                matches!(
                    permission,
                    PropertiesView
                        | RoomsView
                        | InventoryView
                        | RatesView
                        | ReservationsView
                        | FolioView
                        | InvoiceIssue
                )
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
    /// See room conditions, the housekeeping board, assignments and room issues.
    HousekeepingView,
    /// Move a room through its cleaning steps and resolve room issues.
    HousekeepingUpdate,
    /// Assign rooms, inspect, set any condition, and override a check-in on a room that is not ready.
    HousekeepingSupervise,
    /// Report a problem with a room, without changing its condition.
    RoomIssuesReport,
    /// Upload, reorder, caption and remove room type gallery photos.
    MediaManage,
    /// Upload, view (through an audited signed link) and delete guests' ID documents.
    GuestDocumentsManage,
    /// Folios, lines, payments, invoices.
    FolioView,
    /// Charges, payments.
    FolioPost,
    /// Reversals, transfers, refunds, routing, credit notes, posting tonight's charges.
    FolioAdjust,
    /// Issue invoices, invoice links.
    InvoiceIssue,
    /// Tax rules, outlets, property tax settings.
    TaxSettingsManage,
    /// Manual exchange rates.
    FxOverride,
    /// POS bills.
    PosPost,
    /// The night audit; see [`describe`].
    NightAudit,
    /// See linen. See [`describe`].
    LinenView,
    /// Move linen. See [`describe`].
    LinenMove,
    /// Manage linen. See [`describe`].
    LinenManage,
    /// Guests' laundry orders. See [`describe`].
    LaundryOrders,
    /// The guest laundry price list. See [`describe`].
    LaundryPricesManage,
    /// Inviting staff, changing their roles and removing them; see [`describe`].
    StaffManage,
}

/// Every permission, in declaration order.
pub const ALL_PERMISSIONS: [Permission; 32] = [
    Permission::PropertiesView,
    Permission::PropertiesCreate,
    Permission::PropertiesManage,
    Permission::RoomsView,
    Permission::RoomsManage,
    Permission::InventoryView,
    Permission::InventoryBlock,
    Permission::RatesView,
    Permission::RatesManage,
    Permission::ReservationsView,
    Permission::ReservationsManage,
    Permission::FrontDeskCheckIn,
    Permission::HousekeepingView,
    Permission::HousekeepingUpdate,
    Permission::HousekeepingSupervise,
    Permission::RoomIssuesReport,
    Permission::MediaManage,
    Permission::GuestDocumentsManage,
    Permission::FolioView,
    Permission::FolioPost,
    Permission::FolioAdjust,
    Permission::InvoiceIssue,
    Permission::TaxSettingsManage,
    Permission::FxOverride,
    Permission::PosPost,
    Permission::NightAudit,
    Permission::LinenView,
    Permission::LinenMove,
    Permission::LinenManage,
    Permission::LaundryOrders,
    Permission::LaundryPricesManage,
    Permission::StaffManage,
];

/// Every role.
pub const ALL_ROLES: [Role; 7] =
    [Role::Owner, Role::Manager, Role::FrontDesk, Role::Housekeeping, Role::Accountant, Role::HkSupervisor, Role::Pos];

/// The roles a manager may grant and act on; owners and managers are not junior.
pub const JUNIOR_ROLES: [Role; 5] =
    [Role::FrontDesk, Role::Housekeeping, Role::HkSupervisor, Role::Accountant, Role::Pos];

/// What `permission` allows, in plain words, for the permission matrix.
pub fn describe(permission: Permission) -> &'static str {
    use Permission::*;
    match permission {
        PropertiesView => "",
        PropertiesCreate => "",
        PropertiesManage => "Change a property's settings (name, check-in and check-out times).",
        RoomsView => "See room types, rooms, sections and block reasons.",
        RoomsManage => "Create, change, deactivate and reorder room types, rooms, sections and block reasons.",
        InventoryView => "See inventory counts and room blocks.",
        InventoryBlock => "Block rooms and release or shorten blocks.",
        RatesView => {
            "See rate plans, prices, restrictions, meal supplements and cancellation policies, and quote stays."
        }
        RatesManage => {
            "Create and change rate plans, prices, restrictions, meal supplements and cancellation policies."
        }
        ReservationsView => "See reservations, what is free to sell, and guests.",
        ReservationsManage => {
            "Create guests and reservations, change guests, cancel reservation rooms, and assign and unassign rooms."
        }
        FrontDeskCheckIn => "Check a reservation room in, undo a same-day check-in, and check it out.",
        HousekeepingView => "See room conditions, the housekeeping board, assignments and room issues.",
        HousekeepingUpdate => "Move a room through its cleaning steps and resolve room issues.",
        HousekeepingSupervise => {
            "Assign rooms, inspect, set any condition, and override a check-in on a room that is not ready."
        }
        RoomIssuesReport => "Report a problem with a room, without changing its condition.",
        MediaManage => "Upload, reorder, caption and remove room type gallery photos.",
        GuestDocumentsManage => "Upload, view (through an audited signed link) and delete guests' ID documents.",
        FolioView => "Folios, lines, payments, invoices.",
        FolioPost => "Charges, payments.",
        FolioAdjust => "Reversals, transfers, refunds, routing, credit notes, posting tonight's charges.",
        InvoiceIssue => "Issue invoices, invoice links.",
        TaxSettingsManage => "Tax rules, outlets, property tax settings.",
        FxOverride => "Manual exchange rates.",
        PosPost => "POS bills.",
        NightAudit => "the night-audit review, starting and retrying the close, and the audit history.",
        LinenView => "seeing linen items, locations, par levels, stock, the movement ledger and laundry batches.",
        LinenMove => "receiving, transferring, sending and returning linen (laundry batches out and back).",
        LinenManage => "linen items, locations, par levels and write-offs.",
        LaundryOrders => {
            "seeing guests' laundry orders, collecting one from a stay, moving it along, delivering and cancelling it."
        }
        LaundryPricesManage => "creating and changing the guest laundry price list.",
        StaffManage => {
            "Inviting staff, changing their roles and removing them; managers only for junior roles at their properties."
        }
    }
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
