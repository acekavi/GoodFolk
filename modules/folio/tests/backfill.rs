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

/// The statistics-date backfill of `migrations/0031_night_audit.sql`, which ran before any test stay existed.
fn business_date_backfill() -> &'static str {
    let migration = include_str!("../../../migrations/0031_night_audit.sql");
    let (_, rest) = migration.split_once("-- backfill: start\n").expect("backfill start marker");
    rest.split_once("-- backfill: end").expect("backfill end marker").0
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn bookings_and_cancellations_made_before_night_audit_get_their_business_dates(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let hotel = Hotel::new(opts).await;
    let booking = hotel.book_in(hotel.property, 2, None, None).await;
    let (cancelled, no_show) = (booking.rooms[0], booking.rooms[1]);
    // As before 0031, with the property in Asia/Colombo (UTC+5:30): 20:00 UTC is already the next day there.
    sqlx::query("update reservation set booked_business_date = date '2000-01-01', created_at = timestamptz '2026-03-01 20:00+00' where id = $1")
        .bind(booking.id)
        .execute(&hotel.owner)
        .await
        .unwrap();
    sqlx::query(
        "update reservation_room
         set status = 'cancelled', cancelled_at = timestamptz '2026-03-05 21:00+00', cancellation_penalty = 0,
             cancelled_business_date = date '2000-01-01'
         where id = $1",
    )
    .bind(cancelled)
    .execute(&hotel.owner)
    .await
    .unwrap();
    sqlx::query(
        "update reservation_room set status = 'no_show', cancelled_business_date = date '2000-01-01' where id = $1",
    )
    .bind(no_show)
    .execute(&hotel.owner)
    .await
    .unwrap();

    sqlx::raw_sql(business_date_backfill()).execute(&hotel.owner).await.unwrap();

    let booked: time::Date = sqlx::query_scalar("select booked_business_date from reservation where id = $1")
        .bind(booking.id)
        .fetch_one(&hotel.owner)
        .await
        .unwrap();
    let dates: Vec<(Uuid, time::Date, time::Date)> = sqlx::query_as(
        "select id, cancelled_business_date, lower(stay) from reservation_room where id = any($1) order by id",
    )
    .bind([cancelled, no_show])
    .fetch_all(&hotel.owner)
    .await
    .unwrap();
    assert_eq!(booked, time::macros::date!(2026 - 03 - 02));
    let by_room = |room: Uuid| dates.iter().find(|(id, ..)| *id == room).unwrap();
    assert_eq!(by_room(cancelled).1, time::macros::date!(2026 - 03 - 06));
    assert_eq!(by_room(no_show).1, by_room(no_show).2, "a no-show is dated its arrival day");
}

/// Runs `statements` as a table owner that is neither a superuser nor BYPASSRLS, as a managed database's migration
/// role is: forced row-level security applies to it, so a backfill sees only what it lifts that for.
async fn as_plain_owner(hotel: &Hotel, statements: &'static str) {
    let mut tx = hotel.owner.begin().await.unwrap();
    // Roles are the cluster's, so tests running at once may race to create or grant this one.
    sqlx::raw_sql(
        "do $$ begin
           create role goodfolk_plain_owner nologin nosuperuser nobypassrls;
         exception when duplicate_object or unique_violation then null;
         end $$;
         do $$ begin
           grant goodfolk_owner to goodfolk_plain_owner;
         exception when others then null;
         end $$;",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let mut tx = hotel.owner.begin().await.unwrap();
    sqlx::query("set local role goodfolk_plain_owner").execute(&mut *tx).await.unwrap();
    sqlx::raw_sql(statements).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
}

/// The tax-line backfill of `migrations/0045_line_tax_date.sql`: the statements between its markers, run again here.
fn tax_date_statements() -> &'static str {
    let migration = include_str!("../../../migrations/0045_line_tax_date.sql");
    let (_, rest) = migration.split_once("-- backfill: start\n").expect("backfill start marker");
    rest.split_once("-- backfill: end").expect("backfill end marker").0
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_tax_line_backfill_gives_old_rows_their_property_and_business_date(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let hotel = Hotel::new(opts).await;
    let booking = hotel.book(None).await;
    let line = Uuid::now_v7();
    sqlx::query(
        "insert into folio_line (id, tenant_id, folio_id, source, category, description, net, total, currency,
                                 base_total, rate, rate_date, business_date)
         values ($1, $2, $3, 'manual', 'misc', 'Old', 100, 110, 'LKR', 110, 1, $4, $4)",
    )
    .bind(line)
    .bind(hotel.tenant.0)
    .bind(booking.guests[0])
    .bind(hotel.business_date)
    .execute(&hotel.owner)
    .await
    .unwrap();
    // The insert trigger sets the two columns; as before 0045, they are empty.
    sqlx::query(
        "insert into folio_line_tax (tenant_id, line_id, code, name, kind, rate_bp, base, amount, presentation)
         values ($1, $2, 'SC', 'Service charge', 'service_charge', 1000, 100, 10, 'on_bill')",
    )
    .bind(hotel.tenant.0)
    .bind(line)
    .execute(&hotel.owner)
    .await
    .unwrap();
    sqlx::raw_sql(
        "alter table folio_line_tax alter column property_id drop not null;
         alter table folio_line_tax alter column business_date drop not null;
         update folio_line_tax set property_id = null, business_date = null;",
    )
    .execute(&hotel.owner)
    .await
    .unwrap();

    as_plain_owner(&hotel, tax_date_statements()).await;
    // Run twice, as a migration re-applied by hand would be.
    as_plain_owner(&hotel, tax_date_statements()).await;

    let found: (Option<Uuid>, Option<time::Date>) =
        sqlx::query_as("select property_id, business_date from folio_line_tax where line_id = $1")
            .bind(line)
            .fetch_one(&hotel.owner)
            .await
            .unwrap();
    assert_eq!(found, (Some(hotel.property), Some(hotel.business_date)));
}

/// The outlet backfill of `migrations/0046_reports_fixes.sql`: the statements between its markers, run again here.
fn report_group_statements() -> &'static str {
    let migration = include_str!("../../../migrations/0046_reports_fixes.sql");
    let (_, rest) = migration.split_once("-- backfill: start\n").expect("backfill start marker");
    rest.split_once("-- backfill: end").expect("backfill end marker").0
}

/// An outlet that 0041 could not give a report group (its owner read no outlets under forced row-level security) gets
/// its `food` one; an outlet with a group keeps it.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_report_group_backfill_gives_each_outlet_without_one_food(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let (missing, kept) = (hotel.outlet("SPA", "Spa", false).await, hotel.outlet("BAR", "Bar", true).await);
    sqlx::query("update outlet_report_group set report_group = 'bar' where outlet_id = $1")
        .bind(kept.id)
        .execute(&hotel.owner)
        .await
        .unwrap();
    sqlx::query("delete from outlet_report_group where outlet_id = $1")
        .bind(missing.id)
        .execute(&hotel.owner)
        .await
        .unwrap();

    as_plain_owner(&hotel, report_group_statements()).await;
    as_plain_owner(&hotel, report_group_statements()).await;

    let groups: Vec<(Uuid, String, time::Date)> = sqlx::query_as(
        "select outlet_id, report_group, effective_from from outlet_report_group where outlet_id = any($1)
         order by report_group",
    )
    .bind([missing.id, kept.id])
    .fetch_all(&hotel.owner)
    .await
    .unwrap();
    let start = time::macros::date!(2000 - 01 - 01);
    assert_eq!(groups, [(kept.id, "bar".to_owned(), start), (missing.id, "food".to_owned(), start)]);
}
