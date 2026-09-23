use crate::graphql::{GqlSchema, build_schema};
use db::Event;
use sqlx::PgPool;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub schema: GqlSchema,
    /// Fan-out of database change events to this instance's SSE subscribers.
    pub events: broadcast::Sender<Event>,
    /// Production sets `Secure` cookies and disables GraphQL introspection.
    pub production: bool,
}

impl AppState {
    pub fn new(pool: PgPool, production: bool) -> Self {
        let (events, _) = broadcast::channel(1024);
        Self { schema: build_schema(production), pool, events, production }
    }
}
