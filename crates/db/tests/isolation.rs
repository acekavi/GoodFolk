//! Tenant isolation suite. Every tenant-scoped table added later gets a case here.

use db::testing::app_pool;
use db::{Scope, TenantId, UserId, begin};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{AssertSqlSafe, PgPool};
use uuid::Uuid;

async fn seed_tenant(pool: &PgPool, name: &str) -> TenantId {
    let tenant = TenantId(Uuid::now_v7());
    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
    sqlx::query("insert into tenant (id, name) values ($1, $2)")
        .bind(tenant.0)
        .bind(name)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "insert into property (id, tenant_id, code, name, timezone, base_currency, business_date)
         values ($1, $2, 'MAIN', $3, 'Asia/Colombo', 'LKR', current_date)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(format!("{name} Hotel"))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    tenant
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_tenant_sees_only_its_own_rows(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool, "A").await;
    let b = seed_tenant(&pool, "B").await;

    let mut tx = begin(&pool, Scope::tenant(a)).await.unwrap();
    let tenants: Vec<Uuid> = sqlx::query_scalar("select id from tenant").fetch_all(&mut *tx).await.unwrap();
    let properties: Vec<Uuid> = sqlx::query_scalar("select tenant_id from property").fetch_all(&mut *tx).await.unwrap();

    assert_eq!(tenants, vec![a.0]);
    assert_eq!(properties, vec![a.0]);
    assert!(!properties.contains(&b.0));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn no_scope_sees_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    seed_tenant(&pool, "A").await;

    let mut tx = begin(&pool, Scope::default()).await.unwrap();
    let count: i64 = sqlx::query_scalar("select count(*) from property").fetch_one(&mut *tx).await.unwrap();

    assert_eq!(count, 0);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn writing_into_another_tenant_is_rejected(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool, "A").await;
    let b = seed_tenant(&pool, "B").await;

    let mut tx = begin(&pool, Scope::tenant(a)).await.unwrap();
    let result = sqlx::query(
        "insert into property (id, tenant_id, code, name, timezone, base_currency, business_date)
         values ($1, $2, 'X1', 'Intruder', 'Asia/Colombo', 'LKR', current_date)",
    )
    .bind(Uuid::now_v7())
    .bind(b.0)
    .execute(&mut *tx)
    .await;

    let err = result.unwrap_err().to_string();
    assert!(err.contains("row-level security"), "unexpected error: {err}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn tenant_context_does_not_leak_between_transactions(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool, "A").await;

    begin(&pool, Scope::tenant(a)).await.unwrap().commit().await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let setting: Option<String> = sqlx::query_scalar("select nullif(current_setting('app.tenant_id', true), '')")
        .fetch_one(&mut *conn)
        .await
        .unwrap();

    assert_eq!(setting, None);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_user_sees_tenants_they_belong_to_but_cannot_join_others(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool, "A").await;
    let b = seed_tenant(&pool, "B").await;
    let user = UserId(Uuid::now_v7());
    let mut tx = begin(&pool, Scope::tenant(a)).await.unwrap();
    sqlx::query("insert into app_user (id, email, password_hash, display_name) values ($1, 'u@example.com', 'x', 'U')")
        .bind(user.0)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("insert into membership (tenant_id, user_id) values ($1, $2)")
        .bind(a.0)
        .bind(user.0)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = begin(&pool, Scope::user(user)).await.unwrap();
    let visible: Vec<Uuid> = sqlx::query_scalar("select id from tenant").fetch_all(&mut *tx).await.unwrap();
    let join = sqlx::query("insert into membership (tenant_id, user_id) values ($1, $2)")
        .bind(b.0)
        .bind(user.0)
        .execute(&mut *tx)
        .await;

    assert_eq!(visible, vec![a.0]);
    assert!(join.is_err());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn audit_log_is_append_only(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool, "A").await;
    let mut tx = begin(&pool, Scope::tenant(a)).await.unwrap();
    sqlx::query("insert into audit_log (id, tenant_id, action, entity) values ($1, $2, 'test', 'tenant')")
        .bind(Uuid::now_v7())
        .bind(a.0)
        .execute(&mut *tx)
        .await
        .unwrap();

    let delete = sqlx::query("delete from audit_log").execute(&mut *tx).await;

    assert!(delete.is_err());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn role_grants_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool, "A").await;
    let b = seed_tenant(&pool, "B").await;
    let user = UserId(Uuid::now_v7());
    let mut tx = begin(&pool, Scope::tenant(a)).await.unwrap();
    sqlx::query("insert into app_user (id, email, password_hash, display_name) values ($1, 'u@example.com', 'x', 'U')")
        .bind(user.0)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("insert into membership (tenant_id, user_id) values ($1, $2)")
        .bind(a.0)
        .bind(user.0)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("insert into role_grant (id, tenant_id, user_id, property_id, role) values ($1, $2, $3, $4, $5)")
        .bind(Uuid::now_v7())
        .bind(a.0)
        .bind(user.0)
        .bind::<Option<Uuid>>(None)
        .bind("owner")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = begin(&pool, Scope::tenant(b)).await.unwrap();
    let count: i64 = sqlx::query_scalar("select count(*) from role_grant").fetch_one(&mut *tx).await.unwrap();
    let result =
        sqlx::query("insert into role_grant (id, tenant_id, user_id, property_id, role) values ($1, $2, $3, $4, $5)")
            .bind(Uuid::now_v7())
            .bind(a.0)
            .bind(user.0)
            .bind::<Option<Uuid>>(None)
            .bind("owner")
            .execute(&mut *tx)
            .await;

    assert_eq!(count, 0);
    let err = result.unwrap_err().to_string();
    assert!(err.contains("row-level security"), "unexpected error: {err}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn idempotency_keys_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool, "A").await;
    let b = seed_tenant(&pool, "B").await;
    let mut tx = begin(&pool, Scope::tenant(a)).await.unwrap();
    sqlx::query("insert into idempotency_key (tenant_id, key, request_hash) values ($1, $2, $3)")
        .bind(a.0)
        .bind("key-00000001")
        .bind(vec![1u8])
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = begin(&pool, Scope::tenant(b)).await.unwrap();
    let count: i64 = sqlx::query_scalar("select count(*) from idempotency_key").fetch_one(&mut *tx).await.unwrap();
    let result = sqlx::query("insert into idempotency_key (tenant_id, key, request_hash) values ($1, $2, $3)")
        .bind(a.0)
        .bind("key-00000002")
        .bind(vec![2u8])
        .execute(&mut *tx)
        .await;

    assert_eq!(count, 0);
    let err = result.unwrap_err().to_string();
    assert!(err.contains("row-level security"), "unexpected error: {err}");
}

/// One row in every Phase 1 table, for tenant `a`'s property.
struct RoomsSeed {
    property: Uuid,
    room_type: Uuid,
    section: Uuid,
    room: Uuid,
    reason: Uuid,
}

async fn seed_rooms(pool: &PgPool, tenant: TenantId) -> RoomsSeed {
    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
    let property: Uuid = sqlx::query_scalar("select id from property").fetch_one(&mut *tx).await.unwrap();
    let seed = RoomsSeed {
        property,
        room_type: Uuid::now_v7(),
        section: Uuid::now_v7(),
        room: Uuid::now_v7(),
        reason: Uuid::now_v7(),
    };
    sqlx::query(
        "insert into room_type (id, tenant_id, property_id, code, name, base_occupancy, max_adults, max_children, max_occupancy)
         values ($1, $2, $3, 'DLX', 'Deluxe', 2, 2, 1, 3)",
    )
    .bind(seed.room_type)
    .bind(tenant.0)
    .bind(property)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("insert into housekeeping_section (id, tenant_id, property_id, name) values ($1, $2, $3, 'East')")
        .bind(seed.section)
        .bind(tenant.0)
        .bind(property)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "insert into room (id, tenant_id, property_id, room_type_id, number, section_id) values ($1, $2, $3, $4, '101', $5)",
    )
    .bind(seed.room)
    .bind(tenant.0)
    .bind(property)
    .bind(seed.room_type)
    .bind(seed.section)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "insert into block_reason (id, tenant_id, property_id, code, label, default_kind)
         values ($1, $2, $3, 'LEAK', 'Leak', 'out_of_order')",
    )
    .bind(seed.reason)
    .bind(tenant.0)
    .bind(property)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "insert into room_block (id, tenant_id, property_id, room_id, period, kind, reason_id)
         values ($1, $2, $3, $4, daterange(current_date, current_date + 3), 'out_of_order', $5)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(seed.room)
    .bind(seed.reason)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "insert into inventory_day (tenant_id, property_id, room_type_id, date, physical) values ($1, $2, $3, current_date, 1)",
    )
    .bind(tenant.0)
    .bind(property)
    .bind(seed.room_type)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    seed
}

