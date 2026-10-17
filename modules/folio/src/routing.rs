//! Routing: which of a stay's charges its reservation's account pays, by category.

use crate::folios::{ensure_account_folio, ensure_guest_folio, lock_folios};
use crate::lines::{LineRow, NewCharge, Posting, check_charge, line_by_key, post_locked};
use crate::{FolioError, audit, check_category, notify};
use db::{TenantId, Tx, UserId};
use std::collections::BTreeSet;
use uuid::Uuid;

/// The categories a reservation billed to an account routes there when it is booked.
pub const DEFAULT_ROUTING: [&str; 2] = ["room", "meal_plan"];

/// What [`post_for_stay`] needs to know about a stay.
#[derive(sqlx::FromRow)]
struct Stay {
    property_id: Uuid,
    /// The rate plan's.
    residency: Option<String>,
    /// That account, when a routing rule sends the charge's category to it.
    routed_account: Option<Uuid>,
}

/// Routes `categories` of the reservation's stays to its account's folio, replacing its rules; an empty list routes
/// nothing. Each category must be one a line can carry (`Invalid`), and a reservation without an account can route
/// nothing (`Invalid`). Locks the reservation row, which every reservation-room command takes last. Audited as
/// `reservation.routing_set`; queues the reservation's key.
pub async fn set_routing(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    reservation: Uuid,
    categories: &[String],
) -> Result<(), FolioError> {
    for category in categories {
        check_category(category)?;
    }
    let categories: Vec<&str> = categories.iter().map(String::as_str).collect::<BTreeSet<_>>().into_iter().collect();
    let (property, account): (Uuid, Option<Uuid>) =
        sqlx::query_as("select property_id, account_id from reservation where id = $1 for update")
            .bind(reservation)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(FolioError::NotFound("reservation"))?;
    if account.is_none() && !categories.is_empty() {
        return Err(FolioError::Invalid("the reservation has no account to route charges to".into()));
    }
    sqlx::query("delete from routing_rule where reservation_id = $1").bind(reservation).execute(&mut **tx).await?;
    if let Some(account) = account {
        sqlx::query(
            "insert into routing_rule (tenant_id, reservation_id, category, account_id)
             select $1, $2, category, $3 from unnest($4::text[]) as category",
        )
        .bind(tenant.0)
        .bind(reservation)
        .bind(account)
        .bind(&categories)
        .execute(&mut **tx)
        .await?;
    }
    let data = serde_json::json!({ "categories": categories, "account_id": account });
    audit(tx, tenant, user, "reservation.routing_set", "reservation", reservation, data).await?;
    // `reservations::reservation_key`: this crate cannot depend on reservations, which depends on it.
    notify(tx, tenant, property, vec![format!("reservation:{reservation}")]).await?;
    Ok(())
}

/// Follows a change of the reservation's account: re-points its routing rules, with the same categories, to
/// `account`, or deletes them when `account` is `None`. Returns the categories affected, sorted. Neither locks nor
/// audits: the caller holds the `reservation` row lock and audits the change as part of the reservation update.
pub async fn follow_account_change(
    tx: &mut Tx,
    reservation: Uuid,
    account: Option<Uuid>,
) -> Result<Vec<String>, FolioError> {
    let mut categories: Vec<String> = match account {
        Some(account) => {
            sqlx::query_scalar("update routing_rule set account_id = $2 where reservation_id = $1 returning category")
                .bind(reservation)
                .bind(account)
                .fetch_all(&mut **tx)
                .await?
        }
        None => {
            sqlx::query_scalar("delete from routing_rule where reservation_id = $1 returning category")
                .bind(reservation)
                .fetch_all(&mut **tx)
                .await?
        }
    };
    categories.sort();
    Ok(categories)
}

/// Posts `charge` for the stay `reservation_room`: on its reservation's account folio when a routing rule for the
/// charge's category names the reservation's current account, else on the stay's guest folio. Taxed with the stay's
/// rate plan residency, as [`crate::post_charge`].
///
/// A posting key names one line in the tenant (see the crate docs), so a key posted before a routing or account
/// change returns the existing line wherever it is. The stay's postings run one at a time: the guest folio (and the
/// routed folio) are locked, in ascending id order, before the key is looked up. No `reservation_room` lock is taken,
/// so the lock order holds.
pub async fn post_for_stay(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    reservation_room: Uuid,
    charge: NewCharge,
) -> Result<LineRow, FolioError> {
    post_routed(tx, tenant, user, reservation_room, charge, false).await
}

/// [`post_for_stay`], taxed without the service charge when `exclude_service_charge` (the POS path, for an outlet
/// that charges none).
pub(crate) async fn post_routed(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    reservation_room: Uuid,
    charge: NewCharge,
    exclude_service_charge: bool,
) -> Result<LineRow, FolioError> {
    let description = check_charge(&charge)?;
    let stay: Stay = sqlx::query_as(
        "select rr.property_id, rp.residency,
                (select rule.account_id from routing_rule rule
                 where rule.reservation_id = r.id and rule.category = $2 and rule.account_id = r.account_id)
                  as routed_account
         from reservation_room rr
         join reservation r on r.id = rr.reservation_id
         join rate_plan rp on rp.id = rr.rate_plan_id
         where rr.id = $1",
    )
    .bind(reservation_room)
    .bind(&charge.category)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(FolioError::NotFound("reservation room"))?;
    let Stay { property_id: property, residency, routed_account } = stay;
    let resident = residency.map(|residency| residency == "resident");

    let guest = ensure_guest_folio(tx, tenant, property, reservation_room).await?;
    let target = match routed_account {
        Some(account) => ensure_account_folio(tx, tenant, property, account).await?,
        None => guest,
    };
    let locked = lock_folios(tx, property, &[guest, target]).await?;
    let target = locked.into_iter().find(|folio| folio.id == target).ok_or(FolioError::NotFound("folio"))?;

    if let Some(key) = &charge.posting_key
        && let Some(line) = line_by_key(tx, property, key).await?
    {
        return Ok(line);
    }
    post_locked(
        tx,
        tenant,
        user,
        &target,
        charge,
        description,
        Posting { resident, exclude_service_charge, ..Posting::default() },
    )
    .await
}
