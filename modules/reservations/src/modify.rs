//! Changing a booked room's dates, type or occupancy: the stay stays inside the window, the inventory
//! counters move to match, and every night's price is fixed as booking did unless the caller asks to reprice.

use crate::assignment::room_taken_conflict;
use crate::{
    ReservationsError, SELLABLE, audit, business_date, check_window, decode_error, notify, reservation_key,
    reservations_key, violates,
};
use db::{TenantId, Tx, UserId};
use domain::RoomStatus;
use rates::{MealPlan, QuoteRequest, Residency};
use serde::Serialize;
use sqlx::Acquire;
use std::collections::{BTreeSet, HashMap};
use time::{Date, Duration};
use uuid::Uuid;

/// A change to a booked room's stay, type or occupancy. `None` leaves a field unchanged. At least one of
/// `check_in`, `check_out`, `room_type_id`, `adults` or `children` must actually change the room, or
/// `reprice` must be set, or the call is refused.
///
/// `keep_price` and `reprice` only matter when something else changes: `keep_price` keeps the amounts of
/// nights the new stay still covers even across a type or occupancy change (an upgrade keeps its price);
/// `reprice` requotes every night of the new stay regardless. Without either, nights common to the old and
/// new stay keep their amounts and only the added nights are quoted, exactly as booking priced them.
#[derive(Debug, Clone, Default)]
pub struct RoomChanges {
    pub check_in: Option<Date>,
    pub check_out: Option<Date>,
    pub room_type_id: Option<Uuid>,
    pub adults: Option<i32>,
    pub children: Option<i32>,
    pub keep_price: bool,
    pub reprice: bool,
}

/// A room after [`modify_room`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct ModifiedRoom {
    pub id: Uuid,
    pub reservation_id: Uuid,
    pub version: i32,
    pub check_in: Date,
    pub check_out: Date,
    pub room_type_id: Uuid,
    pub room_id: Option<Uuid>,
    /// Whether a type change unassigned the room it had (the old room isn't of the new type).
    pub unassigned: bool,
    pub total: i64,
    pub currency: String,
}

