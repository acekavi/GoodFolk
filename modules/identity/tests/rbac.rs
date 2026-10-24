use identity::{Grant, Permission, Role, allows};
use uuid::Uuid;

const HOTEL: Uuid = Uuid::from_u128(1);
const OTHER_HOTEL: Uuid = Uuid::from_u128(2);

#[test]
fn an_owner_may_do_anything_anywhere() {
    let grants = [Grant { property_id: None, role: Role::Owner }];

    assert!(allows(&grants, Permission::PropertiesCreate, None));
    assert!(allows(&grants, Permission::PropertiesView, Some(HOTEL)));
}

#[test]
fn a_property_grant_applies_only_to_that_property() {
    let grants = [Grant { property_id: Some(HOTEL), role: Role::FrontDesk }];

    assert!(allows(&grants, Permission::PropertiesView, Some(HOTEL)));
    assert!(!allows(&grants, Permission::PropertiesView, Some(OTHER_HOTEL)));
    assert!(!allows(&grants, Permission::PropertiesView, None));
}

#[test]
fn only_owners_create_properties() {
    let manager = [Grant { property_id: None, role: Role::Manager }];

    assert!(!allows(&manager, Permission::PropertiesCreate, None));
}

#[test]
fn roles_round_trip_through_their_database_names() {
    for role in [Role::Owner, Role::Manager, Role::FrontDesk, Role::Housekeeping, Role::Accountant, Role::HkSupervisor]
    {
        assert_eq!(Role::parse(role.as_str()), Some(role));
    }
    assert_eq!(Role::parse("root"), None);
}

#[test]
fn each_role_has_exactly_its_permissions() {
    use Permission::*;
    let all = [
        PropertiesView,
        PropertiesCreate,
        PropertiesManage,
        RoomsView,
        RoomsManage,
        InventoryView,
        InventoryBlock,
        RatesView,
        RatesManage,
        ReservationsView,
        ReservationsManage,
        FrontDeskCheckIn,
        HousekeepingView,
        HousekeepingUpdate,
        HousekeepingSupervise,
        RoomIssuesReport,
        MediaManage,
        GuestDocumentsManage,
        FolioView,
        FolioPost,
        FolioAdjust,
        InvoiceIssue,
        TaxSettingsManage,
        FxOverride,
        PosPost,
        NightAudit,
        LinenView,
        LinenMove,
        LinenManage,
        LaundryOrders,
        LaundryPricesManage,
    ];
    let expected: [(Role, &[Permission]); 6] = [
        (Role::Owner, &all),
        (
            Role::Manager,
            &[
                PropertiesView,
                PropertiesManage,
                RoomsView,
                RoomsManage,
                InventoryView,
                InventoryBlock,
                RatesView,
                RatesManage,
                ReservationsView,
                ReservationsManage,
                FrontDeskCheckIn,
                HousekeepingView,
                HousekeepingUpdate,
                HousekeepingSupervise,
                RoomIssuesReport,
                MediaManage,
                GuestDocumentsManage,
                FolioView,
                FolioPost,
                FolioAdjust,
                InvoiceIssue,
                FxOverride,
                PosPost,
                NightAudit,
                LinenView,
                LinenMove,
                LinenManage,
                LaundryOrders,
                LaundryPricesManage,
            ],
        ),
        (
            Role::FrontDesk,
            &[
                PropertiesView,
                RoomsView,
                InventoryView,
                InventoryBlock,
                RatesView,
                ReservationsView,
                ReservationsManage,
                FrontDeskCheckIn,
                HousekeepingView,
                RoomIssuesReport,
                GuestDocumentsManage,
                FolioView,
                FolioPost,
                InvoiceIssue,
                NightAudit,
                LaundryOrders,
            ],
        ),
        (
            Role::Housekeeping,
            &[
                PropertiesView,
                RoomsView,
                InventoryView,
                RatesView,
                ReservationsView,
                HousekeepingView,
                HousekeepingUpdate,
                RoomIssuesReport,
                LinenView,
                LinenMove,
                LaundryOrders,
            ],
        ),
        (
            Role::Accountant,
            &[PropertiesView, RoomsView, InventoryView, RatesView, ReservationsView, FolioView, InvoiceIssue],
        ),
        (
            Role::HkSupervisor,
            &[
                PropertiesView,
                RoomsView,
                InventoryView,
                RatesView,
                ReservationsView,
                HousekeepingView,
                HousekeepingUpdate,
                HousekeepingSupervise,
                RoomIssuesReport,
                LinenView,
                LinenMove,
                LinenManage,
                LaundryOrders,
            ],
        ),
    ];

    for (role, permitted) in expected {
        let grants = [Grant { property_id: Some(HOTEL), role }];
        for permission in all {
            assert_eq!(
                allows(&grants, permission, Some(HOTEL)),
                permitted.contains(&permission),
                "{role:?} and {permission:?}"
            );
        }
    }
}

