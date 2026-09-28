//! `guest_search` and `app.search_guest_ids`: the id-only path that lets guest name search use its trigram
//! index without bypassing row-level security. See `migrations/0009_guest_search.sql` and the "Guest search
//! without bypassing RLS" decision it implements.

use db::testing::app_pool;
use db::{Scope, TenantId, begin};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::time::Instant;
use uuid::Uuid;

async fn seed_tenant(pool: &PgPool) -> TenantId {
    let tenant = TenantId(Uuid::now_v7());
    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
    sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(tenant.0).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    tenant
}

async fn insert_guest(pool: &PgPool, tenant: TenantId, first: &str, last: &str) -> Uuid {
    let id = Uuid::now_v7();
    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
    sqlx::query(
        "insert into guest (id, tenant_id, first_name, last_name, residency) values ($1, $2, $3, $4, 'resident')",
    )
    .bind(id)
    .bind(tenant.0)
    .bind(first)
    .bind(last)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

/// The ids `app.search_guest_ids` returns for `query`, scoped to `tenant` (`None` leaves the setting unset).
async fn search_ids(pool: &PgPool, tenant: Option<TenantId>, query: &str) -> Vec<Uuid> {
    let scope = tenant.map(Scope::tenant).unwrap_or_default();
    let mut tx = begin(pool, scope).await.unwrap();
    let ids: Vec<Uuid> = sqlx::query_scalar("select guest_id from app.search_guest_ids($1, $2)")
        .bind(query)
        .bind(20_i32)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    ids
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_tenants_guest_is_never_returned(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool).await;
    let b = seed_tenant(&pool).await;
    insert_guest(&pool, a, "Ada", "Perera").await;
    let theirs = insert_guest(&pool, b, "Ada", "Perera").await;

    let seen_by_b = search_ids(&pool, Some(b), "ada perera").await;
    let seen_unset = search_ids(&pool, None, "ada perera").await;

    assert_eq!(seen_by_b, vec![theirs], "B sees only its own guest, never A's namesake");
    assert!(seen_unset.is_empty(), "no tenant set: nothing comes back");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_app_role_cannot_read_guest_search_directly(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts.clone(), 1).await;
    let owner = PgPool::connect_with(opts).await.unwrap();

    let result = sqlx::query("select 1 from guest_search limit 1").fetch_optional(&pool).await;
    let err = result.unwrap_err().to_string();
    assert!(err.contains("permission denied"), "unexpected error: {err}");

    // Not just SELECT: goodfolk_app has none of the CRUD privileges on guest_search at all (checked as the
    // superuser, since the app role's own denial above only proves SELECT is blocked).
    for privilege in ["SELECT", "INSERT", "UPDATE", "DELETE"] {
        let has: bool = sqlx::query_scalar("select has_table_privilege('goodfolk_app', 'guest_search', $1)")
            .bind(privilege)
            .fetch_one(&owner)
            .await
            .unwrap();
        assert!(!has, "goodfolk_app has {privilege} on guest_search");
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_rename_changes_search_results(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let tenant = seed_tenant(&pool).await;
    let guest = insert_guest(&pool, tenant, "Ada", "Perera").await;

    assert_eq!(search_ids(&pool, Some(tenant), "perera").await, vec![guest]);

    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    sqlx::query("update guest set last_name = 'Fernando' where id = $1").bind(guest).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();

    assert!(search_ids(&pool, Some(tenant), "perera").await.is_empty(), "the old name no longer matches");
    assert_eq!(search_ids(&pool, Some(tenant), "fernando").await, vec![guest], "the new name does");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_deletion_removes_the_guest_from_search(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let tenant = seed_tenant(&pool).await;
    let guest = insert_guest(&pool, tenant, "Ada", "Perera").await;
    assert_eq!(search_ids(&pool, Some(tenant), "perera").await, vec![guest]);

    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    sqlx::query("delete from guest where id = $1").bind(guest).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();

    assert!(search_ids(&pool, Some(tenant), "perera").await.is_empty());
}

/// A guest moved to another tenant -- done here by the owner/superuser pool, since `guest`'s row-level
/// security policy already has a `with check` that blocks the application role from changing `tenant_id` --
/// is no longer found by the old tenant's search, and is found by the new one's.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_tenant_change_moves_the_guest_between_tenants_search(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts.clone(), 1).await;
    let owner = PgPool::connect_with(opts).await.unwrap();
    let a = seed_tenant(&pool).await;
    let b = seed_tenant(&pool).await;
    let guest = insert_guest(&pool, a, "Ada", "Perera").await;
    assert_eq!(search_ids(&pool, Some(a), "perera").await, vec![guest]);

    sqlx::query("update guest set tenant_id = $1 where id = $2").bind(b.0).bind(guest).execute(&owner).await.unwrap();

    assert!(search_ids(&pool, Some(a), "perera").await.is_empty(), "tenant A no longer finds the moved guest");
    assert_eq!(search_ids(&pool, Some(b), "perera").await, vec![guest], "tenant B now does");
}

/// A change unrelated to the name (such as `notes`) must not need the trigger at all, but it must still leave
/// the guest searchable under its unchanged name.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_unrelated_update_leaves_search_results_alone(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let tenant = seed_tenant(&pool).await;
    let guest = insert_guest(&pool, tenant, "Ada", "Perera").await;

    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    sqlx::query("update guest set notes = 'VIP' where id = $1").bind(guest).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(search_ids(&pool, Some(tenant), "perera").await, vec![guest]);
}

/// Seeds 20k guests for one tenant, then: (1) `EXPLAIN`, run directly against `guest_search` as the table
/// owner (the same query `app.search_guest_ids` runs), shows a Bitmap Index Scan on its GIN index; (2) timed
/// as the app role, the old approach (a plain `<%` scan of `guest`, not leakproof, so row-level security keeps
/// it off any index) against the new one (through `app.search_guest_ids`).
///
/// Slow to set up, so it is `#[ignore]`d; run once with:
/// `DATABASE_URL=$TEST_DATABASE_URL cargo test -p db --test guest_search -- --ignored --nocapture`
#[sqlx::test(migrator = "db::MIGRATOR")]
#[ignore = "seeds 20k guests; run explicitly (see this test's doc comment)"]
async fn the_gin_index_serves_a_20k_guest_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    // `owner`: the role that ran the migrations (bypasses row-level security), for seeding and for EXPLAIN as
    // the table owner. `app`: goodfolk_app, subject to row-level security like the real application.
    let owner = PgPool::connect_with(opts.clone()).await.unwrap();
    let app = app_pool(opts, 1).await;
    let tenant = TenantId(Uuid::now_v7());
    sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(tenant.0).execute(&owner).await.unwrap();

    // 20k filler guests with varied names, plus one findable target, all seeded as the (superuser) owner pool
    // for speed -- it bypasses row-level security entirely, and the trigger fires per row regardless of who
    // inserts.
    sqlx::query(
        "insert into guest (id, tenant_id, first_name, last_name, residency)
         select gen_random_uuid(), $1, 'Guest' || g, 'Surname' || (g % 500), 'resident'
         from generate_series(1, 20000) as g",
    )
    .bind(tenant.0)
    .execute(&owner)
    .await
    .unwrap();
    let target = Uuid::now_v7();
    sqlx::query(
        "insert into guest (id, tenant_id, first_name, last_name, residency) values ($1, $2, 'Ann', 'Perera', 'resident')",
    )
    .bind(target)
    .bind(tenant.0)
    .execute(&owner)
    .await
    .unwrap();
    // The planner's row-count estimate for `guest_search` is stale (no autovacuum has run yet in this
    // throwaway database), so without this it under-costs a sequential scan and never reaches instead for
    // the index.
    sqlx::query("analyze guest_search").execute(&owner).await.unwrap();

    // (1) EXPLAIN, as the owner, on exactly the query `app.search_guest_ids` runs against `guest_search`.
    let plan: Vec<String> = sqlx::query_scalar(
        "explain select guest_id, similarity(lower($1), name) from guest_search
         where tenant_id = $2 and lower($1) <% name
         order by word_similarity(lower($1), name) desc, similarity(lower($1), name) desc, guest_id",
    )
    .bind("ann perera")
    .bind(tenant.0)
    .fetch_all(&owner)
    .await
    .unwrap();
    let plan_text = plan.join("\n");
    assert!(plan_text.contains("Bitmap Index Scan"), "no bitmap index scan in plan:\n{plan_text}");
    assert!(plan_text.contains("guest_search_tenant_name_idx"), "GIN index not used:\n{plan_text}");

    // (2) Timed as the app role: the old, scanning approach vs. the new, index-served one.
    let mut tx = begin(&app, Scope::tenant(tenant)).await.unwrap();
    let scan_started = Instant::now();
    let scanned: Vec<Uuid> = sqlx::query_scalar(
        "select id from guest where lower('ann perera') <% lower(first_name || ' ' || last_name) limit 20",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    let scan_elapsed = scan_started.elapsed();

    let indexed_started = Instant::now();
    let found: Vec<Uuid> = sqlx::query_scalar("select guest_id from app.search_guest_ids($1, 20)")
        .bind("ann perera")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    let indexed_elapsed = indexed_started.elapsed();
    tx.commit().await.unwrap();

    println!(
        "guest search, 20k guests in one tenant: old scan {scan_elapsed:?}, app.search_guest_ids {indexed_elapsed:?}"
    );
    assert_eq!(scanned, vec![target]);
    assert_eq!(found, vec![target]);
}
