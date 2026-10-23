//! The daily drift check: a property's inventory counters against the stays they count (`rooms::find_drift`). A
//! drift is logged as an error and raised as the property's `drift` alert; a clean check clears it. Nothing is
//! repaired automatically.

use audit::AlertKind;
use audit::alerts;
use db::{Scope, TenantId};
use time::Date;
use uuid::Uuid;

/// Checks `property`; done for its business date.
pub(crate) async fn check(pool: &sqlx::PgPool, tenant: TenantId, property: Uuid) -> Result<Date, String> {
    let checked = async {
        let mut tx = db::begin(pool, Scope::tenant(tenant)).await?;
        let business_date: Date = sqlx::query_scalar("select business_date from property where id = $1")
            .bind(property)
            .fetch_one(&mut *tx)
            .await?;
        let drift = rooms::find_drift(&mut tx, property).await?;
        match drift.iter().map(|row| row.date).min() {
            None => alerts::clear(&mut tx, property, AlertKind::Drift).await?,
            Some(first) => {
                let mut days: Vec<Date> = drift.iter().map(|row| row.date).collect();
                days.dedup();
                tracing::error!(%property, rows = ?drift, "inventory counters differ from the bookings");
                let message = format!(
                    "Inventory counters differ from the bookings on {} days, from {first}; see the logs",
                    days.len()
                );
                alerts::raise(&mut tx, tenant, property, AlertKind::Drift, &message).await?;
            }
        }
        tx.commit().await?;
        Ok::<_, sqlx::Error>(business_date)
    };
    checked.await.map_err(|error| error.to_string())
}
