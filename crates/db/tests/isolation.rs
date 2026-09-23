//! Tenant isolation suite. Every tenant-scoped table added later gets a case here.

use db::testing::app_pool;
use db::{Scope, TenantId, UserId, begin};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
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
        "insert into property (id, tenant_id, code, name, timezone, base_currency)
         values ($1, $2, 'MAIN', $3, 'Asia/Colombo', 'LKR')",
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
        "insert into property (id, tenant_id, code, name, timezone, base_currency)
         values ($1, $2, 'X1', 'Intruder', 'Asia/Colombo', 'LKR')",
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
