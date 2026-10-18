mod common;

use common::Hotel;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

/// The folio backfill of `migrations/0027_folio_backfill.sql`, which ran before any test reservation existed: the
/// statements between its markers, run again here.
fn backfill_statements() -> &'static str {
    let migration = include_str!("../../../migrations/0027_folio_backfill.sql");
    let (_, rest) = migration.split_once("-- backfill: start\n").expect("backfill start marker");
    rest.split_once("-- backfill: end").expect("backfill end marker").0
}

/// `(kind, owner)` of every folio of the reservation and its stays, by kind.
async fn folios_of(hotel: &Hotel, reservation: Uuid) -> Vec<(String, Uuid)> {
    sqlx::query_as(
        "select f.kind, coalesce(f.reservation_id, f.reservation_room_id)
         from folio f left join reservation_room rr on rr.id = f.reservation_room_id
         where f.reservation_id = $1 or rr.reservation_id = $1
         order by f.kind, 2",
    )
    .bind(reservation)
    .fetch_all(&hotel.owner)
    .await
    .unwrap()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_reservation_made_before_folios_gets_its_folios_and_default_routing(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let hotel = Hotel::new(opts).await;
    let acme = hotel.account().await;
    let billed = hotel.book_in(hotel.property, 2, Some(acme), None).await;
    let unbilled = hotel.book(None).await;
    // As before 0020: no folios and no routing.
    sqlx::query("delete from folio where reservation_id = any($1) or reservation_room_id = any($2)")
        .bind([billed.id, unbilled.id])
        .bind(billed.rooms.iter().chain(&unbilled.rooms).copied().collect::<Vec<_>>())
        .execute(&hotel.owner)
        .await
        .unwrap();
    assert!(folios_of(&hotel, billed.id).await.is_empty());

    sqlx::raw_sql(backfill_statements()).execute(&hotel.owner).await.unwrap();
    // Run twice, as a migration re-applied by hand would be: nothing is added the second time.
    sqlx::raw_sql(backfill_statements()).execute(&hotel.owner).await.unwrap();

    let mut expected = vec![("deposit".to_owned(), billed.id)];
    let mut rooms = billed.rooms.clone();
    rooms.sort();
    expected.extend(rooms.into_iter().map(|room| ("guest".to_owned(), room)));
    expected.sort();
    assert_eq!(folios_of(&hotel, billed.id).await, expected);
    assert_eq!(
        folios_of(&hotel, unbilled.id).await,
        vec![("deposit".to_owned(), unbilled.id), ("guest".to_owned(), unbilled.rooms[0])]
    );
    // The backfilled folio is the one the app finds, in the reservation's tenant.
    let backfilled: Uuid = sqlx::query_scalar("select id from folio where kind = 'deposit' and reservation_id = $1")
        .bind(billed.id)
        .fetch_one(&hotel.owner)
        .await
        .unwrap();
    let mut tx = hotel.tx().await;
    let found = folio::ensure_deposit_folio(&mut tx, hotel.tenant, hotel.property, billed.id).await.unwrap();
    assert_eq!(found, backfilled);

    let mut default_routing = folio::DEFAULT_ROUTING.map(String::from).to_vec();
    default_routing.sort();
    assert_eq!(folio::routing(&mut tx, hotel.property, billed.id).await.unwrap(), default_routing);
    assert!(folio::routing(&mut tx, hotel.property, unbilled.id).await.unwrap().is_empty());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_backfill_keeps_routing_staff_already_set(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let acme = hotel.account().await;
    let booking = hotel.book(Some(acme)).await;
    hotel.route(booking.id, &["laundry"]).await.unwrap();

    sqlx::raw_sql(backfill_statements()).execute(&hotel.owner).await.unwrap();

    let routed = folio::routing(&mut hotel.tx().await, hotel.property, booking.id).await.unwrap();
    assert_eq!(routed, vec!["laundry".to_owned()]);
}
