//! Counter writers read the business date `for share`, so they wait for the night audit's roll (which holds the
//! property row `for update` while it moves the date on) and then work from the new date. See "Locking" in
//! `docs/specs/phase-7b-night-audit.md`.

mod common;

use common::Hotel;
use db::{Scope, Tx};
use rooms::{NewRoom, WINDOW_DAYS};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::time::Duration;

/// How long a writer must still be waiting behind the roll.
const WAIT: Duration = Duration::from_millis(300);

/// Holds the property row as the roll does, until [`advance`] commits.
async fn hold_property(hotel: &Hotel) -> Tx {
    let mut roll = hotel.tx().await;
    sqlx::query("select 1 from property where id = $1 for update")
        .bind(hotel.property)
        .execute(&mut *roll)
        .await
        .unwrap();
    roll
}

/// Moves the business date on by a day and commits, as the roll does.
async fn advance(hotel: &Hotel, mut roll: Tx) {
    sqlx::query("update property set business_date = business_date + 1 where id = $1")
        .bind(hotel.property)
        .execute(&mut *roll)
        .await
        .unwrap();
    roll.commit().await.unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_counter_writer_waits_for_the_roll_and_uses_the_new_business_date(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let room_type = hotel.room_type("A").await;
    let pool = db::testing::app_pool(opts, 2).await;
    let roll = hold_property(&hotel).await;

    let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);
    let writer = tokio::spawn(async move {
        let mut tx = db::begin(&pool, Scope::tenant(tenant)).await.unwrap();
        let input = NewRoom { room_type_id: room_type.id, number: "101".into(), floor: None, section_id: None };
        rooms::create_room(&mut tx, tenant, user, property, input).await.unwrap();
        tx.commit().await.unwrap();
    });
    tokio::time::sleep(WAIT).await;
    assert!(!writer.is_finished(), "the writer waits for the roll");
    advance(&hotel, roll).await;
    writer.await.unwrap();

    let mut tx = hotel.tx().await;
    let days = rooms::list_inventory(&mut tx, property, hotel.day(0), hotel.day(WINDOW_DAYS + 1)).await.unwrap();
    let physical: Vec<(time::Date, i32)> =
        days.iter().filter(|day| day.room_type_id == room_type.id).map(|day| (day.date, day.physical)).collect();
    let expected: Vec<(time::Date, i32)> =
        (0..=WINDOW_DAYS).map(|offset| (hotel.day(offset), i32::from(offset > 0))).collect();
    assert_eq!(physical, expected, "the closed day keeps its count; the room counts from the new business date");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn extending_the_window_waits_for_the_roll(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let room_type = hotel.room_type("A").await;
    let pool = db::testing::app_pool(opts, 2).await;
    let roll = hold_property(&hotel).await;

    let (tenant, property) = (hotel.tenant, hotel.property);
    let extend = tokio::spawn(async move {
        let mut tx = db::begin(&pool, Scope::tenant(tenant)).await.unwrap();
        rooms::extend_window(&mut tx, property).await.unwrap();
        tx.commit().await.unwrap();
    });
    tokio::time::sleep(WAIT).await;
    assert!(!extend.is_finished(), "the extension waits for the roll");
    advance(&hotel, roll).await;
    extend.await.unwrap();

    let mut tx = hotel.tx().await;
    let days = rooms::list_inventory(&mut tx, property, hotel.day(0), hotel.day(WINDOW_DAYS + 1)).await.unwrap();
    let dates: Vec<time::Date> =
        days.iter().filter(|day| day.room_type_id == room_type.id).map(|day| day.date).collect();
    let expected: Vec<time::Date> = (0..=WINDOW_DAYS).map(|offset| hotel.day(offset)).collect();
    assert_eq!(dates, expected, "the window runs from the new business date");
}
