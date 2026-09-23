use db::testing::app_pool;
use db::{RlsBypassed, assert_rls_applies};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_app_role_is_subject_to_row_level_security(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 1).await;

    assert!(assert_rls_applies(&pool).await.is_ok());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_superuser_is_refused(pool: PgPool) {
    let result = assert_rls_applies(&pool).await;

    assert!(matches!(result, Err(RlsBypassed::Role(_))), "{result:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_role_that_owns_a_table_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = PgPool::connect_with(opts.clone()).await.unwrap();
    sqlx::query("create table owned_by_app (id int)").execute(&pool).await.unwrap();
    sqlx::query("alter table owned_by_app owner to goodfolk_app").execute(&pool).await.unwrap();
    let app = app_pool(opts, 1).await;

    let result = assert_rls_applies(&app).await;

    assert!(matches!(result, Err(RlsBypassed::Role(_))), "{result:?}");
}
