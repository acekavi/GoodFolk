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
            ],
        ),
        (Role::Accountant, &[PropertiesView, RoomsView, InventoryView, RatesView, ReservationsView]),
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
