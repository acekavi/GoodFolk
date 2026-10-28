//! The daily cleanup: expired sessions, sign-in failures older than the throttle window, and idempotency keys older
//! than a week, across tenants through `app.purge_expired`; and, when there is object storage, each property's
//! reports rendered on demand once they are 30 days old, with their PDFs, in a transaction scoped to its tenant.

use db::{Scope, TenantId};
use sqlx::PgPool;
use storage::{Bucket, Storage};
use uuid::Uuid;

/// How long an idempotency key is kept, in days.
const IDEMPOTENCY_DAYS: i32 = 7;

/// How long a report rendered on demand, and its PDF, is kept, in days.
const ON_DEMAND_REPORT_DAYS: i32 = 30;

/// Purges the expired rows, and the old on-demand reports when `storage` can take their PDFs too, and logs how many
/// went.
pub(crate) async fn run(pool: &PgPool, storage: Option<&Storage>) -> Result<(), String> {
    let purged: Result<(i64, i64, i64), sqlx::Error> =
        sqlx::query_as("select sessions, login_failures, idempotency_keys from app.purge_expired($1, $2)")
            .bind(identity::LOGIN_WINDOW_SECS)
            .bind(IDEMPOTENCY_DAYS)
            .fetch_one(pool)
            .await;
    let (sessions, login_failures, idempotency_keys) = purged.map_err(|error| error.to_string())?;
    let (reports, pdf_failures) = match storage {
        Some(storage) => purge_reports(pool, storage).await.map_err(|error| error.to_string())?,
        None => (0, 0),
    };
    tracing::info!(sessions, login_failures, idempotency_keys, reports, pdf_failures, "cleanup done");
    Ok(())
}

/// Deletes each property's on-demand reports older than [`ON_DEMAND_REPORT_DAYS`], then their PDFs: how many reports
/// went, and how many PDFs could not be deleted. Those are logged; their rows are gone, so nothing retries them.
async fn purge_reports(pool: &PgPool, storage: &Storage) -> Result<(usize, u32), sqlx::Error> {
    let properties: Vec<(Uuid, Uuid)> =
        sqlx::query_as("select tenant_id, property_id from app.jobs_properties()").fetch_all(pool).await?;
    let (mut reports, mut pdf_failures) = (0, 0);
    for (tenant, property) in properties {
        let mut tx = db::begin(pool, Scope::tenant(TenantId(tenant))).await?;
        let purged: Vec<(Uuid, Option<String>)> = sqlx::query_as(
            "delete from report
             where property_id = $1 and kind = 'on_demand' and created_at < now() - make_interval(days => $2)
             returning id, pdf_key",
        )
        .bind(property)
        .bind(ON_DEMAND_REPORT_DAYS)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        reports += purged.len();
        for (report, key) in purged {
            let Some(key) = key else { continue };
            if let Err(error) = storage.delete(Bucket::Private, &key).await {
                tracing::error!(%error, %tenant, %report, "cleanup: deleting a report's PDF failed");
                pdf_failures += 1;
            }
        }
    }
    Ok((reports, pdf_failures))
}
