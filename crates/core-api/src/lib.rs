//! HTTP layer: REST commands, GraphQL reads and server-sent events over the domain modules.

pub mod auth;
pub mod concurrency;
pub mod config;
pub mod csrf;
pub mod error;
pub mod events;
pub mod extract;
pub mod graphql;
pub mod idempotency;
pub mod openapi;
pub mod routes;
pub mod state;

pub use routes::router;
pub use state::AppState;
