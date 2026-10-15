//! The transactional outbox, the relay that drains it, and the event bus it publishes to.
//!
//! A business transaction calls [`enqueue`]; [`run_relay`] later delivers the row through an [`EventBus`]
//! (Pub/Sub in production, a direct HTTP push in local development). Receivers dedupe with [`first_seen`].

mod bus;
mod outbox;
mod push;
mod relay;

pub use bus::{BusError, EventBus, HeaderProvider, Message, PubSubBus, PushBus, bus_from_env, install_tls_provider};
pub use outbox::{enqueue, first_seen};
pub use push::{PushEnvelope, PushMessage};
pub use relay::{backoff, run_relay};