fn roles_allowed(permission: Permission) -> Vec<Role> {
    [Role::Owner, Role::Manager, Role::FrontDesk, Role::Housekeeping, Role::Accountant, Role::HkSupervisor]
        .into_iter()
        .filter(|role| allows(&[Grant { property_id: None, role: *role }], permission, Some(HOTEL)))
        .collect()
}

#[test]
fn housekeeping_view_is_for_the_front_desk_and_every_housekeeping_role() {
    assert_eq!(
        roles_allowed(Permission::HousekeepingView),
        [Role::Owner, Role::Manager, Role::FrontDesk, Role::Housekeeping, Role::HkSupervisor]
    );
}

#[test]
fn housekeeping_update_excludes_the_front_desk() {
    assert_eq!(
        roles_allowed(Permission::HousekeepingUpdate),
        [Role::Owner, Role::Manager, Role::Housekeeping, Role::HkSupervisor]
    );
}

#[test]
fn only_supervisors_supervise_housekeeping() {
    assert_eq!(roles_allowed(Permission::HousekeepingSupervise), [Role::Owner, Role::Manager, Role::HkSupervisor]);
}

#[test]
fn the_front_desk_may_report_a_room_issue() {
    assert_eq!(
        roles_allowed(Permission::RoomIssuesReport),
        [Role::Owner, Role::Manager, Role::FrontDesk, Role::Housekeeping, Role::HkSupervisor]
    );
}

#[test]
fn only_owners_and_managers_manage_media() {
    assert_eq!(roles_allowed(Permission::MediaManage), [Role::Owner, Role::Manager]);
}

#[test]
fn the_front_desk_manages_guest_documents_and_housekeeping_does_not() {
    assert_eq!(roles_allowed(Permission::GuestDocumentsManage), [Role::Owner, Role::Manager, Role::FrontDesk]);
}

#[test]
fn folio_view_is_for_everyone_who_handles_money() {
    assert_eq!(roles_allowed(Permission::FolioView), [Role::Owner, Role::Manager, Role::FrontDesk, Role::Accountant]);
}

#[test]
fn the_front_desk_posts_to_folios_and_the_accountant_does_not() {
    assert_eq!(roles_allowed(Permission::FolioPost), [Role::Owner, Role::Manager, Role::FrontDesk]);
}

#[test]
fn only_owners_and_managers_adjust_folios() {
    assert_eq!(roles_allowed(Permission::FolioAdjust), [Role::Owner, Role::Manager]);
}

#[test]
fn invoices_are_issued_by_the_front_desk_and_the_accountant_too() {
    assert_eq!(
        roles_allowed(Permission::InvoiceIssue),
        [Role::Owner, Role::Manager, Role::FrontDesk, Role::Accountant]
    );
}

#[test]
fn owners_managers_and_the_front_desk_run_the_night_audit() {
    assert_eq!(roles_allowed(Permission::NightAudit), [Role::Owner, Role::Manager, Role::FrontDesk]);
}

#[test]
fn only_owners_manage_tax_settings() {
    assert_eq!(roles_allowed(Permission::TaxSettingsManage), [Role::Owner]);
}

#[test]
fn only_owners_and_managers_override_exchange_rates() {
    assert_eq!(roles_allowed(Permission::FxOverride), [Role::Owner, Role::Manager]);
}

#[test]
fn only_owners_and_managers_post_pos_bills() {
    assert_eq!(roles_allowed(Permission::PosPost), [Role::Owner, Role::Manager]);
}

#[test]
fn linen_is_kept_by_housekeeping_and_managed_by_supervisors() {
    let keepers = [Role::Owner, Role::Manager, Role::Housekeeping, Role::HkSupervisor];
    assert_eq!(roles_allowed(Permission::LinenView), keepers);
    assert_eq!(roles_allowed(Permission::LinenMove), keepers);
    assert_eq!(roles_allowed(Permission::LinenManage), [Role::Owner, Role::Manager, Role::HkSupervisor]);
}

#[test]
fn guest_laundry_orders_are_taken_by_the_front_desk_and_housekeeping() {
    assert_eq!(
        roles_allowed(Permission::LaundryOrders),
        [Role::Owner, Role::Manager, Role::FrontDesk, Role::Housekeeping, Role::HkSupervisor]
    );
}

#[test]
fn only_owners_and_managers_manage_laundry_prices() {
    assert_eq!(roles_allowed(Permission::LaundryPricesManage), [Role::Owner, Role::Manager]);
}
