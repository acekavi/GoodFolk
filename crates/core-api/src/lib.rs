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
pub mod persisted;
pub mod routes;
pub mod staff;
pub mod state;
pub mod tasks;

pub use routes::router;
pub use state::AppState;

/// `tcp` with Nagle's algorithm turned off on every connection it accepts; serve the [`router`] on this. Otherwise an
/// answer written in more than one piece can wait for the client's delayed ACK, about 40 ms.
pub fn without_nagle(
    tcp: tokio::net::TcpListener,
) -> impl axum::serve::Listener<Io = tokio::net::TcpStream, Addr = std::net::SocketAddr> {
    use axum::serve::ListenerExt;
    tcp.tap_io(|stream| {
        if let Err(error) = stream.set_nodelay(true) {
            tracing::warn!(%error, "could not turn Nagle's algorithm off on a connection");
        }
    })
}
