use crate::auth::TenantContext;
use crate::extract::ApiQuery;
use crate::state::AppState;
use axum::extract::State;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use db::Event;
use futures::Stream;
use identity::{Permission, allows};
use serde::Deserialize;
use sqlx::PgPool;
use sqlx::postgres::PgListener;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, broadcast};
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

/// Subscribes to `NOTIFY gf_events` and `NOTIFY gf_outbox` on `pool` (a direct connection: poolers in
/// transaction mode cannot LISTEN), then forwards every `gf_events` notification to this instance's subscribers
/// and wakes this instance's outbox relay through `outbox_wake` on every `gf_outbox` one. Every instance runs one,
/// so every connected client hears about every change. Fails if the first subscription fails.
///
/// Notifications sent while the connection is down are lost, so streams are told to resync, and the relay is woken
/// to look for rows it missed, when it drops, and again once the listener is back if reconnecting took more than
/// one attempt.
pub async fn spawn_listener(
    pool: PgPool,
    events: broadcast::Sender<LiveEvent>,
    outbox_wake: Arc<Notify>,
) -> Result<tokio::task::JoinHandle<()>, sqlx::Error> {
    let listener = subscribe(&pool).await?;
    Ok(tokio::spawn(forward(pool, listener, events, outbox_wake)))
}

async fn subscribe(pool: &PgPool) -> Result<PgListener, sqlx::Error> {
    let mut listener = PgListener::connect_with(pool).await?;
    listener.listen_all([db::CHANNEL, ::events::OUTBOX_CHANNEL]).await?;
    Ok(listener)
}

// Sending with no subscribers is normal, so send results are ignored.
async fn forward(
    pool: PgPool,
    mut listener: PgListener,
    events: broadcast::Sender<LiveEvent>,
    outbox_wake: Arc<Notify>,
) {
    let resync = || {
        let _ = events.send(LiveEvent::Resync);
        outbox_wake.notify_one();
    };
    loop {
        match listener.try_recv().await {
            Ok(Some(notification)) if notification.channel() == ::events::OUTBOX_CHANNEL => outbox_wake.notify_one(),
            Ok(Some(notification)) => match serde_json::from_str::<Event>(notification.payload()) {
                Ok(event) => {
                    let _ = events.send(LiveEvent::Invalidate(event));
                }
                Err(err) => tracing::warn!(error = %err, "ignoring malformed event payload"),
            },
            Ok(None) => {
                // The connection dropped and sqlx has already reconnected and listens again.
                tracing::warn!("event listener connection lost and re-established; streams will resync");
                resync();
            }
            Err(err) => {
                // The connection is lost and sqlx could not re-establish it. Streams resync now, but
                // changes committed until the listener is back are missed too, so they resync again then.
                tracing::warn!(error = %err, "event listener connection lost; streams will resync");
                resync();
                drop(listener);
                listener = reconnect(&pool).await;
                resync();
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

/// Whether `key` is a folio's (`folio:<id>`) or a property's exchange rates' (`fx:<property>`).
fn is_money_key(key: &str) -> bool {
    key.starts_with("folio:") || key.starts_with("fx:")
}

/// Whether `key` is a property's night audit's (`audit:<property>`): its review and its runs.
fn is_audit_key(key: &str) -> bool {
    key.starts_with("audit:")
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
    let grants = ctx.grants;
    let ready = tokio_stream::once(Ok(SseEvent::default().event("ready").data("{}")));
    let updates = BroadcastStream::new(state.events.subscribe()).filter_map(move |item| match item {
        Ok(LiveEvent::Invalidate(event)) => {
            let relevant = event.tenant_id == tenant
                && (event.property_id.is_none() || query.property.is_none() || event.property_id == query.property);
            // Folio and exchange-rate keys name money: only those who may view the property's folios hear of them.
            // The night audit's keys go to those who run it and to those who may view the folios it closes.
            let folio_viewer = allows(&grants, Permission::FolioView, event.property_id);
            let auditor = folio_viewer || allows(&grants, Permission::NightAudit, event.property_id);
            let keys: Vec<&String> = event
                .keys
                .iter()
                .filter(|key| (folio_viewer || !is_money_key(key)) && (auditor || !is_audit_key(key)))
                .collect();
            (relevant && !keys.is_empty())
                .then(|| Ok(SseEvent::default().event("invalidate").json_data(&keys).expect("keys serialize")))
        }
        Ok(LiveEvent::Resync) | Err(BroadcastStreamRecvError::Lagged(_)) => {
            Some(Ok(SseEvent::default().event("resync").data("{}")))
        }
    });
    Sse::new(ready.chain(updates)).keep_alive(KeepAlive::default())
}
