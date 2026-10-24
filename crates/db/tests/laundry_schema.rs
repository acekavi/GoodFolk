//! What the Phase 8 migration (`migrations/0033_laundry.sql`) puts on linen, laundry batches and ops alerts, checked
//! directly in the database.

use db::testing::app_pool;
use db::{Scope, TenantId, begin};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

struct Hotel {
    tenant: Uuid,
    property: Uuid,
    item: Uuid,
    store: Uuid,
    laundry: Uuid,
}

/// A tenant with one property, a linen item, a store and a laundry. Runs as the superuser (no RLS).
async fn hotel(pool: &PgPool) -> Hotel {
    let hotel = Hotel {
        tenant: Uuid::now_v7(),
        property: Uuid::now_v7(),
        item: Uuid::now_v7(),
        store: Uuid::now_v7(),
        laundry: Uuid::now_v7(),
    };
    sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(hotel.tenant).execute(pool).await.unwrap();
    let statements = [
        "insert into property (id, tenant_id, code, name, timezone, base_currency, business_date)
         values ($2, $1, 'GAL', 'Hotel', 'Asia/Colombo', 'LKR', current_date)",
        "insert into linen_item (id, tenant_id, property_id, code, name) values ($3, $1, $2, 'SHEET_K', 'King sheet')",
        "insert into linen_location (id, tenant_id, property_id, name, kind) values ($4, $1, $2, 'Store', 'store')",
        "insert into linen_location (id, tenant_id, property_id, name, kind) values ($5, $1, $2, 'Laundry', 'laundry')",
    ];
    for statement in statements {
        sqlx::query(statement)
            .bind(hotel.tenant)
            .bind(hotel.property)
            .bind(hotel.item)
            .bind(hotel.store)
            .bind(hotel.laundry)
            .execute(pool)
            .await
            .unwrap();
    }
    hotel
}

fn constraint(result: Result<impl std::fmt::Debug, sqlx::Error>) -> String {
    let err = result.unwrap_err();
    err.as_database_error().and_then(|db_err| db_err.constraint()).unwrap_or_default().to_owned()
}

/// Runs `statement` as the app role in `hotel`'s tenant (`$1` the tenant, `$2` the property, `$3` the item, `$4` the
/// store, `$5` the laundry) and returns the error's SQLSTATE.
async fn app_error(pool: &PgPool, hotel: &Hotel, statement: &'static str) -> Option<String> {
    let mut tx = begin(pool, Scope::tenant(TenantId(hotel.tenant))).await.unwrap();
    let result = sqlx::query(statement)
        .bind(hotel.tenant)
        .bind(hotel.property)
        .bind(hotel.item)
        .bind(hotel.store)
        .bind(hotel.laundry)
        .execute(&mut *tx)
        .await;
    result.err().and_then(|e| e.as_database_error().and_then(|e| e.code()).map(|c| c.into_owned()))
}

/// The test database's owner, which bypasses row-level security, and a pool acting as `goodfolk_app`.
async fn pools(opts: PgConnectOptions) -> (PgPool, PgPool) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts.clone()).await.unwrap();
    (owner, app_pool(opts, 1).await)
}

