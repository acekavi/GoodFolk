use crate::auth::{Authenticated, TenantContext, holds_somewhere};
use crate::extract::ApiQuery;
use crate::state::AppState;
use axum::extract::State;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use db::{Event, TenantId};
use futures::Stream;
use identity::{Grant, Permission, TenantAccess, allows, resolve_tenant_access};
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

/// Whether `key` is a property's linen's (`linen:<property>`): its items, locations, par, stock and batches.
fn is_linen_key(key: &str) -> bool {
    key.starts_with("linen:")
}

/// Whether `key` is a property's guest laundry's (`laundry:<property>`): its prices and orders.
fn is_laundry_key(key: &str) -> bool {
    key.starts_with("laundry:")
}

/// Whether `key` is a tenant's staff's (`staff:<tenant>`): its members and invitations.
fn is_staff_key(key: &str) -> bool {
    key.starts_with("staff:")
}

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    pub property: Option<Uuid>,
}

/// One open stream: whose it is and what they may hear.
struct Watch {
    pool: PgPool,
    /// The stream's session, looked up again when the tenant's staff change.
    token: String,
    tenant: TenantId,
    grants: Vec<Grant>,
    property: Option<Uuid>,
    staff_key: String,
}

impl Watch {
    /// Reloads the session's access after a staff change. False when the session no longer reaches this tenant as a
    /// member (removed, signed out, switched to another tenant) or cannot be checked, and the stream should end.
    async fn refresh(&mut self) -> bool {
        match resolve_tenant_access(&self.pool, &self.token).await {
            Ok(TenantAccess::Member { tenant, grants, .. }) if tenant == self.tenant => {
                self.grants = grants;
                true
            }
            Ok(_) => false,
            Err(err) => {
                tracing::warn!(error = %err, "could not recheck a stream's session after a staff change; ending it");
                false
            }
        }
    }

    /// The `invalidate` event for `event`'s keys this stream may hear, if any.
    fn invalidation(&self, event: &Event) -> Option<SseEvent> {
        let grants = &self.grants;
        let relevant = event.tenant_id == self.tenant
            && (event.property_id.is_none() || self.property.is_none() || event.property_id == self.property);
        // Folio and exchange-rate keys name money: only those who may view the property's folios hear of them.
        // The night audit's keys go to those who run it and to those who may view the folios it closes.
        let folio_viewer = allows(grants, Permission::FolioView, event.property_id);
        let auditor = folio_viewer || allows(grants, Permission::NightAudit, event.property_id);
        // Linen keys go to those who may view the linen, and guest laundry keys to those who take the orders.
        let linen_viewer = allows(grants, Permission::LinenView, event.property_id);
        let laundry_taker = allows(grants, Permission::LaundryOrders, event.property_id);
        // Staff keys name who works where: only those who manage staff, at any property, hear of them.
        let staff_manager = holds_somewhere(grants, Permission::StaffManage);
        let keys: Vec<&String> = event
            .keys
            .iter()
            .filter(|key| {
                (folio_viewer || !is_money_key(key))
                    && (auditor || !is_audit_key(key))
                    && (linen_viewer || !is_linen_key(key))
                    && (laundry_taker || !is_laundry_key(key))
                    && (staff_manager || !is_staff_key(key))
            })
            .collect();
        (relevant && !keys.is_empty())
            .then(|| SseEvent::default().event("invalidate").json_data(&keys).expect("keys serialize"))
    }
}

/// `ready` is sent immediately so proxies that hold headers until the first bytes forward the stream.
/// `invalidate` events carry the cache keys that changed. `resync` means events were missed and
/// the client should refetch everything on screen.
///
/// A change to the tenant's staff (`staff:<tenant>`) rechecks the stream's session first: a member who was removed
/// (or whose session ended) has the stream ended, so the client's reconnect gets the `401`, and one whose grants
/// changed hears the following keys under the new grants. The recheck runs in the stream, not in the listener, so
/// one slow lookup never holds up another stream.
pub async fn stream(
    State(state): State<AppState>,
    ctx: TenantContext,
    auth: Authenticated,
    ApiQuery(query): ApiQuery<EventsQuery>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let watch = Watch {
        pool: state.pool.clone(),
        token: auth.token,
        tenant: ctx.tenant,
        grants: ctx.grants,
        property: query.property,
        staff_key: identity::staff_key(ctx.tenant),
    };
    let ready = tokio_stream::once(Ok(SseEvent::default().event("ready").data("{}")));
    let events = BroadcastStream::new(state.events.subscribe());
    let updates = futures::stream::unfold((events, watch), |(mut events, mut watch)| async move {
        loop {
            match events.next().await? {
                Ok(LiveEvent::Invalidate(event)) => {
                    if event.tenant_id == watch.tenant
                        && event.keys.contains(&watch.staff_key)
                        && !watch.refresh().await
                    {
                        return None;
                    }
                    if let Some(invalidate) = watch.invalidation(&event) {
                        return Some((Ok(invalidate), (events, watch)));
                    }
                }
                Ok(LiveEvent::Resync) | Err(BroadcastStreamRecvError::Lagged(_)) => {
                    return Some((Ok(SseEvent::default().event("resync").data("{}")), (events, watch)));
                }
            }
        }
    });
    Sse::new(ready.chain(updates)).keep_alive(KeepAlive::default())
}
