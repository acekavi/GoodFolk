use crate::auth::TenantContext;
use crate::state::AppState;
use axum::extract::{Query, State};
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

/// Forwards every `NOTIFY gf_events` from Postgres to this instance's subscribers.
/// Every instance runs one, so every connected client hears about every change.
pub fn spawn_listener(mut listener: PgListener, events: broadcast::Sender<Event>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match listener.recv().await {
                Ok(notification) => match serde_json::from_str::<Event>(notification.payload()) {
                    Ok(event) => {
                        // No subscribers is normal; nothing to do.
                        let _ = events.send(event);
                    }
                    Err(err) => tracing::warn!(error = %err, "ignoring malformed event payload"),
                },
                Err(err) => {
                    // PgListener reconnects on the next recv; back off briefly.
                    tracing::warn!(error = %err, "event listener error");
                    tokio::time::sleep(Duration::from_secs(1)).await;
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
    Query(query): Query<EventsQuery>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let tenant = ctx.tenant;
    let ready = tokio_stream::once(Ok(SseEvent::default().event("ready").data("{}")));
    let updates = BroadcastStream::new(state.events.subscribe()).filter_map(move |item| match item {
        Ok(event) => {
            let relevant = event.tenant_id == tenant
                && (event.property_id.is_none() || query.property.is_none() || event.property_id == query.property);
            relevant
                .then(|| Ok(SseEvent::default().event("invalidate").json_data(&event.keys).expect("keys serialize")))
        }
        Err(BroadcastStreamRecvError::Lagged(_)) => Some(Ok(SseEvent::default().event("resync").data("{}"))),
    });
    Sse::new(ready.chain(updates)).keep_alive(KeepAlive::default())
}
