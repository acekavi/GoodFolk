use identity::{
    ALL_PERMISSIONS, ALL_ROLES, Grant, JUNIOR_ROLES, Permission, Role, allows, check_may_act_on, check_may_give,
    describe,
};
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
    for role in ALL_ROLES {
        assert_eq!(Role::parse(role.as_str()), Some(role));
    }
    assert_eq!(Role::parse("root"), None);
}

#[test]
fn each_role_has_exactly_its_permissions() {
    use Permission::*;
    let all = ALL_PERMISSIONS;
    let expected: [(Role, &[Permission]); 7] = [
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
                StaffManage,
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
        (Role::Pos, &[PosPost]),
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
    ALL_ROLES
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
fn owners_managers_and_the_pos_role_post_pos_bills() {
    assert_eq!(roles_allowed(Permission::PosPost), [Role::Owner, Role::Manager, Role::Pos]);
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

#[test]
fn only_owners_and_managers_manage_staff() {
    assert_eq!(roles_allowed(Permission::StaffManage), [Role::Owner, Role::Manager]);
}

#[test]
fn the_pos_role_posts_bills_and_nothing_else() {
    let grants = [Grant { property_id: None, role: Role::Pos }];

    for permission in ALL_PERMISSIONS {
        assert_eq!(allows(&grants, permission, Some(HOTEL)), permission == Permission::PosPost, "{permission:?}");
    }
}

#[test]
fn junior_roles_are_every_role_but_owner_and_manager() {
    let expected: Vec<Role> = ALL_ROLES.into_iter().filter(|r| !matches!(r, Role::Owner | Role::Manager)).collect();

    assert_eq!(JUNIOR_ROLES.len(), expected.len());
    assert!(expected.iter().all(|role| JUNIOR_ROLES.contains(role)));
}

#[test]
fn every_permission_is_described() {
    for permission in ALL_PERMISSIONS {
        assert!(!describe(permission).trim().is_empty(), "{permission:?}");
    }
}

/// Fails to compile when a permission is added without a slot here, which keeps `ALL_PERMISSIONS` complete.
fn slot(permission: Permission) -> usize {
    use Permission::*;
    match permission {
        PropertiesView => 0,
        PropertiesCreate => 1,
        PropertiesManage => 2,
        RoomsView => 3,
        RoomsManage => 4,
        InventoryView => 5,
        InventoryBlock => 6,
        RatesView => 7,
        RatesManage => 8,
        ReservationsView => 9,
        ReservationsManage => 10,
        FrontDeskCheckIn => 11,
        HousekeepingView => 12,
        HousekeepingUpdate => 13,
        HousekeepingSupervise => 14,
        RoomIssuesReport => 15,
        MediaManage => 16,
        GuestDocumentsManage => 17,
        FolioView => 18,
        FolioPost => 19,
        FolioAdjust => 20,
        InvoiceIssue => 21,
        TaxSettingsManage => 22,
        FxOverride => 23,
        PosPost => 24,
        NightAudit => 25,
        LinenView => 26,
        LinenMove => 27,
        LinenManage => 28,
        LaundryOrders => 29,
        LaundryPricesManage => 30,
        StaffManage => 31,
    }
}

#[test]
fn all_permissions_lists_every_permission_once() {
    for (index, permission) in ALL_PERMISSIONS.into_iter().enumerate() {
        assert_eq!(slot(permission), index, "{permission:?}");
    }
}

fn grant(role: Role, property_id: Option<Uuid>) -> Grant {
    Grant { property_id, role }
}

#[test]
fn an_owner_gives_any_role_but_owner() {
    let owner = [grant(Role::Owner, None)];

    assert!(check_may_give(&owner, &[grant(Role::Manager, None), grant(Role::FrontDesk, Some(HOTEL))]).is_ok());
    assert_eq!(check_may_give(&owner, &[grant(Role::Owner, None)]), Err("nobody can grant the owner role".into()));
}

#[test]
fn a_manager_gives_only_junior_roles() {
    let manager = [grant(Role::Manager, None)];

    for role in JUNIOR_ROLES {
        assert!(check_may_give(&manager, &[grant(role, None), grant(role, Some(HOTEL))]).is_ok(), "{role:?}");
    }
    assert_eq!(
        check_may_give(&manager, &[grant(Role::Manager, Some(HOTEL))]),
        Err("only owners can grant the manager role".into())
    );
}

#[test]
fn a_property_manager_gives_only_at_their_properties_and_never_tenant_wide() {
    let manager = [grant(Role::Manager, Some(HOTEL))];

    assert!(check_may_give(&manager, &[grant(Role::FrontDesk, Some(HOTEL))]).is_ok());
    assert_eq!(
        check_may_give(&manager, &[grant(Role::FrontDesk, Some(OTHER_HOTEL))]),
        Err(format!("you do not manage property {OTHER_HOTEL}"))
    );
    assert_eq!(
        check_may_give(&manager, &[grant(Role::Pos, None)]),
        Err("a tenant-wide pos grant needs a tenant-wide manager".into())
    );
}

#[test]
fn a_junior_gives_nothing() {
    let front_desk = [grant(Role::FrontDesk, None)];

    assert!(check_may_give(&front_desk, &[grant(Role::Housekeeping, Some(HOTEL))]).is_err());
    assert!(check_may_act_on(&front_desk, &[grant(Role::Housekeeping, Some(HOTEL))]).is_err());
}

#[test]
fn a_manager_acts_only_on_members_whose_every_grant_is_junior() {
    let manager = [grant(Role::Manager, Some(HOTEL))];
    let tenant_wide_manager = [grant(Role::Manager, None)];
    let owner = [grant(Role::Owner, None)];
    let mixed = [grant(Role::FrontDesk, Some(HOTEL)), grant(Role::Manager, Some(OTHER_HOTEL))];

    assert!(check_may_act_on(&manager, &[grant(Role::Accountant, Some(HOTEL)), grant(Role::Pos, Some(HOTEL))]).is_ok());
    assert_eq!(check_may_act_on(&manager, &mixed), Err("only owners can manage a member with the manager role".into()));
    assert_eq!(
        check_may_act_on(&tenant_wide_manager, &[grant(Role::Owner, None)]),
        Err("only owners can manage a member with the owner role".into())
    );
    assert!(check_may_act_on(&owner, &mixed).is_ok());
}

#[test]
fn a_property_manager_acts_only_on_members_at_their_properties() {
    let manager = [grant(Role::Manager, Some(HOTEL))];
    let elsewhere = [grant(Role::FrontDesk, Some(HOTEL)), grant(Role::Housekeeping, Some(OTHER_HOTEL))];

    assert_eq!(check_may_act_on(&manager, &elsewhere), Err(format!("you do not manage property {OTHER_HOTEL}")));
    assert!(check_may_act_on(&[grant(Role::Manager, Some(OTHER_HOTEL)), manager[0]], &elsewhere).is_ok());
}

#[test]
fn a_tenant_wide_junior_needs_a_tenant_wide_manager() {
    let member = [grant(Role::Accountant, None)];

    assert_eq!(
        check_may_act_on(&[grant(Role::Manager, Some(HOTEL))], &member),
        Err("a member with a tenant-wide accountant grant needs a tenant-wide manager".into())
    );
    assert!(check_may_act_on(&[grant(Role::Manager, None)], &member).is_ok());
    assert!(check_may_act_on(&[grant(Role::Manager, None)], &[grant(Role::Pos, Some(OTHER_HOTEL))]).is_ok());
}
