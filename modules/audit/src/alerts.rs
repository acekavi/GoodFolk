//! Ops alerts: a problem a person must look at, one per property and kind, until what raised it is resolved.

use crate::audit_key;
use db::{Event, TenantId, Tx};
use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;

/// What an ops alert is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertKind {
    /// An automatic night audit that failed its third attempt.
    Audit,
    /// Inventory counters that drifted from the stays they count.
    Drift,
    /// Linen stock below par in a pantry or the store.
    Linen,
}

impl AlertKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AlertKind::Audit => "audit",
            AlertKind::Drift => "drift",
            AlertKind::Linen => "linen",
        }
    }

    pub fn parse(value: &str) -> Option<AlertKind> {
        match value {
            "audit" => Some(AlertKind::Audit),
            "drift" => Some(AlertKind::Drift),
            "linen" => Some(AlertKind::Linen),
            _ => None,
        }
    }
}

/// A raised alert.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OpsAlert {
    pub kind: AlertKind,
    pub message: String,
    pub raised_at: OffsetDateTime,
}

/// Raises the property's `kind` alert with `message` (at most 2000 characters, longer is cut), replacing the one
/// already raised. Notifies the property's audit key.
pub async fn raise(
    tx: &mut Tx,
    tenant: TenantId,
    property: Uuid,
    kind: AlertKind,
    message: &str,
) -> Result<(), sqlx::Error> {
    let message: String = message.chars().take(2000).collect();
    sqlx::query(
        "insert into ops_alert (tenant_id, property_id, kind, message) values ($1, $2, $3, $4)
         on conflict (property_id, kind) do update set message = excluded.message, raised_at = now()",
    )
    .bind(tenant.0)
    .bind(property)
    .bind(kind.as_str())
    .bind(message)
    .execute(&mut **tx)
    .await?;
    db::notify(tx, &Event { tenant_id: tenant, property_id: Some(property), keys: vec![audit_key(property)] }).await
}

/// Clears the property's `kind` alert. Notifies the property's audit key when there was one.
pub async fn clear(tx: &mut Tx, property: Uuid, kind: AlertKind) -> Result<(), sqlx::Error> {
    let tenant: Option<Uuid> =
        sqlx::query_scalar("delete from ops_alert where property_id = $1 and kind = $2 returning tenant_id")
            .bind(property)
            .bind(kind.as_str())
            .fetch_optional(&mut **tx)
            .await?;
    match tenant {
        Some(tenant) => {
            let event =
                Event { tenant_id: TenantId(tenant), property_id: Some(property), keys: vec![audit_key(property)] };
            db::notify(tx, &event).await
        }
        None => Ok(()),
    }
}

/// The property's raised alerts, by kind.
pub async fn list(tx: &mut Tx, property: Uuid) -> Result<Vec<OpsAlert>, sqlx::Error> {
    let rows: Vec<(String, String, OffsetDateTime)> =
        sqlx::query_as("select kind, message, raised_at from ops_alert where property_id = $1 order by kind")
            .bind(property)
            .fetch_all(&mut **tx)
            .await?;
    rows.into_iter()
        .map(|(kind, message, raised_at)| {
            let kind = AlertKind::parse(&kind)
                .ok_or_else(|| sqlx::Error::Decode(format!("unknown ops alert kind {kind:?}").into()))?;
            Ok(OpsAlert { kind, message, raised_at })
        })
        .collect()
}
