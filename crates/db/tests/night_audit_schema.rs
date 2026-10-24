//! What the Phase 7b migration (`migrations/0031_night_audit.sql`) puts on audit runs, statistics, stays and the
//! jobs-svc definer functions, checked directly in the database.

use db::testing::app_pool;
use db::{Scope, TenantId, begin};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

struct Hotel {
    tenant: Uuid,
    property: Uuid,
}

/// A tenant with one property, a reservation and a confirmed stay on it. Runs as the superuser (no RLS).
async fn hotel(pool: &PgPool, code: &str) -> Hotel {
    let hotel = Hotel { tenant: Uuid::now_v7(), property: Uuid::now_v7() };
    let (room_type, guest, reservation, plan) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(hotel.tenant).execute(pool).await.unwrap();
    let statements = [
        "insert into property (id, tenant_id, code, name, timezone, base_currency, business_date)
         values ($2, $1, $3, 'Hotel', 'Asia/Colombo', 'LKR', current_date)",
        "insert into room_type (id, tenant_id, property_id, code, name, base_occupancy, max_adults, max_children, max_occupancy)
         values ($4, $1, $2, 'DLX', 'Deluxe', 2, 2, 0, 2)",
        "insert into rate_plan (id, tenant_id, property_id, code, name, kind, segment, currency)
         values ($5, $1, $2, 'BAR', 'Best available', 'standard', 'IBE', 'USD')",
        "insert into guest (id, tenant_id, first_name, last_name, residency) values ($6, $1, 'Ada', 'Silva', 'resident')",
        "insert into reservation (id, tenant_id, property_id, confirmation_no, source, booker_guest_id,
                                  booked_business_date)
         values ($7, $1, $2, $3 || '-000001', 'front_desk', $6, current_date)",
    ];
    for statement in statements {
        sqlx::query(statement)
            .bind(hotel.tenant)
            .bind(hotel.property)
            .bind(code)
            .bind(room_type)
            .bind(plan)
            .bind(guest)
            .bind(reservation)
            .execute(pool)
            .await
            .unwrap();
    }
    sqlx::query(
        "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, stay, adults, children,
                                       rate_plan_id, meal_plan, status, primary_guest_id, currency)
         values (gen_random_uuid(), $1, $2, $3, $4, daterange(current_date, current_date + 2), 2, 0, $5, 'RO',
                 'confirmed', $6, 'USD')",
    )
    .bind(hotel.tenant)
    .bind(hotel.property)
    .bind(reservation)
    .bind(room_type)
    .bind(plan)
    .bind(guest)
    .execute(pool)
    .await
    .unwrap();
    hotel
}

/// A run of `status` for today's business date at `hotel`'s property.
async fn run(pool: &PgPool, hotel: &Hotel, status: &str) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into audit_run (id, tenant_id, property_id, business_date, mode, status, finished_at)
         values ($1, $2, $3, current_date, 'manual', $4, case when $4 = 'running' then null else now() end)",
    )
    .bind(id)
    .bind(hotel.tenant)
    .bind(hotel.property)
    .bind(status)
    .execute(pool)
    .await?;
    Ok(id)
}

/// A step, a statistics row and a breakdown row for `hotel`'s property and today's business date.
async fn statistics(pool: &PgPool, hotel: &Hotel, run: Uuid) {
    let statements = [
        "insert into audit_run_step (tenant_id, run_id, step) values ($1, $3, 'resolve')",
        "insert into daily_stats (tenant_id, property_id, business_date, rooms_total, rooms_out_of_order,
                                  rooms_available, rooms_sold, complimentary_rooms, occupancy_bp, adults, children,
                                  arrivals, departures, no_shows, cancellations, walk_ins, room_revenue,
                                  meal_plan_revenue, pos_revenue, fee_revenue, other_revenue, adr, revpar, taxes,
                                  payments)
         values ($1, $2, current_date, 10, 0, 10, 5, 0, 5000, 10, 0, 2, 1, 0, 0, 0, 100000, 0, 0, 0, 0, 20000, 10000,
                 '{}', '{}')",
        "insert into daily_stats_breakdown (tenant_id, property_id, business_date, dimension, key, room_nights,
                                            room_revenue, revenue)
         values ($1, $2, current_date, 'segment', 'IBE', 5, 100000, 100000)",
    ];
    for statement in statements {
        sqlx::query(statement).bind(hotel.tenant).bind(hotel.property).bind(run).execute(pool).await.unwrap();
    }
}

fn constraint(result: Result<impl std::fmt::Debug, sqlx::Error>) -> String {
    let err = result.unwrap_err();
    err.as_database_error().and_then(|db_err| db_err.constraint()).unwrap_or_default().to_owned()
}