/// Changes room `id` of the property's dates, type or occupancy, at `expected_version`.
///
/// A confirmed room may change any field; a checked-in room may change only `check_out`, and the new
/// check-out must be after both the current check-in and the business date -- every other status, and every
/// other field on a checked-in room, is a `Conflict`. The new stay must lie inside the counter window and
/// arrive on or after the business date (unless the check-in is unchanged on a checked-in room).
///
/// Lock order: this row, then (if a room is assigned) the `room` row, then `rooms::lock_days` once over the
/// union of the old and new room types and the full `[min(old check-in, new check-in), max(old check-out,
/// new check-out))` range -- see "Room assignment lock order" and the `inventory_day` lock order in
/// `docs/design/api-conventions.md`. Nights the room no longer holds are released; nights it newly holds are
/// taken, each checked against [`SELLABLE`] (nights it already holds count as held, so only the added nights
/// need a free room); a night with none free is a `Conflict` naming it, exactly as booking one is. Only
/// counter rows from the business date on are touched, so a checked-in stay's past nights stay as they are.
///
/// Every night of the new stay is priced by [`rates::load_quote`] for the primary guest's residency; any
/// reason it lists not to sell it is `Invalid`, with every reason joined. Nights common to the old and new
/// stay keep their booked amounts unless `reprice` is set, or the type or occupancy changed without
/// `keep_price`, in which case every night is requoted; added nights always come from the same quote. The
/// room's currency never changes.
///
/// A type change unassigns the room if it isn't of the new type (`unassigned: true` in the result); a room
/// kept assigned across a date change may lose the room to `reservation_room_no_double_booking` (a
/// `Conflict` naming the booking that holds it, via the savepoint pattern `assign_room` uses) or to a block
/// over the new stay (`Conflict`, as `assign_room` checks).
pub async fn modify_room(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    changes: RoomChanges,
) -> Result<ModifiedRoom, ReservationsError> {
    let row: Option<RoomRow> = sqlx::query_as(
        "select reservation_id, room_type_id, room_id, lower(stay) as check_in, upper(stay) as check_out, adults,
                children, rate_plan_id, meal_plan, status, primary_guest_id, currency, version
         from reservation_room where id = $1 and property_id = $2
         for update",
    )
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    let RoomRow {
        reservation_id,
        room_type_id,
        room_id,
        check_in,
        check_out,
        adults,
        children,
        rate_plan_id,
        meal_plan,
        status,
        primary_guest_id,
        currency,
        version,
    } = row.ok_or(ReservationsError::NotFound("reservation room"))?;
    if version != expected_version {
        return Err(ReservationsError::VersionMismatch("reservation room"));
    }

    let new_check_in = changes.check_in.unwrap_or(check_in);
    let new_check_out = changes.check_out.unwrap_or(check_out);
    let new_room_type_id = changes.room_type_id.unwrap_or(room_type_id);
    let new_adults = changes.adults.unwrap_or(adults);
    let new_children = changes.children.unwrap_or(children);
    let changed = new_check_in != check_in
        || new_check_out != check_out
        || new_room_type_id != room_type_id
        || new_adults != adults
        || new_children != children
        || changes.reprice;
    if !changed {
        return Err(invalid("nothing to change".into()));
    }

    let status = RoomStatus::parse(&status).ok_or_else(|| decode_error("status", &status))?;
    if new_check_out <= new_check_in {
        return Err(invalid("check-out is after check-in".into()));
    }
    let today = business_date(tx, property).await?;
    match status {
        RoomStatus::Confirmed => check_window(today, new_check_in, new_check_out)?,
        RoomStatus::CheckedIn => {
            if new_check_in != check_in
                || new_room_type_id != room_type_id
                || new_adults != adults
                || new_children != children
            {
                return Err(ReservationsError::Conflict(
                    "a checked-in room can only have its check-out date changed".into(),
                ));
            }
            let last = today + Duration::days(rooms::WINDOW_DAYS);
            if new_check_out > last {
                return Err(invalid(format!("stays must arrive on or after {today} and leave by {last}")));
            }
            let floor = new_check_in.max(today);
            if new_check_out <= floor {
                return Err(invalid(format!("check-out must be after {floor}")));
            }
        }
        other => {
            return Err(ReservationsError::Conflict(format!(
                "only a confirmed or checked-in room can be modified; this one is {}",
                other.as_str().replace('_', " ")
            )));
        }
    }

    let (new_type_code, new_type_active): (String, bool) =
        sqlx::query_as("select code, active from room_type where id = $1 and property_id = $2")
            .bind(new_room_type_id)
            .bind(property)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| invalid("no such room type".into()))?;
    if new_room_type_id != room_type_id && !new_type_active {
        return Err(invalid(format!("{new_type_code} is no longer sold")));
    }

    // Lock order: this row (already locked above), then the room, before any inventory lock.
    let mut new_room_id = room_id;
    let mut unassigned = false;
    if let Some(rid) = room_id {
        let locked: Option<(String, Uuid)> =
            sqlx::query_as("select number, room_type_id from room where id = $1 and property_id = $2 for update")
                .bind(rid)
                .bind(property)
                .fetch_optional(&mut **tx)
                .await?;
        let (number, assigned_type) = locked.ok_or(ReservationsError::NotFound("room"))?;
        if assigned_type != new_room_type_id {
            new_room_id = None;
            unassigned = true;
        } else if new_check_in != check_in || new_check_out != check_out {
            let block: Option<(Date, Date)> = sqlx::query_as(
                "select lower(period), upper(period) from room_block
                 where room_id = $1 and released_at is null and period && daterange($2, $3)
                 order by lower(period)
                 limit 1",
            )
            .bind(rid)
            .bind(new_check_in)
            .bind(new_check_out)
            .fetch_optional(&mut **tx)
            .await?;
            if let Some((from, to)) = block {
                return Err(ReservationsError::Conflict(format!("room {number} is blocked from {from} to {to}")));
            }
        }
    }

    let type_ids: Vec<Uuid> =
        [room_type_id, new_room_type_id].into_iter().collect::<BTreeSet<_>>().into_iter().collect();
    let (from, to) = (check_in.min(new_check_in), check_out.max(new_check_out));
    rooms::extend_window(tx, property).await?;
    rooms::lock_days(tx, property, &type_ids, from, to).await?;

    // Counters: only nights from the business date on are ever held or freed; a checked-in stay's past
    // nights (before the business date) are history and untouched.
    let counter_old: BTreeSet<Date> = dates_in(check_in.max(today), check_out);
    let counter_new: BTreeSet<Date> = dates_in(new_check_in.max(today), new_check_out);
    let same_type = new_room_type_id == room_type_id;
    let released: Vec<Date> = counter_old.iter().copied().filter(|d| !(same_type && counter_new.contains(d))).collect();
    let taken: Vec<Date> = counter_new.iter().copied().filter(|d| !(same_type && counter_old.contains(d))).collect();
    let taken_set: BTreeSet<Date> = taken.iter().copied().collect();
    check_sellable(tx, property, new_room_type_id, &new_type_code, &taken_set).await?;

    let residency: String = sqlx::query_scalar("select residency from guest where id = $1 for share")
        .bind(primary_guest_id)
        .fetch_one(&mut **tx)
        .await?;
    let residency = Residency::parse(&residency).ok_or_else(|| decode_error("residency", &residency))?;
    let meal = MealPlan::parse(&meal_plan).ok_or_else(|| decode_error("meal_plan", &meal_plan))?;
    let request = QuoteRequest {
        room_type_id: new_room_type_id,
        rate_plan_id,
        meal_plan: meal,
        check_in: new_check_in,
        check_out: new_check_out,
        adults: new_adults,
        children: new_children,
        residency,
    };
    let quote = match rates::load_quote(tx, property, &request).await {
        Err(rates::RatesError::NotFound("rate plan")) => return Err(invalid("no such rate plan".into())),
        other => other?,
    };
    if !quote.violations.is_empty() {
        let reasons: Vec<String> = quote.violations.iter().map(|violation| violation.message.clone()).collect();
        return Err(invalid(reasons.join("; ")));
    }

    // Nothing has been written until here: every refusal above leaves the transaction with no changes. This
    // row's own update is the one write below that can still be refused (a conflicting
    // `reservation_room_no_double_booking`), so it runs first, via the same savepoint pattern `assign_room`
    // uses, before any counter or night write -- if it fails, none of those happen either. The reservation's
    // own version bump comes last, once every other write here has succeeded.
    let mut savepoint = tx.begin().await?;
    let updated = sqlx::query_scalar(
        "update reservation_room
         set stay = daterange($2, $3), room_type_id = $4, adults = $5, children = $6, room_id = $7,
             version = version + 1
         where id = $1
         returning version",
    )
    .bind(id)
    .bind(new_check_in)
    .bind(new_check_out)
    .bind(new_room_type_id)
    .bind(new_adults)
    .bind(new_children)
    .bind(new_room_id)
    .fetch_one(&mut *savepoint)
    .await;
    let new_version: i32 = match updated {
        Ok(version) => {
            savepoint.commit().await?;
            version
        }
        Err(err) if violates(&err, "reservation_room_no_double_booking") => {
            savepoint.rollback().await?;
            let room = new_room_id.expect("the exclusion constraint only fires when room_id is not null");
            let number: String =
                sqlx::query_scalar("select number from room where id = $1").bind(room).fetch_one(&mut **tx).await?;
            return Err(room_taken_conflict(tx, room, &number, id, new_check_in, new_check_out).await?);
        }
        Err(err) => return Err(err.into()),
    };

    if !released.is_empty() {
        sqlx::query(
            "update inventory_day set sold = sold - 1 where property_id = $1 and room_type_id = $2 and date = any($3)",
        )
        .bind(property)
        .bind(room_type_id)
        .bind(&released)
        .execute(&mut **tx)
        .await?;
    }
    if !taken.is_empty() {
        sqlx::query(
            "update inventory_day set sold = sold + 1 where property_id = $1 and room_type_id = $2 and date = any($3)",
        )
        .bind(property)
        .bind(new_room_type_id)
        .bind(&taken)
        .execute(&mut **tx)
        .await?;
    }

    let type_changed = new_room_type_id != room_type_id;
    let occupancy_changed = new_adults != adults || new_children != children;
    let requote_all = changes.reprice || ((type_changed || occupancy_changed) && !changes.keep_price);
    if requote_all {
        sqlx::query("delete from reservation_night where reservation_room_id = $1").bind(id).execute(&mut **tx).await?;
        insert_nights(tx, tenant, property, id, &quote.nights, &currency).await?;
    } else {
        let old_pricing = dates_in(check_in, check_out);
        let new_pricing = dates_in(new_check_in, new_check_out);
        let removed: Vec<Date> = old_pricing.difference(&new_pricing).copied().collect();
        let added: BTreeSet<Date> = new_pricing.difference(&old_pricing).copied().collect();
        if !removed.is_empty() {
            sqlx::query("delete from reservation_night where reservation_room_id = $1 and date = any($2)")
                .bind(id)
                .bind(&removed)
                .execute(&mut **tx)
                .await?;
        }
        if !added.is_empty() {
            let added_nights: Vec<rates::QuoteNight> =
                quote.nights.iter().filter(|night| added.contains(&night.date)).copied().collect();
            insert_nights(tx, tenant, property, id, &added_nights, &currency).await?;
        }
    }

    sqlx::query("update reservation set version = version + 1 where id = $1")
        .bind(reservation_id)
        .execute(&mut **tx)
        .await?;

    let total: i64 = sqlx::query_scalar(
        "select coalesce(sum(room_amount + meal_amount), 0)::bigint from reservation_night where reservation_room_id = $1",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;

    let data = serde_json::json!({
        "before": {
            "check_in": check_in, "check_out": check_out, "room_type": room_type_id,
            "adults": adults, "children": children,
        },
        "after": {
            "check_in": new_check_in, "check_out": new_check_out, "room_type": new_room_type_id,
            "adults": new_adults, "children": new_children,
        },
        "keep_price": changes.keep_price,
        "reprice": changes.reprice,
        "unassigned": unassigned,
    });
    audit(tx, tenant, actor, "reservation_room.modified", "reservation_room", id, data).await?;
    let months: BTreeSet<String> = rooms::month_keys(property, check_in, check_out)
        .into_iter()
        .chain(rooms::month_keys(property, new_check_in, new_check_out))
        .collect();
    let tape: BTreeSet<String> = rooms::tape_keys(property, check_in, check_out)
        .into_iter()
        .chain(rooms::tape_keys(property, new_check_in, new_check_out))
        .collect();
    let keys =
        [reservations_key(property), reservation_key(reservation_id)].into_iter().chain(months).chain(tape).collect();
    notify(tx, tenant, property, keys).await?;

    Ok(ModifiedRoom {
        id,
        reservation_id,
        version: new_version,
        check_in: new_check_in,
        check_out: new_check_out,
        room_type_id: new_room_type_id,
        room_id: new_room_id,
        unassigned,
        total,
        currency,
    })
}

