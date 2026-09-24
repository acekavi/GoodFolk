use crate::auth::TenantContext;
use crate::extract::ApiQuery;
use crate::state::AppState;
use axum::extract::State;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use db::Event;
use futures::Stream;
use serde::Deserialize;
use sqlx::PgPool;
use sqlx::postgres::PgListener;
use std::convert::Infallible;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use uuid::Uuid;

/// What this instance tells its open streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveEvent {
    /// A committed change; clients invalidate the event's keys.
    Invalidate(Event),
    /// Changes may have been missed (the database connection was lost); clients refetch everything on screen.
    Resync,
}

/// How long the listener waits before each attempt to reconnect.
const RECONNECT_DELAY: Duration = Duration::from_secs(1);

/// Subscribes to `NOTIFY gf_events` on `pool` (a direct connection: poolers in transaction mode cannot
/// LISTEN), then forwards every notification to this instance's subscribers. Every instance runs one,
/// so every connected client hears about every change. Fails if the first subscription fails.
///
/// Notifications sent while the connection is down are lost, so streams are told to resync when it
/// drops, and again once the listener is back if reconnecting took more than one attempt.
pub async fn spawn_listener(
    pool: PgPool,
    events: broadcast::Sender<LiveEvent>,
) -> Result<tokio::task::JoinHandle<()>, sqlx::Error> {
    let listener = subscribe(&pool).await?;
    Ok(tokio::spawn(forward(pool, listener, events)))
}

async fn subscribe(pool: &PgPool) -> Result<PgListener, sqlx::Error> {
    let mut listener = PgListener::connect_with(pool).await?;
    listener.listen(db::CHANNEL).await?;
    Ok(listener)
}

// Sending with no subscribers is normal, so send results are ignored.
async fn forward(pool: PgPool, mut listener: PgListener, events: broadcast::Sender<LiveEvent>) {
    loop {
        match listener.try_recv().await {
            Ok(Some(notification)) => match serde_json::from_str::<Event>(notification.payload()) {
                Ok(event) => {
                    let _ = events.send(LiveEvent::Invalidate(event));
                }
                Err(err) => tracing::warn!(error = %err, "ignoring malformed event payload"),
            },
            Ok(None) => {
                // The connection dropped and sqlx has already reconnected and listens again.
                tracing::warn!("event listener connection lost and re-established; streams will resync");
                let _ = events.send(LiveEvent::Resync);
            }
            Err(err) => {
                // The connection is lost and sqlx could not re-establish it. Streams resync now, but
                // changes committed until the listener is back are missed too, so they resync again then.
                tracing::warn!(error = %err, "event listener connection lost; streams will resync");
                let _ = events.send(LiveEvent::Resync);
                drop(listener);
                listener = reconnect(&pool).await;
                let _ = events.send(LiveEvent::Resync);
            }
        }
    }
}

async fn reconnect(pool: &PgPool) -> PgListener {
    loop {
        tokio::time::sleep(RECONNECT_DELAY).await;
        match subscribe(pool).await {
            Ok(listener) => {
                tracing::info!("event listener reconnected; streams will resync");
                return listener;
            }
            Err(err) => tracing::warn!(error = %err, "event listener could not reconnect; retrying"),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    pub property: Option<Uuid>,
}

/// `ready` is sent immediately so proxies that hold headers until the first bytes forward the stream.
/// `invalidate` events carry the cache keys that changed. `resync` means events were missed and
/// the client should refetch everything on screen.
pub async fn stream(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiQuery(query): ApiQuery<EventsQuery>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let tenant = ctx.tenant;
    let ready = tokio_stream::once(Ok(SseEvent::default().event("ready").data("{}")));
    let updates = BroadcastStream::new(state.events.subscribe()).filter_map(move |item| match item {
        Ok(LiveEvent::Invalidate(event)) => {
            let relevant = event.tenant_id == tenant
                && (event.property_id.is_none() || query.property.is_none() || event.property_id == query.property);
            relevant
                .then(|| Ok(SseEvent::default().event("invalidate").json_data(&event.keys).expect("keys serialize")))
        }
        Ok(LiveEvent::Resync) | Err(BroadcastStreamRecvError::Lagged(_)) => {
            Some(Ok(SseEvent::default().event("resync").data("{}")))
        }
    });
    Sse::new(ready.chain(updates)).keep_alive(KeepAlive::default())
}
