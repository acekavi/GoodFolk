//! The booking site's schema: the `ibe_resolve` definer and the `ibe_site` checks.

use db::testing::app_pool;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

async fn hotel(pool: &PgPool, code: &str) -> (Uuid, Uuid) {
    let (tenant, property) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(tenant).execute(pool).await.unwrap();
    sqlx::query(
        "insert into property (id, tenant_id, code, name, timezone, base_currency, business_date)
         values ($2, $1, $3, 'Hotel', 'Asia/Colombo', 'LKR', current_date)",
    )
    .bind(tenant)
    .bind(property)
    .bind(code)
    .execute(pool)
    .await
    .unwrap();
    (tenant, property)
}

async fn site(pool: &PgPool, hotel: (Uuid, Uuid), enabled: bool, host: Option<&str>) {
    try_site(pool, hotel, enabled, host).await.unwrap();
}

async fn try_site(pool: &PgPool, hotel: (Uuid, Uuid), enabled: bool, host: Option<&str>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into ibe_site (tenant_id, property_id, enabled, custom_host, public_code)
         select $1, id, $3, $4, code from property where id = $2",
    )
    .bind(hotel.0)
    .bind(hotel.1)
    .bind(enabled)
    .bind(host)
    .execute(pool)
    .await
    .map(|_| ())
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn ibe_resolve_finds_an_enabled_site_by_code_in_any_case_or_by_host(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts.clone()).await.unwrap();
    let pool = app_pool(opts, 1).await;
    let galle = hotel(&owner, "GAL").await;
    site(&owner, galle, true, Some("book.galle.example")).await;

    for code in ["GAL", "gal", "Gal"] {
        let found = db::ibe_resolve(&pool, Some(code), None).await.unwrap();
        assert_eq!(found.map(|(tenant, property)| (tenant.0, property)), Some(galle), "code {code}");
    }
    let by_host = db::ibe_resolve(&pool, None, Some("book.galle.example")).await.unwrap();
    assert_eq!(by_host.map(|(tenant, property)| (tenant.0, property)), Some(galle));
    let by_host_in_caps = db::ibe_resolve(&pool, None, Some("Book.Galle.Example")).await.unwrap();
    assert!(by_host_in_caps.is_some());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn ibe_resolve_finds_nothing_for_a_disabled_site_or_an_unknown_code(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts.clone()).await.unwrap();
    let pool = app_pool(opts, 1).await;
    site(&owner, hotel(&owner, "OFF").await, false, Some("off.example")).await;
    hotel(&owner, "NOS").await;

    assert!(db::ibe_resolve(&pool, Some("OFF"), None).await.unwrap().is_none());
    assert!(db::ibe_resolve(&pool, None, Some("off.example")).await.unwrap().is_none());
    assert!(db::ibe_resolve(&pool, Some("NOS"), None).await.unwrap().is_none(), "no site row");
    assert!(db::ibe_resolve(&pool, Some("ZZZ"), Some("nowhere.example")).await.unwrap().is_none());
    assert!(db::ibe_resolve(&pool, None, None).await.unwrap().is_none());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_custom_host_is_lowercase_and_unique(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts).await.unwrap();
    let (a, b) = (hotel(&owner, "AAA").await, hotel(&owner, "BBB").await);
    site(&owner, a, true, Some("stay.example")).await;

    let upper =
        sqlx::query("insert into ibe_site (tenant_id, property_id, custom_host) values ($1, $2, 'Stay.Example')")
            .bind(b.0)
            .bind(b.1)
            .execute(&owner)
            .await;
    let taken =
        sqlx::query("insert into ibe_site (tenant_id, property_id, custom_host) values ($1, $2, 'stay.example')")
            .bind(b.0)
            .bind(b.1)
            .execute(&owner)
            .await;

    assert!(upper.is_err());
    assert!(taken.is_err());
}
