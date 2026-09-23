use db::testing::app_pool;
use db::{CHANNEL, Event, Scope, TenantId, begin, notify};
use sqlx::postgres::{PgConnectOptions, PgListener, PgPoolOptions};
use std::time::Duration;
use uuid::Uuid;

fn event(tenant: TenantId) -> Event {
    Event { tenant_id: tenant, property_id: None, keys: vec!["properties".into()] }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn events_are_delivered_only_when_the_transaction_commits(_: PgPoolOptions, opts: PgConnectOptions) {
    let pool = app_pool(opts, 2).await;
    let mut listener = PgListener::connect_with(&pool).await.unwrap();
    listener.listen(CHANNEL).await.unwrap();
    let tenant = TenantId(Uuid::now_v7());

    let mut rolled_back = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    notify(&mut rolled_back, &Event { keys: vec!["discarded".into()], ..event(tenant) }).await.unwrap();
    rolled_back.rollback().await.unwrap();
    let mut committed = begin(&pool, Scope::tenant(tenant)).await.unwrap();
    notify(&mut committed, &event(tenant)).await.unwrap();
    committed.commit().await.unwrap();

    let received = tokio::time::timeout(Duration::from_secs(5), listener.recv()).await.unwrap().unwrap();
    let decoded: Event = serde_json::from_str(received.payload()).unwrap();
    assert_eq!(decoded, event(tenant));
}
