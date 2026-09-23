use crate::graphql::{GqlSchema, build_schema};
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub schema: GqlSchema,
    /// Production sets `Secure` cookies and disables GraphQL introspection.
    pub production: bool,
}

impl AppState {
    pub fn new(pool: PgPool, production: bool) -> Self {
        Self { schema: build_schema(production), pool, production }
    }
}
