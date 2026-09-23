//! HTTP layer: REST commands, GraphQL reads and server-sent events over the domain modules.

pub mod auth;
pub mod config;
pub mod csrf;
pub mod error;
pub mod events;
pub mod graphql;
pub mod idempotency;
pub mod routes;
pub mod state;

pub use routes::router;
pub use state::AppState;
