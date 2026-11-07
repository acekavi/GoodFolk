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

fn constraint(result: Result<(), sqlx::Error>) -> Option<String> {
    result.err().and_then(|err| err.as_database_error().and_then(|db| db.constraint()).map(str::to_owned))
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_custom_host_is_a_real_host_name(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts).await.unwrap();
    let a = hotel(&owner, "AAA").await;

    for bad in ["...", "-a-.com", "localhost", "a..b", "a.b.", &format!("{}.com", "x".repeat(64))] {
        assert!(try_site(&owner, a, false, Some(bad)).await.is_err(), "{bad}");
    }
    assert!(try_site(&owner, a, false, Some("book.galleforthotel.lk")).await.is_ok());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn two_tenants_with_one_property_code_cannot_both_enable_their_site(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts).await.unwrap();
    let (first, second, third) = (hotel(&owner, "GAL").await, hotel(&owner, "GAL").await, hotel(&owner, "GAL").await);

    let enabled = try_site(&owner, first, true, None).await;
    let clash = try_site(&owner, second, true, None).await;
    let disabled = try_site(&owner, third, false, None).await;

    assert!(enabled.is_ok());
    assert_eq!(constraint(clash).as_deref(), Some("ibe_site_public_code_idx"));
    assert!(disabled.is_ok(), "a disabled site does not hold the code");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn ibe_resolve_by_code_returns_the_one_enabled_site_of_a_shared_code(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts.clone()).await.unwrap();
    let pool = app_pool(opts, 1).await;
    let (first, second) = (hotel(&owner, "GAL").await, hotel(&owner, "GAL").await);
    site(&owner, second, false, None).await;
    site(&owner, first, true, None).await;

    for code in ["GAL", "gal"] {
        let found = db::ibe_resolve(&pool, Some(code), None).await.unwrap();
        assert_eq!(found.map(|(tenant, property)| (tenant.0, property)), Some(first), "{code}");
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_host_lookup_ignores_the_code_and_a_code_lookup_ignores_hosts(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts.clone()).await.unwrap();
    let pool = app_pool(opts, 1).await;
    let (galle, kandy) = (hotel(&owner, "GAL").await, hotel(&owner, "KAN").await);
    site(&owner, galle, true, Some("book.galle.example")).await;
    site(&owner, kandy, true, Some("book.kandy.example")).await;

    let other_code = db::ibe_resolve(&pool, Some("GAL"), Some("book.kandy.example")).await.unwrap();
    let unknown_code = db::ibe_resolve(&pool, Some("ZZZ"), Some("book.galle.example")).await.unwrap();
    let unknown_host = db::ibe_resolve(&pool, Some("GAL"), Some("nowhere.example")).await.unwrap();
    let code_alone = db::ibe_resolve(&pool, Some("KAN"), None).await.unwrap();
    let hosts_never_match_a_code = db::ibe_resolve(&pool, Some("book.galle.example"), None).await.unwrap();

    assert_eq!(other_code.map(|(_, property)| property), Some(kandy.1));
    assert_eq!(unknown_code.map(|(_, property)| property), Some(galle.1));
    assert!(unknown_host.is_none(), "the host decides, whatever the code");
    assert_eq!(code_alone.map(|(_, property)| property), Some(kandy.1));
    assert!(hosts_never_match_a_code.is_none());
}
