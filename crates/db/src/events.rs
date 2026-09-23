use crate::{TenantId, Tx};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Postgres NOTIFY channel carrying cache-invalidation events to every API instance.
pub const CHANNEL: &str = "gf_events";

/// Tells clients which cached data changed. Carries keys, never the data itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub tenant_id: TenantId,
    pub property_id: Option<Uuid>,
    pub keys: Vec<String>,
}

/// Queues `event` for delivery. Postgres delivers it only if `tx` commits.
pub async fn notify(tx: &mut Tx, event: &Event) -> Result<(), sqlx::Error> {
    let payload = serde_json::to_string(event).expect("Event always serializes");
    sqlx::query("select pg_notify($1, $2)").bind(CHANNEL).bind(payload).execute(&mut **tx).await?;
    Ok(())
}
