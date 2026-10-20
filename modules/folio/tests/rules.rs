use db::testing::app_pool;
use db::{Scope, TenantId, UserId, begin};
use folio::load_rules;
use property::{NewProperty, create_property};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

/// The preset backfill of `migrations/0020_folio.sql`, which ran on an empty database before any test property
/// existed: the statement between its markers, run again here.
fn backfill_statement() -> &'static str {
    let migration = include_str!("../../../migrations/0020_folio.sql");
    let (_, rest) = migration.split_once("-- backfill: start\n").expect("backfill start marker");
    rest.split_once("-- backfill: end").expect("backfill end marker").0
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_backfill_gives_an_existing_property_exactly_the_rust_preset(pool: PgPool) {
    let tenant = TenantId(Uuid::now_v7());
    let property = Uuid::now_v7();
    sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(tenant.0).execute(&pool).await.unwrap();
    sqlx::query(
        "insert into property (id, tenant_id, code, name, timezone, base_currency, business_date)
         values ($1, $2, 'OLD', 'Old Hotel', 'Asia/Colombo', 'LKR', current_date)",
    )
    .bind(property)
    .bind(tenant.0)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(backfill_statement()).execute(&pool).await.unwrap();

    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    assert_eq!(load_rules(&mut tx, property).await.unwrap(), tax::sri_lanka_preset(false));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_backfill_halves_the_levy_for_a_small_turnover_property(pool: PgPool) {
    let tenant = TenantId(Uuid::now_v7());
    let property = Uuid::now_v7();
    sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(tenant.0).execute(&pool).await.unwrap();
    sqlx::query(
        "insert into property (id, tenant_id, code, name, timezone, base_currency, business_date, tdl_small_turnover)
         values ($1, $2, 'SML', 'Small Hotel', 'Asia/Colombo', 'LKR', current_date, true)",
    )
    .bind(property)
    .bind(tenant.0)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(backfill_statement()).execute(&pool).await.unwrap();

    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    assert_eq!(load_rules(&mut tx, property).await.unwrap(), tax::sri_lanka_preset(true));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_new_property_gets_the_preset(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (tenant, user) = (TenantId(Uuid::now_v7()), UserId(Uuid::now_v7()));
    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(tenant.0).execute(&mut *tx).await.unwrap();
    sqlx::query("insert into app_user (id, email, password_hash, display_name) values ($1, $2, 'x', 'U')")
        .bind(user.0)
        .bind(format!("{}@example.com", user.0))
        .execute(&mut *tx)
        .await
        .unwrap();

    let created = create_property(
        &mut tx,
        tenant,
        user,
        NewProperty {
            code: "NEW".into(),
            name: "New Hotel".into(),
            timezone: "Asia/Colombo".into(),
            base_currency: "LKR".into(),
        },
    )
    .await
    .unwrap();

    assert_eq!(load_rules(&mut tx, created.id).await.unwrap(), tax::sri_lanka_preset(false));
}