/// How many rows of `table` tenant `viewer` can see.
async fn visible_rows(pool: &PgPool, viewer: TenantId, table: &str) -> i64 {
    let mut tx = begin(pool, Scope::tenant(viewer)).await.unwrap();
    sqlx::query_scalar(AssertSqlSafe(format!("select count(*) from {table}"))).fetch_one(&mut *tx).await.unwrap()
}

/// Runs `insert` (with `$1` bound to the owning tenant) as tenant `writer`, and returns the error message.
async fn foreign_insert_error(pool: &PgPool, writer: TenantId, owner: TenantId, insert: &'static str) -> String {
    let mut tx = begin(pool, Scope::tenant(writer)).await.unwrap();
    sqlx::query(insert).bind(owner.0).execute(&mut *tx).await.unwrap_err().to_string()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn room_types_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool, "A").await;
    let b = seed_tenant(&pool, "B").await;
    seed_rooms(&pool, a).await;

    let seen = visible_rows(&pool, b, "room_type").await;
    let err = foreign_insert_error(
        &pool,
        b,
        a,
        "insert into room_type (id, tenant_id, property_id, code, name, base_occupancy, max_adults, max_children, max_occupancy)
         select gen_random_uuid(), $1, id, 'STD', 'Standard', 1, 1, 0, 1 from property limit 1",
    )
    .await;

    assert_eq!(visible_rows(&pool, a, "room_type").await, 1);
    assert_eq!(seen, 0);
    assert!(err.contains("row-level security"), "unexpected error: {err}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn housekeeping_sections_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool, "A").await;
    let b = seed_tenant(&pool, "B").await;
    seed_rooms(&pool, a).await;

    let seen = visible_rows(&pool, b, "housekeeping_section").await;
    let err = foreign_insert_error(
        &pool,
        b,
        a,
        "insert into housekeeping_section (id, tenant_id, property_id, name)
         select gen_random_uuid(), $1, id, 'West' from property limit 1",
    )
    .await;

    assert_eq!(visible_rows(&pool, a, "housekeeping_section").await, 1);
    assert_eq!(seen, 0);
    assert!(err.contains("row-level security"), "unexpected error: {err}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn rooms_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool, "A").await;
    let b = seed_tenant(&pool, "B").await;
    seed_rooms(&pool, a).await;
    let theirs = seed_rooms(&pool, b).await;

    let seen = visible_rows(&pool, b, "room").await;
    let mut tx = begin(&pool, Scope::tenant(b)).await.unwrap();
    let err = sqlx::query(
        "insert into room (id, tenant_id, property_id, room_type_id, number) values (gen_random_uuid(), $1, $2, $3, '102')",
    )
    .bind(a.0)
    .bind(theirs.property)
    .bind(theirs.room_type)
    .execute(&mut *tx)
    .await
    .unwrap_err()
    .to_string();

    assert_eq!(seen, 1, "B sees only its own room");
    assert!(err.contains("row-level security"), "unexpected error: {err}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn block_reasons_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool, "A").await;
    let b = seed_tenant(&pool, "B").await;
    seed_rooms(&pool, a).await;

    let seen = visible_rows(&pool, b, "block_reason").await;
    let err = foreign_insert_error(
        &pool,
        b,
        a,
        "insert into block_reason (id, tenant_id, property_id, code, label, default_kind)
         select gen_random_uuid(), $1, id, 'PAINT', 'Painting', 'out_of_order' from property limit 1",
    )
    .await;

    assert_eq!(seen, 0);
    assert!(err.contains("row-level security"), "unexpected error: {err}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn room_blocks_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool, "A").await;
    let b = seed_tenant(&pool, "B").await;
    seed_rooms(&pool, a).await;
    let theirs = seed_rooms(&pool, b).await;

    let seen = visible_rows(&pool, b, "room_block").await;
    let mut tx = begin(&pool, Scope::tenant(b)).await.unwrap();
    let err = sqlx::query(
        "insert into room_block (id, tenant_id, property_id, room_id, period, kind, reason_id)
         values (gen_random_uuid(), $1, $2, $3, daterange(current_date + 10, current_date + 12), 'out_of_order', $4)",
    )
    .bind(a.0)
    .bind(theirs.property)
    .bind(theirs.room)
    .bind(theirs.reason)
    .execute(&mut *tx)
    .await
    .unwrap_err()
    .to_string();

    assert_eq!(seen, 1, "B sees only its own block");
    assert!(err.contains("row-level security"), "unexpected error: {err}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn inventory_days_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool, "A").await;
    let b = seed_tenant(&pool, "B").await;
    let ours = seed_rooms(&pool, a).await;

    let seen = visible_rows(&pool, b, "inventory_day").await;
    let mut tx = begin(&pool, Scope::tenant(b)).await.unwrap();
    let err = sqlx::query(
        "insert into inventory_day (tenant_id, property_id, room_type_id, date) values ($1, $2, $3, current_date + 1)",
    )
    .bind(a.0)
    .bind(ours.property)
    .bind(ours.room_type)
    .execute(&mut *tx)
    .await
    .unwrap_err()
    .to_string();

    assert_eq!(seen, 0);
    assert!(err.contains("row-level security"), "unexpected error: {err}");
}

/// One row in every Phase 2 table, on top of [`seed_rooms`], for `tenant`'s property.
async fn seed_rates(pool: &PgPool, tenant: TenantId) {
    let rooms = seed_rooms(pool, tenant).await;
    let (policy, plan) = (Uuid::now_v7(), Uuid::now_v7());
    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
    let statements = [
        "insert into cancellation_policy (id, tenant_id, property_id, name, rules, no_show)
         values ($4, $1, $2, 'Flexible', '[]', '{\"kind\": \"nights\", \"value\": 1}')",
        "insert into rate_plan (id, tenant_id, property_id, code, name, kind, segment, currency, cancellation_policy_id)
         values ($5, $1, $2, 'BAR', 'Best available', 'standard', 'IBE', 'USD', $4)",
        "insert into rate_plan_room_type (tenant_id, property_id, rate_plan_id, room_type_id) values ($1, $2, $5, $3)",
        "insert into rate_day (tenant_id, property_id, rate_plan_id, room_type_id, date, occupancy, amount)
         values ($1, $2, $5, $3, current_date, 2, 12000)",
        "insert into rate_restriction (tenant_id, property_id, rate_plan_id, room_type_id, date, min_stay)
         values ($1, $2, $5, $3, current_date, 2)",
        "insert into meal_supplement (id, tenant_id, property_id, meal_plan, currency, adult_amount, child_amount, valid)
         values (gen_random_uuid(), $1, $2, 'BB', 'USD', 1500, 750, daterange(current_date, null))",
    ];
    for statement in statements {
        sqlx::query(statement)
            .bind(tenant.0)
            .bind(rooms.property)
            .bind(rooms.room_type)
            .bind(policy)
            .bind(plan)
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
}

/// Which phase's fixture to give each tenant in [`assert_table_isolated`], before checking isolation. Each
/// variant builds on the one before, so a later phase's seed covers the earlier tables too. Plain enum instead
/// of a generic seed function/closure parameter: async fn items don't implement `Fn` generally enough over the
/// pool's borrowed lifetime (a higher-ranked trait bound issue), and an enum sidesteps it entirely.
enum Seed {
    Rates,
    Reservations,
    AccountsAndOccupants,
    Housekeeping,
    Media,
    Folio,
}

impl Seed {
    async fn apply(&self, pool: &PgPool, tenant: TenantId) {
        match self {
            Seed::Rates => seed_rates(pool, tenant).await,
            Seed::Reservations => seed_reservations(pool, tenant).await,
            Seed::AccountsAndOccupants => seed_accounts_and_occupants(pool, tenant).await,
            Seed::Housekeeping => seed_housekeeping(pool, tenant).await,
            Seed::Media => seed_media(pool, tenant).await,
            Seed::Folio => seed_folio(pool, tenant).await,
        }
    }
}

/// Tenant `b` sees none of `a`'s rows in `table`, only its own, and cannot insert a row owned by `a` (`insert`
/// binds `$1` to `a` and takes every other id from `b`'s own rows). `seed` seeds one tenant with every row its
/// phase's tables need.
async fn assert_table_isolated(opts: PgConnectOptions, table: &str, insert: &'static str, seed: Seed) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool, "A").await;
    let b = seed_tenant(&pool, "B").await;
    seed.apply(&pool, a).await;
    seed.apply(&pool, b).await;

    let seen = visible_rows(&pool, b, table).await;
    let err = foreign_insert_error(&pool, b, a, insert).await;

    assert_eq!(seen, 1, "B sees only its own {table} row");
    assert!(err.contains("row-level security"), "unexpected error: {err}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn cancellation_policies_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "cancellation_policy",
        "insert into cancellation_policy (id, tenant_id, property_id, name, rules, no_show)
         select gen_random_uuid(), $1, id, 'Strict', '[]', '{}' from property",
        Seed::Rates,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn rate_plans_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "rate_plan",
        "insert into rate_plan (id, tenant_id, property_id, code, name, kind, segment, currency)
         select gen_random_uuid(), $1, id, 'OTA', 'OTA', 'custom', 'OTA', 'USD' from property",
        Seed::Rates,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn rate_plan_room_types_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "rate_plan_room_type",
        "insert into rate_plan_room_type (tenant_id, property_id, rate_plan_id, room_type_id)
         select $1, p.property_id, p.id, t.id from rate_plan p, room_type t",
        Seed::Rates,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn rate_days_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "rate_day",
        "insert into rate_day (tenant_id, property_id, rate_plan_id, room_type_id, date, occupancy, amount)
         select $1, property_id, rate_plan_id, room_type_id, current_date + 1, 1, 9000 from rate_plan_room_type",
        Seed::Rates,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn rate_restrictions_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "rate_restriction",
        "insert into rate_restriction (tenant_id, property_id, rate_plan_id, room_type_id, date, closed)
         select $1, property_id, rate_plan_id, room_type_id, current_date + 1, true from rate_plan_room_type",
        Seed::Rates,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn meal_supplements_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "meal_supplement",
        "insert into meal_supplement (id, tenant_id, property_id, meal_plan, currency, adult_amount, child_amount, valid)
         select gen_random_uuid(), $1, id, 'HB', 'USD', 3000, 1500, daterange(current_date, null) from property",
        Seed::Rates,
    )
    .await;
}

/// One row in every Phase 3 table, on top of [`seed_rates`], for `tenant`'s property: a guest booking room 101
/// for two nights, with the first night's price.
async fn seed_reservations(pool: &PgPool, tenant: TenantId) {
    seed_rates(pool, tenant).await;
    let (guest, reservation) = (Uuid::now_v7(), Uuid::now_v7());
    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
    let statements = [
        "insert into guest (id, tenant_id, first_name, last_name, residency) values ($2, $1, 'Ada', 'Silva', 'resident')",
        "insert into property_counter (tenant_id, property_id, name, value) select $1, id, 'confirmation', 1 from property",
        "insert into reservation (id, tenant_id, property_id, confirmation_no, source, booker_guest_id)
         select $3, $1, id, 'MAIN-000001', 'front_desk', $2 from property",
        "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, room_id, stay, adults,
                                       children, rate_plan_id, meal_plan, status, primary_guest_id, currency)
         select gen_random_uuid(), $1, r.property_id, $3, r.room_type_id, r.id, daterange(current_date, current_date + 2),
                2, 0, p.id, 'RO', 'confirmed', $2, p.currency
         from room r, rate_plan p",
        "insert into reservation_night (tenant_id, property_id, reservation_room_id, date, room_amount, meal_amount, currency)
         select $1, property_id, id, lower(stay), 12000, 0, currency from reservation_room",
    ];
    for statement in statements {
        sqlx::query(statement).bind(tenant.0).bind(guest).bind(reservation).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
}

/// Guests belong to the tenant, not to a property: the policy is the tenant's alone.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn guests_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "guest",
        "insert into guest (id, tenant_id, first_name, last_name, residency)
         values (gen_random_uuid(), $1, 'Eve', 'Perera', 'non_resident')",
        Seed::Reservations,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn property_counters_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "property_counter",
        "insert into property_counter (tenant_id, property_id, name, value)
         select $1, property_id, name, value + 1 from property_counter",
        Seed::Reservations,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn reservations_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "reservation",
        "insert into reservation (id, tenant_id, property_id, confirmation_no, source, booker_guest_id)
         select gen_random_uuid(), $1, property_id, 'MAIN-000002', 'phone', booker_guest_id from reservation",
        Seed::Reservations,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn reservation_rooms_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "reservation_room",
        "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, stay, adults, children,
                                       rate_plan_id, meal_plan, status, primary_guest_id, currency)
         select gen_random_uuid(), $1, property_id, reservation_id, room_type_id, daterange(current_date + 5, current_date + 6),
                1, 0, rate_plan_id, 'RO', 'confirmed', primary_guest_id, currency
         from reservation_room",
        Seed::Reservations,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn reservation_nights_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "reservation_night",
        "insert into reservation_night (tenant_id, property_id, reservation_room_id, date, room_amount, meal_amount, currency)
         select $1, property_id, id, lower(stay) + 1, 12000, 0, currency from reservation_room",
        Seed::Reservations,
    )
    .await;
}

/// On top of [`seed_reservations`], for `tenant`: an account billed on the reservation, and a second guest
/// added to its room as an additional occupant (3b).
async fn seed_accounts_and_occupants(pool: &PgPool, tenant: TenantId) {
    seed_reservations(pool, tenant).await;
    let (account, occupant) = (Uuid::now_v7(), Uuid::now_v7());
    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
    let statements = [
        "insert into account (id, tenant_id, kind, name, currency) values ($2, $1, 'company', 'Acme Corp', 'USD')",
        "update reservation set account_id = $2 where tenant_id = $1",
        "insert into guest (id, tenant_id, first_name, last_name, residency) values ($3, $1, 'Nadia', 'Fernando', 'resident')",
        "insert into reservation_guest (tenant_id, property_id, reservation_room_id, guest_id)
         select $1, property_id, id, $3 from reservation_room where tenant_id = $1",
    ];
    for statement in statements {
        sqlx::query(statement).bind(tenant.0).bind(account).bind(occupant).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn accounts_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "account",
        "insert into account (id, tenant_id, kind, name, currency)
         values (gen_random_uuid(), $1, 'travel_agent', 'Intruder Travel', 'USD')",
        Seed::AccountsAndOccupants,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn reservation_guests_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "reservation_guest",
        "insert into reservation_guest (tenant_id, property_id, reservation_room_id, guest_id)
         select $1, property_id, reservation_room_id, guest_id from reservation_guest",
        Seed::AccountsAndOccupants,
    )
    .await;
}

/// One room condition, one assignment and one open issue on top of [`seed_rooms`], for `tenant`'s room (5).
async fn seed_housekeeping(pool: &PgPool, tenant: TenantId) {
    seed_rooms(pool, tenant).await;
    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
    let statements = [
        "insert into room_condition (room_id, tenant_id, property_id)
         select id, tenant_id, property_id from room where tenant_id = $1",
        "insert into hk_assignment (tenant_id, property_id, business_date, room_id, user_id)
         select tenant_id, property_id, current_date, id, gen_random_uuid() from room where tenant_id = $1",
        "insert into room_issue (id, tenant_id, property_id, room_id, severity, description, reported_by)
         select gen_random_uuid(), tenant_id, property_id, id, 'low', 'Dripping tap', gen_random_uuid()
         from room where tenant_id = $1",
    ];
    for statement in statements {
        sqlx::query(statement).bind(tenant.0).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn room_conditions_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "room_condition",
        "insert into room_condition (room_id, tenant_id, property_id)
         select gen_random_uuid(), $1, property_id from room_condition",
        Seed::Housekeeping,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn hk_assignments_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "hk_assignment",
        "insert into hk_assignment (tenant_id, property_id, business_date, room_id, user_id)
         select $1, property_id, current_date + 1, room_id, gen_random_uuid() from hk_assignment",
        Seed::Housekeeping,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn room_issues_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "room_issue",
        "insert into room_issue (id, tenant_id, property_id, room_id, severity, description, reported_by)
         select gen_random_uuid(), $1, property_id, room_id, 'low', 'Intruder', gen_random_uuid() from room_issue",
        Seed::Housekeeping,
    )
    .await;
}

/// On top of [`seed_housekeeping`], for `tenant`: one asset linked to the room type's gallery, the room's issue and a
/// guest's document (6a).
async fn seed_media(pool: &PgPool, tenant: TenantId) {
    seed_housekeeping(pool, tenant).await;
    let (asset, guest) = (Uuid::now_v7(), Uuid::now_v7());
    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
    let statements = [
        "insert into media_asset (id, tenant_id, property_id, purpose, visibility, filename, declared_bytes, sha256)
         select $2, $1, id, 'gallery', 'public', 'room.jpg', 1000, sha256('x') from property",
        "insert into room_type_media (tenant_id, room_type_id, asset_id, position) select $1, id, $2, 0 from room_type",
        "insert into room_issue_media (tenant_id, issue_id, asset_id) select $1, id, $2 from room_issue",
        "insert into guest (id, tenant_id, first_name, last_name, residency) values ($3, $1, 'Ada', 'Silva', 'resident')",
        "insert into guest_document (id, tenant_id, guest_id, asset_id, kind) values (gen_random_uuid(), $1, $3, $2, 'nic')",
    ];
    for statement in statements {
        sqlx::query(statement).bind(tenant.0).bind(asset).bind(guest).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn media_assets_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "media_asset",
        "insert into media_asset (id, tenant_id, property_id, purpose, visibility, filename, declared_bytes, sha256)
         select gen_random_uuid(), $1, property_id, 'gallery', 'public', 'x.jpg', 1, sha256('y') from media_asset",
        Seed::Media,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn room_type_media_is_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "room_type_media",
        "insert into room_type_media (tenant_id, room_type_id, asset_id, position)
         select $1, room_type_id, asset_id, 1 from room_type_media",
        Seed::Media,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn room_issue_media_is_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "room_issue_media",
        "insert into room_issue_media (tenant_id, issue_id, asset_id) select $1, issue_id, asset_id from room_issue_media",
        Seed::Media,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn guest_documents_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "guest_document",
        "insert into guest_document (id, tenant_id, guest_id, asset_id, kind)
         select gen_random_uuid(), $1, guest_id, asset_id, 'passport' from guest_document",
        Seed::Media,
    )
    .await;
}

/// On top of [`seed_accounts_and_occupants`], for `tenant`: one row in every Phase 7 table. The guest folio carries a
/// room line with a tax component, a payment, an invoice and a POS bill; the account is a routing target.
async fn seed_folio(pool: &PgPool, tenant: TenantId) {
    seed_accounts_and_occupants(pool, tenant).await;
    let (folio, line, payment, outlet) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
    let statements = [
        "insert into tax_rule (id, tenant_id, property_id, code, name, kind, rate_bp, applies_to, base, sequence,
                               presentation, effective_from)
         select gen_random_uuid(), $1, id, 'VAT', 'Value added tax', 'tax', 1800, array['*'], array['net'], 40,
                'on_bill', date '2026-01-01'
         from property",
        "insert into outlet (id, tenant_id, property_id, code, name) select $5, $1, id, 'BAR', 'Bar' from property",
        "insert into fx_rate (tenant_id, property_id, date, currency, rate_to_base, source, source_date)
         select $1, id, current_date, 'USD', 300.5, 'cbsl', current_date from property",
        "insert into fx_alert (property_id, tenant_id, message) select id, $1, 'No rate' from property",
        "insert into folio (id, tenant_id, property_id, kind, reservation_room_id)
         select $2, $1, property_id, 'guest', id from reservation_room",
        "insert into folio_line (id, tenant_id, folio_id, source, category, description, net, total, currency, base_total,
                                 rate, rate_date, business_date)
         values ($3, $1, $2, 'room', 'room', 'Room 101', 10000, 11800, 'LKR', 11800, 1, current_date, current_date)",
        "insert into folio_line_tax (tenant_id, line_id, code, name, kind, rate_bp, base, amount, presentation)
         values ($1, $3, 'VAT', 'Value added tax', 'tax', 1800, 10000, 1800, 'on_bill')",
        "insert into payment (id, tenant_id, folio_id, method, amount, currency, base_amount, rate, rate_date,
                              business_date)
         values ($4, $1, $2, 'cash', 11800, 'LKR', 11800, 1, current_date, current_date)",
        "insert into routing_rule (tenant_id, reservation_id, category, account_id)
         select $1, r.id, 'laundry', a.id from reservation r, account a",
        "insert into pos_bill (id, tenant_id, property_id, outlet_id, bill_no, business_date, body_hash, settled)
         select gen_random_uuid(), $1, property_id, id, 'B-1', current_date, sha256('x'), false from outlet",
        "insert into invoice (id, tenant_id, property_id, folio_id, kind, serial, data, total_lkr)
         select gen_random_uuid(), $1, property_id, id, 'invoice', 'MAIN-2026-000001', '{}', 11800 from folio",
    ];
    for statement in statements {
        sqlx::query(statement)
            .bind(tenant.0)
            .bind(folio)
            .bind(line)
            .bind(payment)
            .bind(outlet)
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn tax_rules_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "tax_rule",
        "insert into tax_rule (id, tenant_id, property_id, code, name, kind, rate_bp, applies_to, base, sequence,
                               presentation, effective_from)
         select gen_random_uuid(), $1, property_id, 'XTAX', 'Intruder', 'tax', 1, array['*'], array['net'], 50,
                'on_bill', date '2026-01-01'
         from tax_rule",
        Seed::Folio,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn outlets_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "outlet",
        "insert into outlet (id, tenant_id, property_id, code, name) select gen_random_uuid(), $1, property_id, 'SPA', 'Spa' from outlet",
        Seed::Folio,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn fx_rates_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "fx_rate",
        "insert into fx_rate (tenant_id, property_id, date, currency, rate_to_base, source, source_date)
         select $1, property_id, date, 'EUR', 330, 'manual', date from fx_rate",
        Seed::Folio,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn fx_alerts_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "fx_alert",
        "insert into fx_alert (property_id, tenant_id, message) select gen_random_uuid(), $1, 'x' from fx_alert",
        Seed::Folio,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn folios_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "folio",
        "insert into folio (id, tenant_id, property_id, kind, account_id)
         select gen_random_uuid(), $1, property_id, 'account', a.id from folio, account a",
        Seed::Folio,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn folio_lines_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "folio_line",
        "insert into folio_line (id, tenant_id, folio_id, source, category, description, net, total, currency, base_total,
                                 rate, rate_date, business_date)
         select gen_random_uuid(), $1, folio_id, 'manual', 'misc', 'Intruder', 1, 1, 'LKR', 1, 1, current_date, current_date
         from folio_line",
        Seed::Folio,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn folio_line_taxes_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "folio_line_tax",
        "insert into folio_line_tax (tenant_id, line_id, code, name, kind, rate_bp, base, amount, presentation)
         select $1, line_id, 'XTAX', 'Intruder', 'tax', 1, 1, 1, 'on_bill' from folio_line_tax",
        Seed::Folio,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn payments_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "payment",
        "insert into payment (id, tenant_id, folio_id, method, amount, currency, base_amount, rate, rate_date, business_date)
         select gen_random_uuid(), $1, folio_id, 'cash', 1, 'LKR', 1, 1, current_date, current_date from payment",
        Seed::Folio,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn routing_rules_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "routing_rule",
        "insert into routing_rule (tenant_id, reservation_id, category, account_id)
         select $1, reservation_id, 'misc', account_id from routing_rule",
        Seed::Folio,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn pos_bills_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "pos_bill",
        "insert into pos_bill (id, tenant_id, property_id, outlet_id, bill_no, business_date, body_hash, settled)
         select gen_random_uuid(), $1, property_id, outlet_id, 'B-2', business_date, body_hash, settled from pos_bill",
        Seed::Folio,
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn invoices_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_table_isolated(
        opts,
        "invoice",
        "insert into invoice (id, tenant_id, property_id, folio_id, kind, serial, data, total_lkr)
         select gen_random_uuid(), $1, property_id, folio_id, 'invoice', 'MAIN-2026-000002', '{}', 1 from invoice",
        Seed::Folio,
    )
    .await;
}

