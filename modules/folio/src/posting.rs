//! Posting a stay's nights: each in-house stay's room and meal-plan charges for a business date, and at check-out
//! every night still unposted. Each charge carries its posting key (`room:<stay>:<date>`, `meal_plan:<stay>:<date>`),
//! so a night is charged once however often it is posted.

use crate::lines::{NewCharge, line_by_key};
use crate::{FolioError, Source, audit, post_for_stay};
use db::{Scope, TenantId, Tx, UserId};
use sqlx::PgPool;
use time::{Date, Duration};
use uuid::Uuid;

/// What [`post_business_date_charges`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PostReport {
    /// In-house stays whose nights were posted, or found already posted.
    pub stays: u32,
    /// Lines posted by this run.
    pub lines: u32,
    /// Charges skipped because their posting key was already used.
    pub skipped_existing: u32,
    /// Stays that could not be posted, with the reason (such as a missing exchange rate). Nothing of theirs was
    /// posted; a rerun once the cause is fixed posts them.
    pub failures: Vec<(Uuid, String)>,
}

/// One night of a stay, as [`post_night`] posts it.
#[derive(sqlx::FromRow)]
struct Night {
    property_id: Uuid,
    date: Date,
    room_amount: i64,
    meal_amount: i64,
    currency: String,
    meal_plan: String,
    /// The rate plan's `prices_include_tax`.
    inclusive: bool,
    /// The assigned room's number; a checked-in stay always has one.
    room_number: Option<String>,
}

/// New lines posted and charges skipped as already posted.
#[derive(Default)]
struct Counts {
    lines: u32,
    skipped: u32,
}

/// Posts the room charge (`reservation_night.room_amount`) and the meal-plan charge (`meal_amount`, when above zero)
/// of `date` for every stay of `property` checked in, not checked out, with `arrival ≤ date < departure`, routed as
/// [`post_for_stay`] routes them, under their posting keys. The caller checks the permission.
///
/// Each stay posts in its own transaction, in stay id order, and re-checks there (holding its row `for share`, ahead
/// of its folios in the lock order) that it is still in house, so a stay that checked out meanwhile is passed over.
/// A run that fails midway leaves the stays it finished posted, and a rerun skips them by their keys. One transaction
/// per stay, not per batch: a batch would hold the folio locks of many stays in stay order, which can deadlock
/// against a transfer locking two folios in id order.
///
/// A stay that can't be posted (a `Conflict`, `Invalid` or `NotFound`, such as no exchange rate for its currency on
/// `date`, or no price for the night) is rolled back and listed in [`PostReport::failures`], and the run goes on: one
/// stay never holds back the others. A database error stops the run and is returned.
///
/// Audits `folio.business_date_posted` on the property, with `{date, stays, lines, failures}`, in its own transaction
/// at the end.
pub async fn post_business_date_charges(
    pool: &PgPool,
    tenant: TenantId,
    user: Option<UserId>,
    property: Uuid,
    date: Date,
) -> Result<PostReport, FolioError> {
    let stays = in_house(pool, tenant, property, date).await?;
    let mut report = PostReport::default();
    for stay in stays {
        let mut tx = db::begin(pool, Scope::tenant(tenant)).await?;
        match post_stay_date(&mut tx, tenant, user, stay, date).await {
            Ok(None) => {}
            Ok(Some(counts)) => {
                tx.commit().await?;
                report.stays += 1;
                report.lines += counts.lines;
                report.skipped_existing += counts.skipped;
            }
            Err(err @ FolioError::Database(_)) => return Err(err),
            Err(err) => report.failures.push((stay, err.to_string())),
        }
    }

    let mut tx = db::begin(pool, Scope::tenant(tenant)).await?;
    let failures: Vec<serde_json::Value> = report
        .failures
        .iter()
        .map(|(stay, error)| serde_json::json!({ "reservation_room": stay, "error": error }))
        .collect();
    let data = serde_json::json!({
        "date": date,
        "stays": report.stays,
        "lines": report.lines,
        "failures": failures,
    });
    audit(&mut tx, tenant, user, "folio.business_date_posted", "property", property, data).await?;
    tx.commit().await?;
    Ok(report)
}

