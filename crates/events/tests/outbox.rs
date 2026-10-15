use async_trait::async_trait;
use db::testing::app_pool;
use db::{Scope, begin};
use events::{BusError, EventBus, Message, backoff, enqueue, first_seen, run_relay};
use serde_json::json;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Default)]
struct MemoryBus {
    sent: Mutex<Vec<Message>>,
    fail: bool,
}

#[async_trait]
impl EventBus for MemoryBus {
    async fn publish(&self, msg: &Message) -> Result<(), BusError> {
        if self.fail {
            return Err(BusError::Retryable("down".to_owned()));
        }
        self.sent.lock().unwrap().push(msg.clone());
        Ok(())
    }
}

async fn wait_for<F: AsyncFn() -> bool>(what: &str, check: F) {
    for _ in 0..500 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn enqueue_committed(pool: &PgPool, topic: &str) -> Uuid {
    let mut tx = begin(pool, Scope::default()).await.unwrap();
    let id = enqueue(&mut tx, topic, &json!({"asset": "a1"})).await.unwrap();
    tx.commit().await.unwrap();
    id
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn enqueued_in_rolled_back_tx_is_never_sent(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = app_pool(opts.clone(), 2).await;
    let owner = PgPool::connect_with(opts).await.unwrap();
    let mut tx = begin(&app, Scope::default()).await.unwrap();
    enqueue(&mut tx, "media-process", &json!({"asset": "a1"})).await.unwrap();
    tx.rollback().await.unwrap();

    let bus = Arc::new(MemoryBus::default());
    let shutdown = CancellationToken::new();
    let relay = tokio::spawn(run_relay(app, bus.clone(), shutdown.clone()));
    tokio::time::sleep(Duration::from_millis(1200)).await;
    shutdown.cancel();
    relay.await.unwrap();

    let rows: i64 = sqlx::query_scalar("select count(*) from outbox").fetch_one(&owner).await.unwrap();
    assert_eq!(rows, 0);
    assert!(bus.sent.lock().unwrap().is_empty());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn relay_delivers_and_marks_sent(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = app_pool(opts.clone(), 4).await;
    let owner = PgPool::connect_with(opts).await.unwrap();
    let id = enqueue_committed(&app, "media-process").await;

    let bus = Arc::new(MemoryBus::default());
    let shutdown = CancellationToken::new();
    let relay = tokio::spawn(run_relay(app, bus.clone(), shutdown.clone()));
    wait_for("the row to be sent", async || {
        sqlx::query_scalar::<_, bool>("select sent_at is not null from outbox where id = $1")
            .bind(id)
            .fetch_one(&owner)
            .await
            .unwrap()
    })
    .await;
    shutdown.cancel();
    relay.await.unwrap();

    let sent = bus.sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0].id, sent[0].topic.as_str()), (id, "media-process"));
    assert_eq!(sent[0].payload, json!({"asset": "a1"}));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn failed_send_backs_off(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = app_pool(opts.clone(), 4).await;
    let owner = PgPool::connect_with(opts).await.unwrap();
    let id = enqueue_committed(&app, "media-process").await;
    let bus = Arc::new(MemoryBus { fail: true, ..MemoryBus::default() });

    for (attempt, expected_delay) in [(1, 1.0), (2, 2.0)] {
        let shutdown = CancellationToken::new();
        let relay = tokio::spawn(run_relay(app.clone(), bus.clone(), shutdown.clone()));
        wait_for("a failed send", async || {
            sqlx::query_scalar::<_, bool>("select attempts = $2 and last_error is not null from outbox where id = $1")
                .bind(id)
                .bind(attempt)
                .fetch_one(&owner)
                .await
                .unwrap()
        })
        .await;
        shutdown.cancel();
        relay.await.unwrap();

        let delay: f64 =
            sqlx::query_scalar("select extract(epoch from next_attempt_at - now())::float8 from outbox where id = $1")
                .bind(id)
                .fetch_one(&owner)
                .await
                .unwrap();
        assert!(delay > expected_delay - 0.9 && delay <= expected_delay, "attempt {attempt}: next attempt in {delay}s");
        sqlx::query("update outbox set next_attempt_at = now() where id = $1").bind(id).execute(&owner).await.unwrap();
    }
}

#[test]
fn backoff_doubles_to_a_cap() {
    let secs: Vec<u64> = [0, 1, 2, 3, 4, 9, 10, 40, i32::MAX].map(|a| backoff(a).as_secs()).to_vec();
    assert_eq!(secs, [1, 1, 2, 4, 8, 256, 300, 300, 300]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn crash_after_claim_redelivers_after_lease(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = app_pool(opts.clone(), 2).await;
    let id = enqueue_committed(&app, "media-process").await;

    let claim = || async {
        sqlx::query_as::<_, (Uuid, i32)>("select id, attempts from app.outbox_claim(8, 1)")
            .fetch_all(&app)
            .await
            .unwrap()
    };
    assert_eq!(claim().await, [(id, 1)]);
    assert!(claim().await.is_empty(), "leased rows are not handed out twice");

    // The first relay died before marking the row sent: once the lease lapses the row is claimed again.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(claim().await, [(id, 2)]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn receiver_dedupes_by_message_id(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = app_pool(opts, 2).await;
    let mut tx = begin(&app, Scope::default()).await.unwrap();
    assert!(first_seen(&mut tx, "m1").await.unwrap());
    assert!(!first_seen(&mut tx, "m1").await.unwrap());
    tx.rollback().await.unwrap();

    let mut tx = begin(&app, Scope::default()).await.unwrap();
    assert!(first_seen(&mut tx, "m1").await.unwrap(), "a rollback forgets the id");
    tx.commit().await.unwrap();
    let mut tx = begin(&app, Scope::default()).await.unwrap();
    assert!(!first_seen(&mut tx, "m1").await.unwrap());
    assert!(first_seen(&mut tx, "m2").await.unwrap());
}
