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
    for role in [Role::Owner, Role::Manager, Role::FrontDesk, Role::Housekeeping, Role::Accountant] {
        assert_eq!(Role::parse(role.as_str()), Some(role));
    }
    assert_eq!(Role::parse("root"), None);
}
