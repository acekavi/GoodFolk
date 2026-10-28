use db::{Tx, UserId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
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
    pub(crate) fn permits(self, permission: Permission) -> bool {
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
                    | ReportsView
                    | IbeManage
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
                        | ReportsView
                        | ReportsLiability
                )
            }
        }
    }
}

/// Permissions are added here as each phase introduces new actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// See [`describe`].
    PropertiesView,
    /// See [`describe`].
    PropertiesCreate,
    /// See [`describe`].
    PropertiesManage,
    /// See [`describe`].
    RoomsView,
    /// See [`describe`].
    RoomsManage,
    /// See [`describe`].
    InventoryView,
    /// See [`describe`].
    InventoryBlock,
    /// See [`describe`].
    RatesView,
    /// See [`describe`].
    RatesManage,
    /// See [`describe`].
    ReservationsView,
    /// See [`describe`].
    ReservationsManage,
    /// See [`describe`].
    FrontDeskCheckIn,
    /// See [`describe`].
    HousekeepingView,
    /// See [`describe`].
    HousekeepingUpdate,
    /// See [`describe`].
    HousekeepingSupervise,
    /// See [`describe`].
    RoomIssuesReport,
    /// See [`describe`].
    MediaManage,
    /// See [`describe`].
    GuestDocumentsManage,
    /// See [`describe`].
    FolioView,
    /// See [`describe`].
    FolioPost,
    /// See [`describe`].
    FolioAdjust,
    /// See [`describe`].
    InvoiceIssue,
    /// See [`describe`].
    TaxSettingsManage,
    /// See [`describe`].
    FxOverride,
    /// See [`describe`].
    PosPost,
    /// See [`describe`].
    NightAudit,
    /// See [`describe`].
    LinenView,
    /// See [`describe`].
    LinenMove,
    /// See [`describe`].
    LinenManage,
    /// See [`describe`].
    LaundryOrders,
    /// See [`describe`].
    LaundryPricesManage,
    /// See [`describe`].
    StaffManage,
    /// See [`describe`].
    ReportsView,
    /// See [`describe`].
    ReportsLiability,
    /// See [`describe`].
    IbeManage,
}

impl Permission {
    /// The permission's name as the permission matrix shows it, such as `StaffManage`.
    pub fn name(self) -> &'static str {
        use Permission::*;
        match self {
            PropertiesView => "PropertiesView",
            PropertiesCreate => "PropertiesCreate",
            PropertiesManage => "PropertiesManage",
            RoomsView => "RoomsView",
            RoomsManage => "RoomsManage",
            InventoryView => "InventoryView",
            InventoryBlock => "InventoryBlock",
            RatesView => "RatesView",
            RatesManage => "RatesManage",
            ReservationsView => "ReservationsView",
            ReservationsManage => "ReservationsManage",
            FrontDeskCheckIn => "FrontDeskCheckIn",
            HousekeepingView => "HousekeepingView",
            HousekeepingUpdate => "HousekeepingUpdate",
            HousekeepingSupervise => "HousekeepingSupervise",
            RoomIssuesReport => "RoomIssuesReport",
            MediaManage => "MediaManage",
            GuestDocumentsManage => "GuestDocumentsManage",
            FolioView => "FolioView",
            FolioPost => "FolioPost",
            FolioAdjust => "FolioAdjust",
            InvoiceIssue => "InvoiceIssue",
            TaxSettingsManage => "TaxSettingsManage",
            FxOverride => "FxOverride",
            PosPost => "PosPost",
            NightAudit => "NightAudit",
            LinenView => "LinenView",
            LinenMove => "LinenMove",
            LinenManage => "LinenManage",
            LaundryOrders => "LaundryOrders",
            LaundryPricesManage => "LaundryPricesManage",
            StaffManage => "StaffManage",
            ReportsView => "ReportsView",
            ReportsLiability => "ReportsLiability",
            IbeManage => "IbeManage",
        }
    }
}

/// Every permission, in declaration order.
pub const ALL_PERMISSIONS: [Permission; 35] = [
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
    Permission::ReportsView,
    Permission::ReportsLiability,
    Permission::IbeManage,
];

/// Every role.
pub const ALL_ROLES: [Role; 7] =
    [Role::Owner, Role::Manager, Role::FrontDesk, Role::Housekeeping, Role::Accountant, Role::HkSupervisor, Role::Pos];

/// The roles a manager may grant and act on; owners and managers are not junior.
pub const JUNIOR_ROLES: [Role; 5] =
    [Role::FrontDesk, Role::Housekeeping, Role::HkSupervisor, Role::Accountant, Role::Pos];