/// Runs `statement` as the app role in `hotel`'s tenant and returns the error's SQLSTATE.
async fn app_error(pool: &PgPool, hotel: &Hotel, statement: &'static str) -> Option<String> {
    let mut tx = begin(pool, Scope::tenant(TenantId(hotel.tenant))).await.unwrap();
    let result = sqlx::query(statement).execute(&mut *tx).await;
    result.err().and_then(|e| e.as_database_error().and_then(|e| e.code()).map(|c| c.into_owned()))
}

/// The test database's owner, which bypasses row-level security, and a pool acting as `goodfolk_app`.
async fn pools(opts: PgConnectOptions) -> (PgPool, PgPool) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts.clone()).await.unwrap();
    (owner, app_pool(opts, 1).await)
}

const PERMISSION_DENIED: &str = "42501";

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_app_role_cannot_rewrite_the_date_or_mode_of_a_run(_: PgPoolOptions, opts: PgConnectOptions) {
    let (owner, pool) = pools(opts).await;
    let hotel = hotel(&owner, "GAL").await;
    run(&owner, &hotel, "failed").await.unwrap();

    let date = app_error(&pool, &hotel, "update audit_run set business_date = business_date + 1").await;
    let mode = app_error(&pool, &hotel, "update audit_run set mode = 'auto'").await;
    let delete = app_error(&pool, &hotel, "delete from audit_run").await;
    let progress = app_error(
        &pool,
        &hotel,
        "update audit_run set status = 'completed', attempts = attempts + 1, error = null, failure = null,
                finished_at = now()",
    )
    .await;

    assert_eq!(date.as_deref(), Some(PERMISSION_DENIED));
    assert_eq!(mode.as_deref(), Some(PERMISSION_DENIED));
    assert_eq!(delete.as_deref(), Some(PERMISSION_DENIED));
    assert_eq!(progress, None);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn steps_and_statistics_are_append_only_for_the_app_role(_: PgPoolOptions, opts: PgConnectOptions) {
    let (owner, pool) = pools(opts).await;
    let hotel = hotel(&owner, "GAL").await;
    let run = run(&owner, &hotel, "completed").await.unwrap();
    statistics(&owner, &hotel, run).await;

    for statement in [
        "update audit_run_step set detail = '{\"x\":1}'",
        "delete from audit_run_step",
        "update daily_stats set rooms_sold = 0",
        "delete from daily_stats",
        "update daily_stats_breakdown set revenue = 0",
        "delete from daily_stats_breakdown",
    ] {
        assert_eq!(app_error(&pool, &hotel, statement).await.as_deref(), Some(PERMISSION_DENIED), "{statement}");
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_property_has_one_run_per_business_date(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts).await.unwrap();
    let hotel = hotel(&owner, "GAL").await;
    run(&owner, &hotel, "failed").await.unwrap();

    let second = run(&owner, &hotel, "running").await;

    assert_eq!(constraint(second), "audit_run_property_id_business_date_key");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_run_is_running_exactly_when_it_has_not_finished(_: PgPoolOptions, opts: PgConnectOptions) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts).await.unwrap();
    let hotel = hotel(&owner, "GAL").await;
    let running_but_finished = sqlx::query(
        "insert into audit_run (id, tenant_id, property_id, business_date, mode, status, finished_at)
         values (gen_random_uuid(), $1, $2, current_date, 'auto', 'running', now())",
    )
    .bind(hotel.tenant)
    .bind(hotel.property)
    .execute(&owner)
    .await;

    assert_eq!(constraint(running_but_finished), "audit_run_check");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_cancellation_date_is_required_exactly_for_cancelled_and_no_show_rooms(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let owner = PgPoolOptions::new().max_connections(1).connect_with(opts).await.unwrap();
    let hotel = hotel(&owner, "GAL").await;
    let set = |status: &'static str, cancelled: bool| {
        sqlx::query(
            "update reservation_room
             set status = $1, cancelled_at = case when $1 = 'cancelled' then now() end,
                 cancellation_penalty = case when $1 = 'cancelled' then 0 end,
                 cancelled_business_date = case when $2 then current_date end
             where property_id = $3",
        )
        .bind(status)
        .bind(cancelled)
        .bind(hotel.property)
        .execute(&owner)
    };

    let cancelled_without = set("cancelled", false).await;
    let no_show_without = set("no_show", false).await;
    let confirmed_with = set("confirmed", true).await;

    for result in [cancelled_without, no_show_without, confirmed_with] {
        assert_eq!(constraint(result), "reservation_room_cancelled_business_date_check");
    }
    assert!(set("cancelled", true).await.is_ok());
    assert!(set("no_show", true).await.is_ok());
    assert!(set("confirmed", false).await.is_ok());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_jobs_definers_return_rows_across_tenants(_: PgPoolOptions, opts: PgConnectOptions) {
    let (owner, pool) = pools(opts).await;
    let (a, b) = (hotel(&owner, "GAL").await, hotel(&owner, "KAN").await);
    let open = run(&owner, &a, "failed").await.unwrap();
    run(&owner, &b, "completed").await.unwrap();

    let mut properties: Vec<Uuid> =
        sqlx::query_scalar("select property_id from app.jobs_properties()").fetch_all(&pool).await.unwrap();
    let runs: Vec<(Uuid, Uuid, String)> =
        sqlx::query_as("select run_id, tenant_id, status from app.jobs_open_runs()").fetch_all(&pool).await.unwrap();
    properties.sort();
    let mut expected = vec![a.property, b.property];
    expected.sort();
    assert_eq!(properties, expected);
    assert_eq!(runs, vec![(open, a.tenant, "failed".to_owned())]);

    a_second_open_run(&owner, &b).await;
    let runs: Vec<Uuid> =
        sqlx::query_scalar("select tenant_id from app.jobs_open_runs()").fetch_all(&pool).await.unwrap();
    assert_eq!(runs.len(), 2, "open runs of both tenants");
}

/// A second, still running run for `hotel` on the next business date.
async fn a_second_open_run(pool: &PgPool, hotel: &Hotel) {
    sqlx::query(
        "insert into audit_run (id, tenant_id, property_id, business_date, mode, status)
         values (gen_random_uuid(), $1, $2, current_date + 1, 'auto', 'running')",
    )
    .bind(hotel.tenant)
    .bind(hotel.property)
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn purge_expired_deletes_only_rows_past_their_window(_: PgPoolOptions, opts: PgConnectOptions) {
    let (owner, pool) = pools(opts).await;
    let first = hotel(&owner, "GAL").await;
    let user = Uuid::now_v7();
    sqlx::query("insert into app_user (id, email, password_hash, display_name) values ($1, 'u@example.com', 'x', 'U')")
        .bind(user)
        .execute(&owner)
        .await
        .unwrap();
    for (token, expires) in [("expired", "-1 hour"), ("fresh", "1 hour")] {
        sqlx::query(
            "insert into session (id, token_hash, user_id, expires_at)
             values (gen_random_uuid(), sha256($1::bytea), $2, now() + $3::interval)",
        )
        .bind(token.as_bytes())
        .bind(user)
        .bind(expires)
        .execute(&owner)
        .await
        .unwrap();
    }
    for age in ["2 hours", "5 minutes"] {
        sqlx::query("insert into login_failure (id, email, at) values (gen_random_uuid(), 'u@example.com', now() - $1::interval)")
            .bind(age)
            .execute(&owner)
            .await
            .unwrap();
    }
    let other = hotel(&owner, "KAN").await;
    for (tenant, key, age) in [
        (first.tenant, "key-aged-7-days", "7 days"),
        (first.tenant, "key-aged-6-days", "6 days"),
        (other.tenant, "key-other-7-days", "7 days"),
    ] {
        sqlx::query(
            "insert into idempotency_key (tenant_id, key, request_hash, created_at)
             values ($1, $2, '\\x01', now() - $3::interval)",
        )
        .bind(tenant)
        .bind(key)
        .bind(age)
        .execute(&owner)
        .await
        .unwrap();
    }

    let purged: (i64, i64, i64) =
        sqlx::query_as("select sessions, login_failures, idempotency_keys from app.purge_expired(3600, 7)")
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(purged, (1, 1, 2));
    let sessions: i64 = sqlx::query_scalar("select count(*) from session").fetch_one(&pool).await.unwrap();
    let failures: i64 = sqlx::query_scalar("select count(*) from login_failure").fetch_one(&pool).await.unwrap();
    assert_eq!((sessions, failures), (1, 1));
    let mut tx = begin(&pool, Scope::tenant(TenantId(first.tenant))).await.unwrap();
    let keys: Vec<String> = sqlx::query_scalar("select key from idempotency_key").fetch_all(&mut *tx).await.unwrap();
    assert_eq!(keys, vec!["key-aged-6-days".to_owned()]);
    drop(tx);
    let mut tx = begin(&pool, Scope::tenant(TenantId(other.tenant))).await.unwrap();
    let keys: i64 = sqlx::query_scalar("select count(*) from idempotency_key").fetch_one(&mut *tx).await.unwrap();
    assert_eq!(keys, 0, "the other tenant's expired key is purged too");
}