/// Runs `statement` in `tenant`'s scope and returns its error message; `$1` is the tenant and `$2` a second property.
async fn folio_statement_error(pool: &PgPool, tenant: TenantId, other: Uuid, statement: &'static str) -> String {
    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
    sqlx::query(statement).bind(tenant.0).bind(other).execute(&mut *tx).await.unwrap_err().to_string()
}

async fn seed_folio_with_second_property(pool: &PgPool) -> (TenantId, Uuid) {
    let tenant = seed_tenant(pool, "A").await;
    seed_folio(pool, tenant).await;
    let other = Uuid::now_v7();
    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
    sqlx::query(
        "insert into property (id, tenant_id, code, name, timezone, base_currency, business_date)
         values ($2, $1, 'OTH', 'Other', 'Asia/Colombo', 'LKR', current_date)",
    )
    .bind(tenant.0)
    .bind(other)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    (tenant, other)
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_folio_cannot_name_another_propertys_reservation(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (tenant, other) = seed_folio_with_second_property(&pool).await;
    let err = folio_statement_error(
        &pool,
        tenant,
        other,
        "insert into folio (id, tenant_id, property_id, kind, reservation_id)
         select gen_random_uuid(), $1, $2, 'deposit', id from reservation",
    )
    .await;
    assert!(err.contains("foreign key"), "unexpected error: {err}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_invoice_cannot_name_another_propertys_folio(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (tenant, other) = seed_folio_with_second_property(&pool).await;
    let err = folio_statement_error(
        &pool,
        tenant,
        other,
        "insert into invoice (id, tenant_id, property_id, folio_id, kind, serial, data, total_lkr)
         select gen_random_uuid(), $1, $2, id, 'invoice', 'OTH-2026-000001', '{}', 1 from folio",
    )
    .await;
    assert!(err.contains("foreign key"), "unexpected error: {err}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_payment_needs_a_folio_or_a_pos_bill(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (tenant, other) = seed_folio_with_second_property(&pool).await;
    let err = folio_statement_error(
        &pool,
        tenant,
        other,
        "insert into payment (id, tenant_id, method, amount, currency, base_amount, rate, rate_date, business_date)
         values (gen_random_uuid(), $1, 'cash', 1, 'LKR', 1, 1, current_date, current_date)",
    )
    .await;
    assert!(err.contains("payment_target_check"), "unexpected error: {err}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_lowercase_currency_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (tenant, other) = seed_folio_with_second_property(&pool).await;
    let err = folio_statement_error(
        &pool,
        tenant,
        other,
        "insert into fx_rate (tenant_id, property_id, date, currency, rate_to_base, source, source_date)
         values ($1, $2, current_date, 'usd', 300, 'manual', current_date)",
    )
    .await;
    assert!(err.contains("fx_rate_currency_check"), "unexpected error: {err}");
}
