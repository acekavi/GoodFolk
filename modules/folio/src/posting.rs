//! Posting a stay's nights: each in-house stay's room and meal-plan charges for a business date, and at check-out
//! every night still unposted. Each charge carries its posting key (`room:<stay>:<date>`, `meal_plan:<stay>:<date>`),
//! so a night is charged once however often it is posted.

use crate::folios::{ensure_deposit_folio, ensure_guest_folio, lock_folios};
use crate::lines::{NewCharge, line_by_key};
use crate::{FolioError, Source, audit, post_for_stay, reverse};
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
    /// Charges skipped because their posting key was already used. Approximate under a race: a concurrent run that
    /// posts a key between this run's lookup and its posting makes both count the line as posted (it is posted once).
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

/// Locks, once and in ascending id order, every folio that checking `reservation_room` out, or undoing its
/// check-in, may post to or reverse on: the stay's guest folio, its reservation's deposit folio, the open folio (if
/// it has one; none is opened here) of the account its routing sends `room` or `meal_plan` to, and the folios holding
/// the lines posted under the stay's room and meal-plan keys for `dates`. The later postings and reversals then lock nothing new, so a routing change that
/// has spread the stay's lines over several folios can't deadlock them (`40P01`) against a transfer or another
/// stay's posting, which lock in the same order. `NotFound` unless the stay is in `property`.
pub async fn lock_stay_folios(
    tx: &mut Tx,
    tenant: TenantId,
    property: Uuid,
    reservation_room: Uuid,
    dates: &[Date],
) -> Result<(), FolioError> {
    let routed: Option<(Uuid, Option<Uuid>)> = sqlx::query_as(
        "select rr.reservation_id,
                (select a.id from routing_rule rule
                 join folio a on a.kind = 'account' and a.account_id = rule.account_id and a.property_id = $2
                                 and a.status = 'open'
                 where rule.reservation_id = r.id and rule.account_id = r.account_id
                   and rule.category in ('room', 'meal_plan')
                 limit 1)
         from reservation_room rr join reservation r on r.id = rr.reservation_id
         where rr.id = $1 and rr.property_id = $2",
    )
    .bind(reservation_room)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    let (reservation, account_folio) = routed.ok_or(FolioError::NotFound("reservation room"))?;
    let mut ids = vec![
        ensure_guest_folio(tx, tenant, property, reservation_room).await?,
        ensure_deposit_folio(tx, tenant, property, reservation).await?,
    ];
    ids.extend(account_folio);
    ids.extend(posted_lines(tx, property, reservation_room, dates).await?.into_iter().map(|(folio, _)| folio));
    ids.sort();
    ids.dedup();
    lock_folios(tx, property, &ids).await?;
    Ok(())
}

/// `(folio, line)` of the `room` and `meal_plan` lines posted for `reservation_room`'s `dates` and not yet reversed
/// or moved, found in `property` by their posting keys, by folio then line.
async fn posted_lines(
    tx: &mut Tx,
    property: Uuid,
    reservation_room: Uuid,
    dates: &[Date],
) -> Result<Vec<(Uuid, Uuid)>, sqlx::Error> {
    let keys: Vec<String> = dates
        .iter()
        .flat_map(|date| [format!("room:{reservation_room}:{date}"), format!("meal_plan:{reservation_room}:{date}")])
        .collect();
    sqlx::query_as(
        "select l.folio_id, l.id from folio_line l join folio f on f.id = l.folio_id
         where f.property_id = $1 and l.posting_key = any($2)
           and not exists (select 1 from folio_line r where r.reverses = l.id)
         order by l.folio_id, l.id",
    )
    .bind(property)
    .bind(&keys)
    .fetch_all(&mut **tx)
    .await
}

/// Reverses the `room` and `meal_plan` lines already posted for `reservation_room`'s `dates` with `reason`, and
/// returns how many it reversed: the nights an early check-out releases ("Night released at early check-out"), or
/// the night of a check-in being undone ("Check-in undone"). Lines are found in `property` by their posting keys;
/// one already reversed (or moved) is left alone. Each reversal is [`reverse`]'s, on the property's business date,
/// so a line on an invoiced folio is a `Conflict`. Their folios are locked first, together, in ascending id order;
/// a caller that also posts locks them all before with [`lock_stay_folios`]. Check-out calls it, holding the stay's
/// row, after the release and before marking the stay checked out; undoing a check-in, before the deposit moves back.
pub async fn reverse_released_nights(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    property: Uuid,
    reservation_room: Uuid,
    dates: &[Date],
    reason: &str,
) -> Result<u32, FolioError> {
    let lines = posted_lines(tx, property, reservation_room, dates).await?;
    let mut folios: Vec<Uuid> = lines.iter().map(|(folio, _)| *folio).collect();
    folios.dedup();
    lock_folios(tx, property, &folios).await?;
    for (folio, line) in &lines {
        reverse(tx, tenant, user, property, *folio, *line, reason).await?;
    }
    Ok(u32::try_from(lines.len()).unwrap_or(u32::MAX))
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
