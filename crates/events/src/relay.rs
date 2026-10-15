use crate::bus::{BusError, EventBus, Message};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Sends in flight at once.
const IN_FLIGHT: usize = 8;
/// On shutdown, how long sends already under way get to finish. Rows abandoned then come back after the lease.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
/// A relay that dies mid-send leaves its rows due again after this long. It exceeds the longest `/work` (3 x 60 s).
const LEASE_SECONDS: i32 = 900;
/// A send that has failed this many times is logged as an error, so a stuck topic gets noticed.
const ALERT_AFTER_ATTEMPTS: i32 = 10;
/// How long an idle relay waits before claiming again. A new row waits half of this on average before it is sent,
/// which counts against latency gates such as a photo's 5 s to `ready`; one cheap claim this often costs little.
const IDLE_POLL: Duration = Duration::from_millis(100);

/// 1 s for the first attempt, doubling each time, capped at 300 s. `attempts` counts claims, so it is 1 on the first.
pub fn backoff(attempts: i32) -> Duration {
    let doublings = u32::try_from(attempts.saturating_sub(1)).unwrap_or(0).min(9);
    Duration::from_secs(1u64 << doublings).min(Duration::from_secs(300))
}

struct Claimed {
    message: Message,
    attempts: i32,
}

/// Drains the outbox until `shutdown` is cancelled. Up to eight sends run at once and a slot is refilled as soon as
/// it frees up, so one slow send never holds back the others. After cancellation nothing new is claimed and sends
/// under way get [`SHUTDOWN_GRACE`] to finish.
pub async fn run_relay(pool: PgPool, bus: Arc<dyn EventBus>, shutdown: CancellationToken) {
    // Tasks, not futures polled from this loop: a send suspended mid-query would hold its connection while `claim`
    // waited for one.
    let mut in_flight = JoinSet::new();
    while !shutdown.is_cancelled() {
        let free = IN_FLIGHT - in_flight.len();
        let mut wait = IDLE_POLL;
        if free > 0 {
            match claim(&pool, free).await {
                Ok(rows) => {
                    for row in rows {
                        in_flight.spawn(deliver(pool.clone(), bus.clone(), row));
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "outbox claim failed");
                    wait = Duration::from_secs(1);
                }
            }
        }
        tokio::select! {
            () = shutdown.cancelled() => {}
            Some(joined) = in_flight.join_next(), if !in_flight.is_empty() => {
                if let Err(error) = joined {
                    tracing::error!(%error, "an outbox send panicked; its row returns after the lease");
                }
            }
            () = tokio::time::sleep(wait) => {}
        }
    }
    let drained = tokio::time::timeout(SHUTDOWN_GRACE, async { while in_flight.join_next().await.is_some() {} }).await;
    if drained.is_err() {
        tracing::warn!(
            abandoned = in_flight.len(),
            "relay stopped with sends still running; their rows return after the lease"
        );
    }
}

async fn claim(pool: &PgPool, limit: usize) -> Result<Vec<Claimed>, sqlx::Error> {
    let rows: Vec<(Uuid, String, serde_json::Value, i32)> =
        sqlx::query_as("select id, topic, payload, attempts from app.outbox_claim($1, $2)")
            .bind(i32::try_from(limit).unwrap_or(i32::MAX))
            .bind(LEASE_SECONDS)
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(id, topic, payload, attempts)| Claimed { message: Message { id, topic, payload }, attempts })
        .collect())
}

async fn deliver(pool: PgPool, bus: Arc<dyn EventBus>, row: Claimed) {
    let Claimed { message, attempts } = row;
    let (id, topic) = (message.id, message.topic.as_str());
    let outcome = match bus.publish(&message).await {
        Ok(()) => sqlx::query("select app.outbox_sent($1)").bind(id).execute(&pool).await.map(drop),
        Err(BusError::Permanent(error)) => {
            tracing::error!(%id, topic, %error, "outbox message refused for good; dropping it");
            sqlx::query("select app.outbox_refused($1, $2)").bind(id).bind(error).execute(&pool).await.map(drop)
        }
        Err(error) => {
            if attempts >= ALERT_AFTER_ATTEMPTS {
                tracing::error!(%id, topic, attempts, %error, "outbox send keeps failing");
            } else {
                tracing::warn!(%id, topic, attempts, %error, "outbox send failed; will retry");
            }
            let delay = i32::try_from(backoff(attempts).as_secs()).unwrap_or(300);
            sqlx::query("select app.outbox_retry($1, $2, $3)")
                .bind(id)
                .bind(error.to_string())
                .bind(delay)
                .execute(&pool)
                .await
                .map(drop)
        }
    };
    if let Err(error) = outcome {
        // The lease expires and the row is delivered again; receivers dedupe.
        tracing::error!(%id, topic, %error, "recording the outbox outcome failed");
    }
}
