use crate::events::LiveEvent;
use crate::graphql::{GqlSchema, build_schema};
use crate::persisted;
use db::crypto::GuestIdKeys;
use reservations::CheckInPolicy;
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub schema: GqlSchema,
    /// Fan-out of database change events to this instance's SSE subscribers.
    pub events: broadcast::Sender<LiveEvent>,
    /// Production sets `Secure` cookies and disables GraphQL introspection.
    pub production: bool,
    /// Seals guest ID numbers under the current key; opens one sealed under it or a retired key.
    pub guest_id_keys: Arc<GuestIdKeys>,
    /// The room-condition gate for check-in, from `CHECKIN_REQUIRES_CLEAN_ROOM`.
    pub checkin_policy: CheckInPolicy,
}

impl AppState {
    pub fn new(pool: PgPool, production: bool, guest_id_keys: GuestIdKeys, checkin_policy: CheckInPolicy) -> Self {
        persisted::load();
        let (events, _) = broadcast::channel(1024);
        Self {
            schema: build_schema(production),
            pool,
            events,
            production,
            guest_id_keys: Arc::new(guest_id_keys),
            checkin_policy,
        }
    }
}