/// The stays of `property` checked in and in house on `date`, by id.
///
/// `arrival` and `nights` are stored and their comparisons leakproof, so under row-level security
/// `reservation_room_arrival_idx (property_id, arrival, id)` finds the short stays by a bounded arrival range (one of
/// at most 31 nights in house on `date` arrived within the 31 days before it), and
/// `reservation_room_long_stay_idx` the rare long ones. The 31 is written into the SQL so the planner can match the
/// partial index (see "A leakproof stored column for range queries" in `docs/design/api-conventions.md`).
async fn in_house(pool: &PgPool, tenant: TenantId, property: Uuid, date: Date) -> Result<Vec<Uuid>, FolioError> {
    let mut tx = db::begin(pool, Scope::tenant(tenant)).await?;
    let stays = sqlx::query_scalar(
        "select id from reservation_room
         where property_id = $1 and status = 'checked_in'
           and ((arrival >= $2::date - 31 and arrival <= $2) or nights > 31)
           and arrival <= $2 and $2 < arrival + nights
         order by id",
    )
    .bind(property)
    .bind(date)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(stays)
}

/// Posts `date`'s night of `stay` if it is still checked in and in house on that date (`None` when it is not).
async fn post_stay_date(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    stay: Uuid,
    date: Date,
) -> Result<Option<Counts>, FolioError> {
    let in_house: Option<Uuid> = sqlx::query_scalar(
        "select id from reservation_room
         where id = $1 and status = 'checked_in' and arrival <= $2 and $2 < arrival + nights
         for share",
    )
    .bind(stay)
    .bind(date)
    .fetch_optional(&mut **tx)
    .await?;
    if in_house.is_none() {
        return Ok(None);
    }
    let night = nights(tx, stay, Some(date), date + Duration::days(1))
        .await?
        .pop()
        .ok_or_else(|| FolioError::Conflict(format!("the stay has no price for {date}")))?;
    let mut counts = Counts::default();
    post_night(tx, tenant, user, stay, &night, &mut counts).await?;
    Ok(Some(counts))
}

/// Posts every night of `reservation_room` before `before` whose charges are not yet posted, oldest first, and
/// returns the number of lines posted. Check-out calls it, holding the stay's row, before marking the stay checked
/// out, with `before` = `min(departure, business date + 1)`: the business date's own night is unposted until the
/// night audit posts it. A stay arriving and departing the same day has no nights and posts nothing.
pub async fn post_unposted_nights(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    reservation_room: Uuid,
    before: Date,
) -> Result<u32, FolioError> {
    let mut counts = Counts::default();
    for night in nights(tx, reservation_room, None, before).await? {
        post_night(tx, tenant, user, reservation_room, &night, &mut counts).await?;
    }
    Ok(counts.lines)
}

/// The nights of `stay` in `[from, before)` (from its arrival when `from` is `None`), oldest first.
async fn nights(tx: &mut Tx, stay: Uuid, from: Option<Date>, before: Date) -> Result<Vec<Night>, sqlx::Error> {
    sqlx::query_as(
        "select rr.property_id, n.date, n.room_amount, n.meal_amount, n.currency::text as currency, rr.meal_plan,
                rp.prices_include_tax as inclusive, room.number as room_number
         from reservation_night n
         join reservation_room rr on rr.id = n.reservation_room_id
         join rate_plan rp on rp.id = rr.rate_plan_id
         left join room on room.id = rr.room_id
         where n.reservation_room_id = $1 and n.date >= coalesce($2, rr.arrival) and n.date < $3
         order by n.date",
    )
    .bind(stay)
    .bind(from)
    .bind(before)
    .fetch_all(&mut **tx)
    .await
}

/// Posts `night`'s room charge and, when it has one, its meal-plan charge, each unless its key is already used.
async fn post_night(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    stay: Uuid,
    night: &Night,
    counts: &mut Counts,
) -> Result<(), FolioError> {
    let room =
        night.room_number.as_deref().ok_or_else(|| FolioError::Conflict("the stay has no room assigned".into()))?;
    let date = night.date;
    let mut charges = vec![(Source::Room, "room", format!("Room {room} — {date}"), night.room_amount)];
    if night.meal_amount > 0 {
        let description = format!("Meal plan {} — {date}", night.meal_plan);
        charges.push((Source::MealPlan, "meal_plan", description, night.meal_amount));
    }
    for (source, category, description, amount) in charges {
        let key = format!("{category}:{stay}:{date}");
        // Looked up first so a rerun counts what it skips; a concurrent run posting the same key in between is
        // still a no-op in `post_for_stay`, only counted as posted by both.
        if line_by_key(tx, night.property_id, &key).await?.is_some() {
            counts.skipped += 1;
            continue;
        }
        let charge = NewCharge {
            source,
            category: category.into(),
            description,
            quantity: 1,
            unit_net: amount,
            currency: night.currency.clone(),
            business_date: date,
            posting_key: Some(key),
            inclusive: night.inclusive,
        };
        post_for_stay(tx, tenant, user, stay, charge).await?;
        counts.lines += 1;
    }
    Ok(())
}
