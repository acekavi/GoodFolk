mod common;

use common::{Hotel, room_type};
use rooms::{RoomTypeChanges, RoomsError, WINDOW_DAYS};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn room_types_are_listed_in_the_order_they_were_created(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let std = hotel.room_type("STD").await;
    let dlx = hotel.room_type("DLX").await;

    let mut tx = hotel.tx().await;
    let listed = rooms::list_room_types(&mut tx, hotel.property).await.unwrap();

    assert_eq!(listed, vec![std.clone(), dlx.clone()]);
    assert_eq!((std.sort_order, dlx.sort_order), (0, 1));
    assert_eq!(dlx.bed_config, vec![rooms::Bed { kind: "queen".into(), count: 1 }]);
    assert!(dlx.active);
    assert_eq!(dlx.version, 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_new_room_type_has_zero_counters_for_the_whole_window(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;

    let dlx = hotel.room_type("DLX").await;

    let counters = hotel.counters(dlx.id).await;
    assert_eq!(counters.len(), usize::try_from(WINDOW_DAYS).unwrap());
    assert_eq!(counters[0].date, hotel.day(0));
    assert_eq!(counters.last().unwrap().date, hotel.day(WINDOW_DAYS - 1));
    assert!(counters.iter().all(|day| day.physical == 0 && day.out_of_order == 0 && day.sold == 0));
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn codes_are_unique_within_a_property(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.room_type("DLX").await;

    let mut tx = hotel.tx().await;
    let again = rooms::create_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, room_type("DLX")).await;

    assert!(matches!(again, Err(RoomsError::Conflict(_))), "{again:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn capacities_must_add_up(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut tx = hotel.tx().await;

    let base_over_max = rooms::NewRoomType { base_occupancy: 4, ..room_type("A") };
    let max_over_people = rooms::NewRoomType { max_occupancy: 4, base_occupancy: 2, ..room_type("B") };
    let first = rooms::create_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, base_over_max).await;
    let second = rooms::create_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, max_over_people).await;

    assert!(matches!(first, Err(RoomsError::Invalid(_))), "{first:?}");
    assert!(matches!(second, Err(RoomsError::Invalid(_))), "{second:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_overbooking_allowance_must_be_0_to_20(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut tx = hotel.tx().await;

    let too_high = rooms::NewRoomType { overbooking: 21, ..room_type("A") };
    let created_high = rooms::create_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, too_high).await;
    let too_low = rooms::NewRoomType { overbooking: -1, ..room_type("B") };
    let created_low = rooms::create_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, too_low).await;
    let min = rooms::NewRoomType { overbooking: 0, ..room_type("C") };
    let created_min = rooms::create_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, min).await;
    let max = rooms::NewRoomType { overbooking: 20, ..room_type("D") };
    let created_max = rooms::create_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, max).await;

    let dlx = hotel.room_type("DLX").await;
    let mut tx = hotel.tx().await;
    let high_changes = RoomTypeChanges { overbooking: Some(21), ..RoomTypeChanges::default() };
    let updated_high =
        rooms::update_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, dlx.id, dlx.version, high_changes)
            .await;
    let low_changes = RoomTypeChanges { overbooking: Some(-1), ..RoomTypeChanges::default() };
    let updated_low =
        rooms::update_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, dlx.id, dlx.version, low_changes)
            .await;

    assert!(matches!(created_high, Err(RoomsError::Invalid(_))), "{created_high:?}");
    assert!(matches!(created_low, Err(RoomsError::Invalid(_))), "{created_low:?}");
    assert_eq!(created_min.unwrap().overbooking, 0);
    assert_eq!(created_max.unwrap().overbooking, 20);
    assert!(matches!(updated_high, Err(RoomsError::Invalid(_))), "{updated_high:?}");
    assert!(matches!(updated_low, Err(RoomsError::Invalid(_))), "{updated_low:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_unknown_property_is_not_found(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut tx = hotel.tx().await;

    let result = rooms::create_room_type(&mut tx, hotel.tenant, hotel.user, Uuid::now_v7(), room_type("DLX")).await;

    assert!(matches!(result, Err(RoomsError::NotFound("property"))), "{result:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn updates_need_the_current_version(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let rename = |name: &str| RoomTypeChanges { name: Some(name.into()), ..RoomTypeChanges::default() };

    let mut tx = hotel.tx().await;
    let renamed =
        rooms::update_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, dlx.id, 1, rename("Deluxe"))
            .await
            .unwrap();
    let stale =
        rooms::update_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, dlx.id, 1, rename("Deluxe Sea"))
            .await;
    let unknown =
        rooms::update_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, Uuid::now_v7(), 1, rename("X"))
            .await;

    assert_eq!((renamed.name.as_str(), renamed.version), ("Deluxe", 2));
    assert_eq!(renamed.code, "DLX");
    assert!(matches!(stale, Err(RoomsError::VersionMismatch("room type"))), "{stale:?}");
    assert!(matches!(unknown, Err(RoomsError::NotFound("room type"))), "{unknown:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn reordering_must_list_every_room_type_once(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let std = hotel.room_type("STD").await;
    let dlx = hotel.room_type("DLX").await;

    let mut tx = hotel.tx().await;
    let missing = rooms::reorder_room_types(&mut tx, hotel.tenant, hotel.property, &[dlx.id]).await;
    let repeated = rooms::reorder_room_types(&mut tx, hotel.tenant, hotel.property, &[dlx.id, dlx.id]).await;
    rooms::reorder_room_types(&mut tx, hotel.tenant, hotel.property, &[dlx.id, std.id]).await.unwrap();
    let listed = rooms::list_room_types(&mut tx, hotel.property).await.unwrap();

    assert!(matches!(missing, Err(RoomsError::Invalid(_))), "{missing:?}");
    assert!(matches!(repeated, Err(RoomsError::Invalid(_))), "{repeated:?}");
    assert_eq!(listed.iter().map(|t| t.code.as_str()).collect::<Vec<_>>(), ["DLX", "STD"]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn sections_are_unique_by_name_and_renamed_with_their_version(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut tx = hotel.tx().await;
    let east = rooms::create_section(&mut tx, hotel.tenant, hotel.user, hotel.property, "East wing").await.unwrap();
    tx.commit().await.unwrap();

    // A failed statement aborts its transaction, so each attempt that can fail gets its own.
    let duplicate =
        rooms::create_section(&mut hotel.tx().await, hotel.tenant, hotel.user, hotel.property, "East wing").await;
    let mut tx = hotel.tx().await;
    let renamed =
        rooms::rename_section(&mut tx, hotel.tenant, hotel.user, hotel.property, east.id, 1, "East").await.unwrap();
    let stale = rooms::rename_section(&mut tx, hotel.tenant, hotel.user, hotel.property, east.id, 1, "E").await;
    let listed = rooms::list_sections(&mut tx, hotel.property).await.unwrap();

    assert!(matches!(duplicate, Err(RoomsError::Conflict(_))), "{duplicate:?}");
    assert_eq!((renamed.name.as_str(), renamed.version), ("East", 2));
    assert!(matches!(stale, Err(RoomsError::VersionMismatch("section"))), "{stale:?}");
    assert_eq!(listed, vec![renamed]);
}
