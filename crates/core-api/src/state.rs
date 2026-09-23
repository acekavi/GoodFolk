use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    /// Production sets `Secure` cookies and disables GraphQL introspection.
    pub production: bool,
}

impl AppState {
    pub fn new(pool: PgPool, production: bool) -> Self {
        Self { pool, production }
    }
}