const PERMISSION_DENIED: &str = "42501";
const CHECK_VIOLATION: &str = "23514";

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_movement_ledger_is_append_only_for_the_app_role(_: PgPoolOptions, opts: PgConnectOptions) {
    let (owner, pool) = pools(opts).await;
    let hotel = hotel(&owner).await;
    sqlx::query(
        "insert into linen_movement (id, tenant_id, property_id, item_id, kind, quantity, to_location_id, business_date)
         values (gen_random_uuid(), $1, $2, $3, 'receive', 10, $4, current_date)",
    )
    .bind(hotel.tenant)
    .bind(hotel.property)
    .bind(hotel.item)
    .bind(hotel.store)
    .execute(&owner)
    .await
    .unwrap();
    for statement in ["update linen_movement set quantity = 1", "delete from linen_movement"] {
        assert_eq!(app_error(&pool, &hotel, statement).await.as_deref(), Some(PERMISSION_DENIED), "{statement}");
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn stock_cannot_go_negative(_: PgPoolOptions, opts: PgConnectOptions) {
    let (owner, pool) = pools(opts).await;
    let hotel = hotel(&owner).await;

    let negative = app_error(
        &pool,
        &hotel,
        "insert into linen_stock (tenant_id, property_id, location_id, item_id, quantity) values ($1, $2, $4, $3, -1)",
    )
    .await;

    assert_eq!(negative.as_deref(), Some(CHECK_VIOLATION));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_property_has_one_active_store(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts).await.unwrap();
    let hotel = hotel(&owner).await;
    let store = |name: &'static str, active: bool| {
        sqlx::query(
            "insert into linen_location (id, tenant_id, property_id, name, kind, active)
             values (gen_random_uuid(), $1, $2, $3, 'store', $4)",
        )
        .bind(hotel.tenant)
        .bind(hotel.property)
        .bind(name)
        .bind(active)
        .execute(&owner)
    };

    let second = store("Second store", true).await;
    let retired = store("Old store", false).await;

    assert_eq!(constraint(second), "linen_location_one_store_idx");
    retired.unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_pantry_has_a_section_and_nothing_else_does(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts).await.unwrap();
    let hotel = hotel(&owner).await;
    let section = Uuid::now_v7();
    sqlx::query("insert into housekeeping_section (id, tenant_id, property_id, name) values ($1, $2, $3, 'East')")
        .bind(section)
        .bind(hotel.tenant)
        .bind(hotel.property)
        .execute(&owner)
        .await
        .unwrap();
    let location = |name: &'static str, kind: &'static str, section: Option<Uuid>| {
        sqlx::query(
            "insert into linen_location (id, tenant_id, property_id, name, kind, section_id)
             values (gen_random_uuid(), $1, $2, $3, $4, $5)",
        )
        .bind(hotel.tenant)
        .bind(hotel.property)
        .bind(name)
        .bind(kind)
        .bind(section)
        .execute(&owner)
    };

    let pantry_without_section = location("Pantry", "pantry", None).await;
    let store_with_section = location("Annex", "store", Some(section)).await;
    let laundry_with_section = location("Annex", "laundry", Some(section)).await;

    assert_eq!(constraint(pantry_without_section), "linen_location_check");
    assert_eq!(constraint(laundry_with_section), "linen_location_check");
    assert_eq!(constraint(store_with_section), "linen_location_check");
    location("East pantry", "pantry", Some(section)).await.unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_write_off_needs_a_reason(_: PgPoolOptions, opts: PgConnectOptions) {
    let (owner, pool) = pools(opts).await;
    let hotel = hotel(&owner).await;

    let without = app_error(
        &pool,
        &hotel,
        "insert into linen_movement (id, tenant_id, property_id, item_id, kind, quantity, from_location_id, business_date)
         values (gen_random_uuid(), $1, $2, $3, 'write_off', 1, $4, current_date)",
    )
    .await;
    let with = app_error(
        &pool,
        &hotel,
        "insert into linen_movement (id, tenant_id, property_id, item_id, kind, quantity, from_location_id, reason,
                                     business_date)
         values (gen_random_uuid(), $1, $2, $3, 'write_off', 1, $4, 'Torn', current_date)",
    )
    .await;

    assert_eq!(without.as_deref(), Some(CHECK_VIOLATION));
    assert_eq!(with, None);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_send_needs_a_batch(_: PgPoolOptions, opts: PgConnectOptions) {
    let (owner, pool) = pools(opts).await;
    let hotel = hotel(&owner).await;

    let without = app_error(
        &pool,
        &hotel,
        "insert into linen_movement (id, tenant_id, property_id, item_id, kind, quantity, from_location_id,
                                     to_location_id, business_date)
         values (gen_random_uuid(), $1, $2, $3, 'send', 1, $4, $5, current_date)",
    )
    .await;

    assert_eq!(without.as_deref(), Some(CHECK_VIOLATION));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_linen_alert_kind_is_accepted(_: PgPoolOptions, opts: PgConnectOptions) {
    let (owner, pool) = pools(opts).await;
    let hotel = hotel(&owner).await;
    let alert = "insert into ops_alert (tenant_id, property_id, kind, message) values ($1, $2, 'linen', 'Store: SHEET_K 40 of 60')";

    assert_eq!(app_error(&pool, &hotel, alert).await, None);
    let unknown = app_error(
        &pool,
        &hotel,
        "insert into ops_alert (tenant_id, property_id, kind, message) values ($1, $2, 'other', 'x')",
    )
    .await;
    assert_eq!(unknown.as_deref(), Some(CHECK_VIOLATION));
}

/// A second property of `hotel`'s tenant with two locations, a batch between them, a price and an order for a stay.
/// Returns the property, batch, price and order.
async fn other_property(pool: &PgPool, hotel: &Hotel) -> (Uuid, Uuid, Uuid, Uuid) {
    let ids: [Uuid; 11] = std::array::from_fn(|_| Uuid::now_v7());
    let [property, from, to, batch, price, order, room_type, plan, guest, reservation, stay] = ids;
    let statements = [
        "insert into property (id, tenant_id, code, name, timezone, base_currency, business_date)
         values ($2, $1, 'OTH', 'Other', 'Asia/Colombo', 'LKR', current_date)",
        "insert into linen_location (id, tenant_id, property_id, name, kind) values ($3, $1, $2, 'Store', 'store')",
        "insert into linen_location (id, tenant_id, property_id, name, kind) values ($4, $1, $2, 'Laundry', 'laundry')",
        "insert into laundry_batch (id, tenant_id, property_id, to_location_id, from_location_id, status)
         values ($5, $1, $2, $4, $3, 'sent')",
        "insert into laundry_price (id, tenant_id, property_id, item_code, service, name, price)
         values ($6, $1, $2, 'SHIRT', 'wash', 'Shirt, wash', 25000)",
        "insert into room_type (id, tenant_id, property_id, code, name, base_occupancy, max_adults, max_children, max_occupancy)
         values ($8, $1, $2, 'DLX', 'Deluxe', 2, 2, 0, 2)",
        "insert into rate_plan (id, tenant_id, property_id, code, name, kind, segment, currency)
         values ($9, $1, $2, 'BAR', 'Best available', 'standard', 'IBE', 'USD')",
        "insert into guest (id, tenant_id, first_name, last_name, residency) values ($10, $1, 'Ada', 'Silva', 'resident')",
        "insert into reservation (id, tenant_id, property_id, confirmation_no, source, booker_guest_id,
                                  booked_business_date)
         values ($11, $1, $2, 'OTH-000001', 'front_desk', $10, current_date)",
        "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, stay, adults, children,
                                       rate_plan_id, meal_plan, status, primary_guest_id, currency)
         values ($12, $1, $2, $11, $8, daterange(current_date, current_date + 2), 2, 0, $9, 'RO', 'confirmed', $10, 'USD')",
        "insert into laundry_order (id, tenant_id, property_id, reservation_room_id, room_number, status)
         values ($7, $1, $2, $12, '101', 'collected')",
    ];
    for statement in statements {
        sqlx::query(statement)
            .bind(hotel.tenant)
            .bind(property)
            .bind(from)
            .bind(to)
            .bind(batch)
            .bind(price)
            .bind(order)
            .bind(room_type)
            .bind(plan)
            .bind(guest)
            .bind(reservation)
            .bind(stay)
            .execute(pool)
            .await
            .unwrap();
    }
    (property, batch, price, order)
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_movement_cannot_name_another_propertys_batch(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts).await.unwrap();
    let hotel = hotel(&owner).await;
    let (_, batch, _, _) = other_property(&owner, &hotel).await;

    let result = sqlx::query(
        "insert into linen_movement (id, tenant_id, property_id, item_id, kind, quantity, from_location_id,
                                     to_location_id, batch_id, business_date)
         values (gen_random_uuid(), $1, $2, $3, 'send', 1, $4, $5, $6, current_date)",
    )
    .bind(hotel.tenant)
    .bind(hotel.property)
    .bind(hotel.item)
    .bind(hotel.store)
    .bind(hotel.laundry)
    .bind(batch)
    .execute(&owner)
    .await;

    assert_eq!(constraint(result), "linen_movement_property_id_batch_id_fkey");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_order_line_stays_within_its_orders_property(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts).await.unwrap();
    let hotel = hotel(&owner).await;
    let (other, _, price, order) = other_property(&owner, &hotel).await;
    let own_price = Uuid::now_v7();
    sqlx::query(
        "insert into laundry_price (id, tenant_id, property_id, item_code, service, name, price)
         values ($3, $1, $2, 'SHIRT', 'wash', 'Shirt, wash', 25000)",
    )
    .bind(hotel.tenant)
    .bind(hotel.property)
    .bind(own_price)
    .execute(&owner)
    .await
    .unwrap();
    let line = |property: Uuid, order: Uuid, price: Uuid| {
        sqlx::query(
            "insert into laundry_order_line (tenant_id, property_id, order_id, position, price_id, item_code, service,
                                             name, unit_price, quantity)
             values ($1, $2, $3, 1, $4, 'SHIRT', 'wash', 'Shirt, wash', 25000, 1)",
        )
        .bind(hotel.tenant)
        .bind(property)
        .bind(order)
        .bind(price)
        .execute(&owner)
    };

    let foreign_order = line(hotel.property, order, own_price).await;
    let foreign_price = line(other, order, own_price).await;
    let own = line(other, order, price).await;

    assert_eq!(constraint(foreign_order), "laundry_order_line_property_id_order_id_fkey");
    assert_eq!(constraint(foreign_price), "laundry_order_line_property_id_price_id_fkey");
    own.unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_batch_goes_between_two_locations(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts).await.unwrap();
    let hotel = hotel(&owner).await;

    let result = sqlx::query(
        "insert into laundry_batch (id, tenant_id, property_id, to_location_id, from_location_id, status)
         values (gen_random_uuid(), $1, $2, $3, $3, 'sent')",
    )
    .bind(hotel.tenant)
    .bind(hotel.property)
    .bind(hotel.laundry)
    .execute(&owner)
    .await;

    assert_eq!(constraint(result), "laundry_batch_locations_check");
}
