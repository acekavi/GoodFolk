use db::testing::app_pool;
use db::{Scope, TenantId, UserId, begin};
use property::{NewProperty, PropertyError, create_property, list_properties};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

async fn tenant_with_user(pool: &PgPool) -> (TenantId, UserId) {
    let tenant = TenantId(Uuid::now_v7());
    let user = UserId(Uuid::now_v7());
    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
    sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(tenant.0).execute(&mut *tx).await.unwrap();
    sqlx::query("insert into app_user (id, email, password_hash, display_name) values ($1, $2, 'x', 'U')")
        .bind(user.0)
        .bind(format!("{}@example.com", user.0))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    (tenant, user)
}

fn hotel(code: &str, timezone: &str) -> NewProperty {
    NewProperty {
        code: code.into(),
        name: format!("Hotel {code}"),
        timezone: timezone.into(),
        base_currency: "LKR".into(),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn created_properties_are_listed_by_code(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (tenant, user) = tenant_with_user(&pool).await;
    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();

    let galle = create_property(&mut tx, tenant, user, hotel("GAL", "Asia/Colombo")).await.unwrap();
    let kandy = create_property(&mut tx, tenant, user, hotel("KAN", "Asia/Colombo")).await.unwrap();
    let all = list_properties(&mut tx, None).await.unwrap();
    let only_kandy = list_properties(&mut tx, Some(&[kandy.id])).await.unwrap();

    assert_eq!(all, vec![galle, kandy.clone()]);
    assert_eq!(only_kandy, vec![kandy]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn codes_are_unique_within_a_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (tenant, user) = tenant_with_user(&pool).await;
    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    create_property(&mut tx, tenant, user, hotel("GAL", "Asia/Colombo")).await.unwrap();

    let again = create_property(&mut tx, tenant, user, hotel("GAL", "Asia/Colombo")).await;

    assert!(matches!(again, Err(PropertyError::CodeTaken)));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn unknown_time_zones_are_rejected(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;
    let (tenant, user) = tenant_with_user(&pool).await;
    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();

    let result = create_property(&mut tx, tenant, user, hotel("GAL", "Mars/Olympus")).await;

    assert!(matches!(result, Err(PropertyError::UnknownTimezone)));
}
