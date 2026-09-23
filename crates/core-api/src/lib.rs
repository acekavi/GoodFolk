//! HTTP layer: REST commands, GraphQL reads and server-sent events over the domain modules.

pub mod config;
pub mod error;
pub mod routes;
pub mod state;

pub use routes::router;
pub use state::AppState;
