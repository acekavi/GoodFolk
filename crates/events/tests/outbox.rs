use async_trait::async_trait;
use db::testing::app_pool;
use db::{Scope, begin};
use events::{BusError, EventBus, Message, OUTBOX_CHANNEL, backoff, enqueue, first_seen, run_relay};
use serde_json::json;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgListener, PgPoolOptions};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Records what it is sent. A payload with `"block": true` waits for `release`; one with `"refuse": true` is refused.
#[derive(Default)]
struct MemoryBus {
    sent: Mutex<Vec<Message>>,
    started: Mutex<Vec<Uuid>>,
    release: tokio::sync::Notify,
    fail: bool,
}

#[async_trait]
impl EventBus for MemoryBus {
    async fn publish(&self, msg: &Message) -> Result<(), BusError> {
        self.started.lock().unwrap().push(msg.id);
        if self.fail {
            return Err(BusError::Retryable("down".to_owned()));
        }
        if msg.payload["refuse"] == true {
            return Err(BusError::Permanent("nope".to_owned()));
        }
        if msg.payload["block"] == true {
            self.release.notified().await;
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
    enqueue_payload(pool, topic, &json!({"asset": "a1"})).await
}

async fn enqueue_payload(pool: &PgPool, topic: &str, payload: &serde_json::Value) -> Uuid {
    let mut tx = begin(pool, Scope::default()).await.unwrap();
    let id = enqueue(&mut tx, topic, payload).await.unwrap();
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
    let relay = tokio::spawn(run_relay(app, bus.clone(), Arc::default(), shutdown.clone()));
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
    let relay = tokio::spawn(run_relay(app, bus.clone(), Arc::default(), shutdown.clone()));
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
        let relay = tokio::spawn(run_relay(app.clone(), bus.clone(), Arc::default(), shutdown.clone()));
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

async fn count_sent(owner: &PgPool) -> i64 {
    sqlx::query_scalar("select count(*) from outbox where sent_at is not null").fetch_one(owner).await.unwrap()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_blocked_send_does_not_hold_up_the_others(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = app_pool(opts.clone(), 6).await;
    let owner = PgPool::connect_with(opts).await.unwrap();
    let blocked = enqueue_payload(&app, "media-process", &json!({"block": true})).await;
    for _ in 0..15 {
        enqueue_committed(&app, "media-process").await;
    }

    let bus = Arc::new(MemoryBus::default());
    let shutdown = CancellationToken::new();
    let relay = tokio::spawn(run_relay(app, bus.clone(), Arc::default(), shutdown.clone()));
    wait_for("the other fifteen to be sent", async || count_sent(&owner).await == 15).await;
    let blocked_sent: bool = sqlx::query_scalar("select sent_at is not null from outbox where id = $1")
        .bind(blocked)
        .fetch_one(&owner)
        .await
        .unwrap();
    assert!(!blocked_sent);

    bus.release.notify_one();
    wait_for("the blocked send to finish", async || count_sent(&owner).await == 16).await;
    shutdown.cancel();
    relay.await.unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn shutdown_gives_sends_a_grace_then_returns(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = app_pool(opts, 4).await;
    enqueue_payload(&app, "media-process", &json!({"block": true})).await;

    let bus = Arc::new(MemoryBus::default());
    let shutdown = CancellationToken::new();
    let relay = tokio::spawn(run_relay(app, bus.clone(), Arc::default(), shutdown.clone()));
    wait_for("the send to start", async || !bus.started.lock().unwrap().is_empty()).await;

    let cancelled = std::time::Instant::now();
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(8), relay).await.expect("the relay stops").unwrap();
    let took = cancelled.elapsed();
    assert!(took >= Duration::from_secs(4) && took < Duration::from_secs(7), "stopped after {took:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_refused_message_is_finished_with_its_error_kept(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = app_pool(opts.clone(), 4).await;
    let owner = PgPool::connect_with(opts).await.unwrap();
    let id = enqueue_payload(&app, "media-process", &json!({"refuse": true})).await;

    let shutdown = CancellationToken::new();
    let relay = tokio::spawn(run_relay(app, Arc::new(MemoryBus::default()), Arc::default(), shutdown.clone()));
    wait_for("the row to be finished", async || count_sent(&owner).await == 1).await;
    shutdown.cancel();
    relay.await.unwrap();

    let error: Option<String> =
        sqlx::query_scalar("select last_error from outbox where id = $1").bind(id).fetch_one(&owner).await.unwrap();
    assert_eq!(error.as_deref(), Some("nope"));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn two_relays_never_send_a_row_twice(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = app_pool(opts.clone(), 8).await;
    let owner = PgPool::connect_with(opts).await.unwrap();
    for _ in 0..40 {
        enqueue_committed(&app, "media-process").await;
    }

    let bus = Arc::new(MemoryBus::default());
    let shutdown = CancellationToken::new();
    let relays: Vec<_> =
        (0..2).map(|_| tokio::spawn(run_relay(app.clone(), bus.clone(), Arc::default(), shutdown.clone()))).collect();
    wait_for("all rows to be sent", async || count_sent(&owner).await == 40).await;
    shutdown.cancel();
    for relay in relays {
        relay.await.unwrap();
    }

    let mut ids: Vec<Uuid> = bus.sent.lock().unwrap().iter().map(|m| m.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 40);
    assert_eq!(bus.started.lock().unwrap().len(), 40);
}

async fn is_sent(owner: &PgPool, id: Uuid) -> bool {
    sqlx::query_scalar("select sent_at is not null from outbox where id = $1").bind(id).fetch_one(owner).await.unwrap()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_committed_enqueue_wakes_an_idle_relay(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = app_pool(opts.clone(), 4).await;
    let owner = PgPool::connect_with(opts).await.unwrap();
    let bus = Arc::new(MemoryBus::default());
    let wake = Arc::new(Notify::new());
    let shutdown = CancellationToken::new();
    let relay = tokio::spawn(run_relay(app.clone(), bus.clone(), wake.clone(), shutdown.clone()));
    // Let it find the outbox empty and go idle.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Unwoken, an idle relay leaves a new row alone: it does not poll every few hundred milliseconds.
    let unheard = enqueue_committed(&app, "media-process").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!is_sent(&owner, unheard).await, "an idle relay polled");

    // What a LISTEN connection does with the commit's notification.
    let mut listener = PgListener::connect_with(&owner).await.unwrap();
    listener.listen(OUTBOX_CHANNEL).await.unwrap();
    let forward = {
        let wake = wake.clone();
        tokio::spawn(async move {
            while listener.recv().await.is_ok() {
                wake.notify_one();
            }
        })
    };
    let heard = enqueue_committed(&app, "media-process").await;
    // Measured from the commit to the send starting, which is all the wake-up decides.
    let committed = Instant::now();
    while !bus.started.lock().unwrap().contains(&heard) {
        assert!(committed.elapsed() < Duration::from_millis(100), "the woken relay had not sent the row within 100 ms");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    // The wake-up sends everything due, the row enqueued unheard too.
    wait_for("both rows to be marked sent", async || is_sent(&owner, heard).await && is_sent(&owner, unheard).await)
        .await;
    forward.abort();
    shutdown.cancel();
    relay.await.unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_rolled_back_enqueue_notifies_no_one(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = app_pool(opts.clone(), 2).await;
    let owner = PgPool::connect_with(opts).await.unwrap();
    let mut listener = PgListener::connect_with(&owner).await.unwrap();
    listener.listen(OUTBOX_CHANNEL).await.unwrap();

    let mut tx = begin(&app, Scope::default()).await.unwrap();
    enqueue(&mut tx, "media-process", &json!({"asset": "a1"})).await.unwrap();
    tx.rollback().await.unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(300), listener.recv()).await.is_err());

    enqueue_committed(&app, "media-process").await;
    let notification = tokio::time::timeout(Duration::from_secs(2), listener.recv()).await.unwrap().unwrap();
    assert_eq!(notification.channel(), OUTBOX_CHANNEL);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_failed_send_is_retried_when_due_without_a_wake(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = app_pool(opts.clone(), 4).await;
    let owner = PgPool::connect_with(opts).await.unwrap();
    let id = enqueue_committed(&app, "media-process").await;
    let bus = Arc::new(MemoryBus { fail: true, ..MemoryBus::default() });
    let shutdown = CancellationToken::new();
    let relay = tokio::spawn(run_relay(app, bus.clone(), Arc::default(), shutdown.clone()));

    // The first attempt fails and backs off 1 s; the relay claims it again then, well before its idle poll.
    wait_for("the second attempt", async || bus.started.lock().unwrap().len() >= 2).await;
    shutdown.cancel();
    relay.await.unwrap();
    let attempts: i32 =
        sqlx::query_scalar("select attempts from outbox where id = $1").bind(id).fetch_one(&owner).await.unwrap();
    assert!(attempts >= 2);
}
