use crate::bus::{BusError, EventBus, Message};
use futures::future::join_all;
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Rows claimed, and so sends in flight, per batch.
const BATCH: i32 = 8;
/// A relay that dies mid-send leaves its rows due again after this long. It exceeds the longest `/work` (3 x 60 s).
const LEASE_SECONDS: i32 = 900;
const IDLE_POLL: Duration = Duration::from_millis(500);

/// 1 s for the first attempt, doubling each time, capped at 300 s. `attempts` counts claims, so it is 1 on the first.
pub fn backoff(attempts: i32) -> Duration {
    let doublings = u32::try_from(attempts.saturating_sub(1)).unwrap_or(0).min(9);
    Duration::from_secs(1u64 << doublings).min(Duration::from_secs(300))
}

struct Claimed {
    message: Message,
    attempts: i32,
}

/// Drains the outbox until `shutdown` is cancelled, finishing the batch in flight first.
pub async fn run_relay(pool: PgPool, bus: Arc<dyn EventBus>, shutdown: CancellationToken) {
    while !shutdown.is_cancelled() {
        match claim(&pool).await {
            Ok(rows) if rows.is_empty() => {
                tokio::select! {
                    () = shutdown.cancelled() => {}
                    () = tokio::time::sleep(IDLE_POLL) => {}
                }
            }
            Ok(rows) => {
                join_all(rows.into_iter().map(|row| deliver(&pool, bus.as_ref(), row))).await;
            }
            Err(error) => {
                tracing::error!(%error, "outbox claim failed");
                tokio::select! {
                    () = shutdown.cancelled() => {}
                    () = tokio::time::sleep(Duration::from_secs(1)) => {}
                }
            }
        }
    }
}

async fn claim(pool: &PgPool) -> Result<Vec<Claimed>, sqlx::Error> {
    let rows: Vec<(Uuid, String, serde_json::Value, i32)> =
        sqlx::query_as("select id, topic, payload, attempts from app.outbox_claim($1, $2)")
            .bind(BATCH)
            .bind(LEASE_SECONDS)
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(id, topic, payload, attempts)| Claimed { message: Message { id, topic, payload }, attempts })
        .collect())
}

async fn deliver(pool: &PgPool, bus: &dyn EventBus, row: Claimed) {
    let Claimed { message, attempts } = row;
    let (id, topic) = (message.id, message.topic.as_str());
    let outcome = match bus.publish(&message).await {
        Ok(()) => sqlx::query("select app.outbox_sent($1)").bind(id).execute(pool).await.map(drop),
        Err(BusError::Permanent(error)) => {
            tracing::error!(%id, topic, %error, "outbox message refused for good; dropping it");
            // `outbox_sent` clears `last_error`, so the error is recorded after it. The row stays sent.
            mark_refused(pool, id, &error).await
        }
        Err(error) => {
            let delay = backoff(attempts);
            tracing::warn!(%id, topic, attempts, %error, "outbox send failed; will retry");
            sqlx::query("select app.outbox_retry($1, $2, $3)")
                .bind(id)
                .bind(error.to_string())
                .bind(i32::try_from(delay.as_secs()).unwrap_or(300))
                .execute(pool)
                .await
                .map(drop)
        }
    };
    if let Err(error) = outcome {
        // The lease expires and the row is delivered again; receivers dedupe.
        tracing::error!(%id, topic, %error, "recording the outbox outcome failed");
    }
}

async fn mark_refused(pool: &PgPool, id: Uuid, error: &str) -> Result<(), sqlx::Error> {
    sqlx::query("select app.outbox_sent($1)").bind(id).execute(pool).await?;
    sqlx::query("select app.outbox_retry($1, $2, 0)").bind(id).bind(error).execute(pool).await?;
    Ok(())
}