fn invalid(message: String) -> ReservationsError {
    ReservationsError::Invalid(message)
}

/// The dates of `[from, to)`.
fn dates_in(from: Date, to: Date) -> BTreeSet<Date> {
    let mut days = BTreeSet::new();
    let mut day = from;
    while day < to {
        days.insert(day);
        day = day.next_day().expect("stays end inside the counter window");
    }
    days
}

/// Refuses with `Conflict` if any of `dates` has no [`SELLABLE`] room of `room_type` left, counting what the
/// counters already hold (a night this room already holds is never in `dates`: see [`modify_room`]).
async fn check_sellable(
    tx: &mut Tx,
    property: Uuid,
    room_type: Uuid,
    code: &str,
    dates: &BTreeSet<Date>,
) -> Result<(), ReservationsError> {
    let (Some(&from), Some(&last)) = (dates.iter().next(), dates.iter().next_back()) else {
        return Ok(());
    };
    let to = last + Duration::days(1);
    let rows: Vec<(Date, i32)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select i.date, {SELLABLE} from inventory_day i join room_type rt on rt.id = i.room_type_id
         where i.property_id = $1 and i.room_type_id = $2 and i.date >= $3 and i.date < $4"
    )))
    .bind(property)
    .bind(room_type)
    .bind(from)
    .bind(to)
    .fetch_all(&mut **tx)
    .await?;
    let free: HashMap<Date, i32> = rows.into_iter().collect();
    for date in dates {
        if free.get(date).copied().unwrap_or(0) < 1 {
            return Err(ReservationsError::Conflict(format!("no {code} rooms left on {date}")));
        }
    }
    Ok(())
}

