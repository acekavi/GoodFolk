use crate::auth::TenantContext;
use crate::extract::ApiQuery;
use crate::state::AppState;
use axum::extract::State;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use db::Event;
use futures::Stream;
use serde::Deserialize;
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

/// Forwards every `NOTIFY gf_events` from Postgres to this instance's subscribers.
/// Every instance runs one, so every connected client hears about every change.
///
/// Notifications sent while the connection is down are lost, so every stream is told to resync when it drops.
pub fn spawn_listener(mut listener: PgListener, events: broadcast::Sender<LiveEvent>) -> tokio::task::JoinHandle<()> {
    // Sending with no subscribers is normal, so send results are ignored.
    tokio::spawn(async move {
        loop {
            match listener.try_recv().await {
                Ok(Some(notification)) => match serde_json::from_str::<Event>(notification.payload()) {
                    Ok(event) => {
                        let _ = events.send(LiveEvent::Invalidate(event));
                    }
                    Err(err) => tracing::warn!(error = %err, "ignoring malformed event payload"),
                },
                Ok(None) => {
                    // The listener has already reconnected and listens again.
                    tracing::warn!("event listener connection lost; streams will resync");
                    let _ = events.send(LiveEvent::Resync);
                }
                Err(err) => {
                    // The connection may be gone; the next try_recv reconnects. Back off briefly, then
                    // resync, since notifications may have been missed.
                    tracing::warn!(error = %err, "event listener error; streams will resync");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    let _ = events.send(LiveEvent::Resync);
                }
            }
        }
    })
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