/// What `permission` allows, as a sentence for the permission matrix.
pub fn describe(permission: Permission) -> &'static str {
    use Permission::*;
    match permission {
        PropertiesView => "See properties and their settings.",
        PropertiesCreate => "Create properties.",
        PropertiesManage => {
            "Change a property's settings, such as its name, check-in and check-out times and audit time, and see its \
             audit and drift ops alerts."
        }
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
        FolioView => "See folios, lines, payments and invoices.",
        FolioPost => "Post charges and payments to folios.",
        FolioAdjust => {
            "Reverse, transfer, refund and route folio lines, issue credit notes, and post tonight's charges."
        }
        InvoiceIssue => "Issue invoices and open their links.",
        TaxSettingsManage => "Change tax rules, outlets and a property's tax settings.",
        FxOverride => "Set manual exchange rates.",
        PosPost => "Post POS bills.",
        NightAudit => "Review the night audit, start and retry the close, and see the audit history.",
        LinenView => "See linen items, locations, par levels, stock, the movement ledger and laundry batches.",
        LinenMove => "Receive, transfer, send and return linen (laundry batches out and back).",
        LinenManage => "Change linen items, locations and par levels, and write linen off.",
        LaundryOrders => {
            "See guests' laundry orders, collect one from a stay, move it along, deliver it and cancel it."
        }
        LaundryPricesManage => "Create and change the guest laundry price list.",
        StaffManage => {
            "Invite staff, change their roles and remove them; managers only for junior roles at their properties."
        }
        ReportsView => "See the weekly and monthly reports and the service charge report, and run an on-demand report.",
        ReportsLiability => "See and download the TDL, SSCL and VAT reports.",
        IbeManage => "Change a property's booking site settings: turn it on, brand it, and set its terms.",
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

/// The manager rules for giving `given`: nobody grants `owner`; owners give any other role; a manager gives only
/// junior roles, at properties where they hold `manager`, or anywhere (and tenant-wide) with a tenant-wide `manager`
/// grant. A refusal names the role or property.
pub fn check_may_give(actor: &[Grant], given: &[Grant]) -> Result<(), String> {
    let tenant_wide_manager = actor.iter().any(|g| g.role == Role::Manager && g.property_id.is_none());
    for grant in given {
        let role = grant.role.as_str();
        if grant.role == Role::Owner {
            return Err("nobody can grant the owner role".into());
        }
        if is_owner(actor) {
            continue;
        }
        if !JUNIOR_ROLES.contains(&grant.role) {
            return Err(format!("only owners can grant the {role} role"));
        }
        match grant.property_id {
            None if !tenant_wide_manager => {
                return Err(format!("a tenant-wide {role} grant needs a tenant-wide manager"));
            }
            Some(property)
                if !tenant_wide_manager
                    && !actor.iter().any(|g| g.role == Role::Manager && g.property_id == Some(property)) =>
            {
                return Err(format!("you do not manage property {property}"));
            }
            _ => {}
        }
    }
    Ok(())
}

/// The manager rules for acting on a member holding `member`: owners act on anyone; a manager only on members whose
/// every grant is a junior role at a property they manage (any property, with a tenant-wide `manager` grant). A
/// member's tenant-wide grant needs a tenant-wide manager. A refusal names the role or property.
pub fn check_may_act_on(actor: &[Grant], member: &[Grant]) -> Result<(), String> {
    if is_owner(actor) {
        return Ok(());
    }
    if !actor.iter().any(|g| g.role == Role::Manager) {
        return Err("only owners and managers manage staff".into());
    }
    if let Some(grant) = member.iter().find(|g| !JUNIOR_ROLES.contains(&g.role)) {
        return Err(format!("only owners can manage a member with the {} role", grant.role.as_str()));
    }
    let tenant_wide_manager = actor.iter().any(|g| g.role == Role::Manager && g.property_id.is_none());
    if tenant_wide_manager {
        return Ok(());
    }
    for grant in member {
        match grant.property_id {
            None => {
                return Err(format!(
                    "a member with a tenant-wide {} grant needs a tenant-wide manager",
                    grant.role.as_str()
                ));
            }
            Some(property) if !actor.iter().any(|g| g.role == Role::Manager && g.property_id == Some(property)) => {
                return Err(format!("you do not manage property {property}"));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// Owners are always tenant-wide.
fn is_owner(grants: &[Grant]) -> bool {
    grants.iter().any(|g| g.role == Role::Owner && g.property_id.is_none())
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