/// Inserts `nights` as this room's snapshot, in `currency` (the room's own, which never changes).
async fn insert_nights(
    tx: &mut Tx,
    tenant: TenantId,
    property: Uuid,
    room_id: Uuid,
    nights: &[rates::QuoteNight],
    currency: &str,
) -> Result<(), sqlx::Error> {
    if nights.is_empty() {
        return Ok(());
    }
    let dates: Vec<Date> = nights.iter().map(|night| night.date).collect();
    let room_amounts: Vec<i64> = nights.iter().map(|night| night.room).collect();
    let meal_amounts: Vec<i64> = nights.iter().map(|night| night.meal).collect();
    sqlx::query(
        "insert into reservation_night (tenant_id, property_id, reservation_room_id, date, room_amount,
                                        meal_amount, currency)
         select $1, $2, $3, night.date, night.room_amount, night.meal_amount, $7
         from unnest($4::date[], $5::bigint[], $6::bigint[]) as night (date, room_amount, meal_amount)",
    )
    .bind(tenant.0)
    .bind(property)
    .bind(room_id)
    .bind(&dates)
    .bind(&room_amounts)
    .bind(&meal_amounts)
    .bind(currency)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// The parts of a `reservation_room` that modifying reads.
#[derive(sqlx::FromRow)]
struct RoomRow {
    reservation_id: Uuid,
    room_type_id: Uuid,
    room_id: Option<Uuid>,
    check_in: Date,
    check_out: Date,
    adults: i32,
    children: i32,
    rate_plan_id: Uuid,
    meal_plan: String,
    status: String,
    primary_guest_id: Uuid,
    currency: String,
    version: i32,
}
