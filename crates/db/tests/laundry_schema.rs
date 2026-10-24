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
    let laundry_with_section = location("Annex", "laundry", Some(section)).await;

    assert_eq!(constraint(pantry_without_section), "linen_location_check");
    assert_eq!(constraint(laundry_with_section), "linen_location_check");
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
