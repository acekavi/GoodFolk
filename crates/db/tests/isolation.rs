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

/// Tenant `b` sees none of `a`'s rows in `table`, only its own, and cannot insert a row owned by `a` (`insert`
/// binds `$1` to `a` and takes every other id from `b`'s own rows).
async fn assert_rates_table_isolated(opts: PgConnectOptions, table: &str, insert: &'static str) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool, "A").await;
    let b = seed_tenant(&pool, "B").await;
    seed_rates(&pool, a).await;
    seed_rates(&pool, b).await;

    let seen = visible_rows(&pool, b, table).await;
    let err = foreign_insert_error(&pool, b, a, insert).await;

    assert_eq!(seen, 1, "B sees only its own {table} row");
    assert!(err.contains("row-level security"), "unexpected error: {err}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn cancellation_policies_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_rates_table_isolated(
        opts,
        "cancellation_policy",
        "insert into cancellation_policy (id, tenant_id, property_id, name, rules, no_show)
         select gen_random_uuid(), $1, id, 'Strict', '[]', '{}' from property",
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn rate_plans_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_rates_table_isolated(
        opts,
        "rate_plan",
        "insert into rate_plan (id, tenant_id, property_id, code, name, kind, segment, currency)
         select gen_random_uuid(), $1, id, 'OTA', 'OTA', 'custom', 'OTA', 'USD' from property",
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn rate_plan_room_types_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_rates_table_isolated(
        opts,
        "rate_plan_room_type",
        "insert into rate_plan_room_type (tenant_id, property_id, rate_plan_id, room_type_id)
         select $1, p.property_id, p.id, t.id from rate_plan p, room_type t",
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn rate_days_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_rates_table_isolated(
        opts,
        "rate_day",
        "insert into rate_day (tenant_id, property_id, rate_plan_id, room_type_id, date, occupancy, amount)
         select $1, property_id, rate_plan_id, room_type_id, current_date + 1, 1, 9000 from rate_plan_room_type",
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn rate_restrictions_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_rates_table_isolated(
        opts,
        "rate_restriction",
        "insert into rate_restriction (tenant_id, property_id, rate_plan_id, room_type_id, date, closed)
         select $1, property_id, rate_plan_id, room_type_id, current_date + 1, true from rate_plan_room_type",
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn meal_supplements_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_rates_table_isolated(
        opts,
        "meal_supplement",
        "insert into meal_supplement (id, tenant_id, property_id, meal_plan, currency, adult_amount, child_amount, valid)
         select gen_random_uuid(), $1, id, 'HB', 'USD', 3000, 1500, daterange(current_date, null) from property",
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

/// Like [`assert_rates_table_isolated`], with every Phase 3 table seeded for both tenants.
async fn assert_reservations_table_isolated(opts: PgConnectOptions, table: &str, insert: &'static str) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool, "A").await;
    let b = seed_tenant(&pool, "B").await;
    seed_reservations(&pool, a).await;
    seed_reservations(&pool, b).await;

    let seen = visible_rows(&pool, b, table).await;
    let err = foreign_insert_error(&pool, b, a, insert).await;

    assert_eq!(seen, 1, "B sees only its own {table} row");
    assert!(err.contains("row-level security"), "unexpected error: {err}");
}

/// Guests belong to the tenant, not to a property: the policy is the tenant's alone.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn guests_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_reservations_table_isolated(
        opts,
        "guest",
        "insert into guest (id, tenant_id, first_name, last_name, residency)
         values (gen_random_uuid(), $1, 'Eve', 'Perera', 'non_resident')",
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn property_counters_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_reservations_table_isolated(
        opts,
        "property_counter",
        "insert into property_counter (tenant_id, property_id, name, value)
         select $1, property_id, name, value + 1 from property_counter",
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn reservations_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_reservations_table_isolated(
        opts,
        "reservation",
        "insert into reservation (id, tenant_id, property_id, confirmation_no, source, booker_guest_id)
         select gen_random_uuid(), $1, property_id, 'MAIN-000002', 'phone', booker_guest_id from reservation",
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn reservation_rooms_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_reservations_table_isolated(
        opts,
        "reservation_room",
        "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, stay, adults, children,
                                       rate_plan_id, meal_plan, status, primary_guest_id, currency)
         select gen_random_uuid(), $1, property_id, reservation_id, room_type_id, daterange(current_date + 5, current_date + 6),
                1, 0, rate_plan_id, 'RO', 'confirmed', primary_guest_id, currency
         from reservation_room",
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn reservation_nights_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_reservations_table_isolated(
        opts,
        "reservation_night",
        "insert into reservation_night (tenant_id, property_id, reservation_room_id, date, room_amount, meal_amount, currency)
         select $1, property_id, id, lower(stay) + 1, 12000, 0, currency from reservation_room",
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

/// Like [`assert_reservations_table_isolated`], with an account and an additional occupant seeded too (3b).
async fn assert_accounts_table_isolated(opts: PgConnectOptions, table: &str, insert: &'static str) {
    let pool = app_pool(opts, 1).await;
    let a = seed_tenant(&pool, "A").await;
    let b = seed_tenant(&pool, "B").await;
    seed_accounts_and_occupants(&pool, a).await;
    seed_accounts_and_occupants(&pool, b).await;

    let seen = visible_rows(&pool, b, table).await;
    let err = foreign_insert_error(&pool, b, a, insert).await;

    assert_eq!(seen, 1, "B sees only its own {table} row");
    assert!(err.contains("row-level security"), "unexpected error: {err}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn accounts_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_accounts_table_isolated(
        opts,
        "account",
        "insert into account (id, tenant_id, kind, name, currency)
         values (gen_random_uuid(), $1, 'travel_agent', 'Intruder Travel', 'USD')",
    )
    .await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn reservation_guests_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    assert_accounts_table_isolated(
        opts,
        "reservation_guest",
        "insert into reservation_guest (tenant_id, property_id, reservation_room_id, guest_id)
         select $1, property_id, reservation_room_id, guest_id from reservation_guest",
    )
    .await;
}
