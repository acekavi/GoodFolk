//! What the night audit does with a stay still unresolved on the business date: an arrival that never came is a
//! no-show or arrives a day late, and a departure still in house stays one more night. A person runs these from the
//! review; the automatic close runs them with no actor.

use crate::assignment::room_taken_conflict;
use crate::cancellation::{closed_keys, lock_closing_stay, no_show_penalty, release_from, stay_nights};
use crate::modify::check_not_blocked;
use crate::{
    ReservationsError, SELLABLE, audit, bump_reservation, business_date, decode_error, mode, notify, reservation_key,
    reservations_key, violates,
};
use db::{TenantId, Tx, UserId};
use domain::{Action, RoomStatus};
use serde::Serialize;
use sqlx::Acquire;
use time::{Date, Duration};
use uuid::Uuid;

/// A room marked a no-show and what not arriving cost, in its currency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct NoShowRoom {
    pub id: Uuid,
    pub reservation_id: Uuid,
    pub version: i32,
    pub penalty: i64,
    pub currency: String,
}

/// A stay after its arrival was delayed or its departure extended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct ChangedStay {
    pub id: Uuid,
    pub reservation_id: Uuid,
    pub version: i32,
    pub check_in: Date,
    pub check_out: Date,
    /// The stay's room and meal amounts, all nights.
    pub total: i64,
    pub currency: String,
}

/// Marks the room `id` of the property, at `expected_version` (`VersionMismatch`), confirmed (`Conflict`) and due
/// on or before the business date (`Conflict`), a no-show. Its nights from the business date on go back on sale, and
/// its booked terms' `no_show` penalty ([`no_show_penalty`], nothing without terms) posts to the reservation's
/// deposit folio as a `no_show_fee` line ([`folio::post_no_show_fee`]), tax-inclusive when its rate plan's prices
/// are. The deposit stays where it is, for a refund or a write-off. `actor` is `None` for the automatic close.
///
/// Locks as [`crate::cancel_room`] does: this row, the business date, the counters, the deposit folio, then the
/// reservation, whose version moves (its derived status may change).
pub async fn mark_no_show(
    tx: &mut Tx,
    tenant: TenantId,
    actor: Option<UserId>,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
) -> Result<NoShowRoom, ReservationsError> {
    let (stay, current) = lock_closing_stay(tx, property, id, expected_version).await?;
    domain::transition(current, Action::NoShow).map_err(|invalid| ReservationsError::Conflict(invalid.message))?;
    let today = business_date(tx, property).await?;
    if stay.check_in > today {
        return Err(ReservationsError::Conflict(format!(
            "the guest is not due until {}: only a stay due by {today} can be a no-show",
            stay.check_in
        )));
    }

    let from = release_from(tx, property, &stay, today).await?;
    let nights = stay_nights(tx, id).await?;
    let penalty = no_show_penalty(stay.cancellation_terms.as_ref().map(|terms| &terms.0), &nights);
    let version: i32 = sqlx::query_scalar(
        "update reservation_room set status = 'no_show', cancelled_business_date = $2, version = version + 1
         where id = $1
         returning version",
    )
    .bind(id)
    .bind(today)
    .fetch_one(&mut **tx)
    .await?;
    folio::post_no_show_fee(tx, tenant, actor, property, id, penalty, &stay.currency, stay.prices_include_tax).await?;
    bump_reservation(tx, stay.reservation_id).await?;

    let data = serde_json::json!({
        "reservation_id": stay.reservation_id,
        "penalty": penalty,
        "currency": stay.currency,
        "mode": mode(actor),
    });
    audit(tx, tenant, actor, "reservation_room.no_show", "reservation_room", id, data).await?;
    notify(tx, tenant, property, closed_keys(property, &stay, from)).await?;

    Ok(NoShowRoom { id, reservation_id: stay.reservation_id, version, penalty, currency: stay.currency })
}

/// Delays the arrival of the room `id` of the property, at `expected_version` (`VersionMismatch`) and tentative or
/// confirmed (`Conflict`), to the day after the business date D: the stay becomes `[D + 1, departure)`. It must be
/// due by D and leave after D + 1 (`Conflict` otherwise: a later or one-night change is Modify's). Its nights before
/// D + 1 are dropped and D's counter released; earlier days are history. The room stays assigned and the other
/// nights keep their prices.
pub async fn delay_arrival(
    tx: &mut Tx,
    tenant: TenantId,
    actor: Option<UserId>,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
) -> Result<ChangedStay, ReservationsError> {
    let (stay, status) = lock_stay(tx, property, id, expected_version).await?;
    if !matches!(status, RoomStatus::Tentative | RoomStatus::Confirmed) {
        return Err(ReservationsError::Conflict(format!(
            "only a tentative or confirmed stay can arrive late; this one is {}",
            status.as_str().replace('_', " ")
        )));
    }
    let today = business_date(tx, property).await?;
    let new_check_in = today + Duration::days(1);
    if stay.check_in > today || stay.check_out <= new_check_in {
        return Err(ReservationsError::Conflict("Change the dates with Modify".into()));
    }

    rooms::lock_days(tx, property, &[stay.room_type_id], today, new_check_in).await?;
    sqlx::query("update inventory_day set sold = sold - 1 where property_id = $1 and room_type_id = $2 and date = $3")
        .bind(property)
        .bind(stay.room_type_id)
        .bind(today)
        .execute(&mut **tx)
        .await?;
    sqlx::query("delete from reservation_night where reservation_room_id = $1 and date < $2")
        .bind(id)
        .bind(new_check_in)
        .execute(&mut **tx)
        .await?;
    // A shorter stay overlaps nothing it did not already: the room needs no new check.
    let version: i32 = sqlx::query_scalar(
        "update reservation_room set stay = daterange($2, upper(stay)), version = version + 1
         where id = $1
         returning version",
    )
    .bind(id)
    .bind(new_check_in)
    .fetch_one(&mut **tx)
    .await?;
    bump_reservation(tx, stay.reservation_id).await?;
    let total = total(tx, id).await?;

    let data = serde_json::json!({
        "reservation_id": stay.reservation_id,
        "from": stay.check_in,
        "to": new_check_in,
        "mode": mode(actor),
    });
    audit(tx, tenant, actor, "reservation_room.arrival_delayed", "reservation_room", id, data).await?;
    let keys = [reservations_key(property), reservation_key(stay.reservation_id), housekeeping::hk_key(property)]
        .into_iter()
        .chain(rooms::month_keys(property, today, new_check_in))
        .chain(rooms::tape_keys(property, stay.check_in, stay.check_out))
        .collect();
    notify(tx, tenant, property, keys).await?;

    Ok(ChangedStay {
        id,
        reservation_id: stay.reservation_id,
        version,
        check_in: new_check_in,
        check_out: stay.check_out,
        total,
        currency: stay.currency,
    })
}

/// Keeps the room `id` of the property, at `expected_version` (`VersionMismatch`), checked in (`Conflict`) and due
/// out on the business date D (`Conflict`), one more night: it now leaves on D + 1, and night D costs what its last
/// night did, room and meal, with no quote and no rate restrictions. That night must have a room of its type to sell,
/// and its room must be neither blocked nor taken by another stay then (`Conflict`, naming the block or the booking).
///
/// Locks this row, its room (as assigning does), then D's counter.
pub async fn extend_stay(
    tx: &mut Tx,
    tenant: TenantId,
    actor: Option<UserId>,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
) -> Result<ChangedStay, ReservationsError> {
    let (stay, status) = lock_stay(tx, property, id, expected_version).await?;
    if status != RoomStatus::CheckedIn {
        return Err(ReservationsError::Conflict(format!(
            "only a checked-in stay can be extended; this one is {}",
            status.as_str().replace('_', " ")
        )));
    }
    let today = business_date(tx, property).await?;
    if stay.check_out != today {
        return Err(ReservationsError::Conflict(format!(
            "only a stay leaving on {today} can be extended a night; this one leaves on {}",
            stay.check_out
        )));
    }
    let room = stay.room_id.ok_or_else(|| ReservationsError::Conflict("the stay has no room assigned".into()))?;
    let number: String =
        sqlx::query_scalar("select number from room where id = $1 for update").bind(room).fetch_one(&mut **tx).await?;
    // The guest stays tonight: the night dated D, so the stay now leaves on D + 1.
    let night = today;
    let new_check_out = night + Duration::days(1);

    rooms::lock_days(tx, property, &[stay.room_type_id], night, new_check_out).await?;
    let sellable: Option<i32> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "select {SELLABLE} from inventory_day i join room_type rt on rt.id = i.room_type_id
         where i.property_id = $1 and i.room_type_id = $2 and i.date = $3"
    )))
    .bind(property)
    .bind(stay.room_type_id)
    .bind(night)
    .fetch_optional(&mut **tx)
    .await?;
    if sellable.unwrap_or(0) < 1 {
        return Err(ReservationsError::Conflict(format!("No room of this type is free on {night}")));
    }
    check_not_blocked(tx, room, &number, night, new_check_out).await?;

    // The savepoint keeps the transaction usable to name the booking in the way, as in assigning.
    let mut savepoint = tx.begin().await?;
    let updated = sqlx::query_scalar(
        "update reservation_room set stay = daterange(lower(stay), $2), version = version + 1
         where id = $1
         returning version",
    )
    .bind(id)
    .bind(new_check_out)
    .fetch_one(&mut *savepoint)
    .await;
    let version: i32 = match updated {
        Ok(version) => {
            savepoint.commit().await?;
            version
        }
        Err(err) if violates(&err, "reservation_room_no_double_booking") => {
            savepoint.rollback().await?;
            return Err(room_taken_conflict(tx, room, &number, id, night, new_check_out).await?);
        }
        Err(err) => return Err(err.into()),
    };

    sqlx::query("update inventory_day set sold = sold + 1 where property_id = $1 and room_type_id = $2 and date = $3")
        .bind(property)
        .bind(stay.room_type_id)
        .bind(night)
        .execute(&mut **tx)
        .await?;
    let priced = sqlx::query(
        "insert into reservation_night (tenant_id, property_id, reservation_room_id, date, room_amount, meal_amount,
                                        currency)
         select tenant_id, property_id, reservation_room_id, $2, room_amount, meal_amount, currency
         from reservation_night where reservation_room_id = $1
         order by date desc
         limit 1",
    )
    .bind(id)
    .bind(night)
    .execute(&mut **tx)
    .await?;
    if priced.rows_affected() != 1 {
        return Err(ReservationsError::Conflict("the stay has no night to take tonight's price from".into()));
    }
    bump_reservation(tx, stay.reservation_id).await?;
    let total = total(tx, id).await?;

    let data = serde_json::json!({
        "reservation_id": stay.reservation_id,
        "from": stay.check_out,
        "to": new_check_out,
        "mode": mode(actor),
    });
    audit(tx, tenant, actor, "reservation_room.extended", "reservation_room", id, data).await?;
    let keys = [reservations_key(property), reservation_key(stay.reservation_id), housekeeping::hk_key(property)]
        .into_iter()
        .chain(rooms::month_keys(property, night, new_check_out))
        .chain(rooms::tape_keys(property, stay.check_in, new_check_out))
        .collect();
    notify(tx, tenant, property, keys).await?;

    Ok(ChangedStay {
        id,
        reservation_id: stay.reservation_id,
        version,
        check_in: stay.check_in,
        check_out: new_check_out,
        total,
        currency: stay.currency,
    })
}

/// Locks the stay `id` of the property and checks its version.
async fn lock_stay(
    tx: &mut Tx,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
) -> Result<(StayRow, RoomStatus), ReservationsError> {
    let row: Option<StayRow> = sqlx::query_as(
        "select reservation_id, room_type_id, room_id, status, lower(stay) as check_in, upper(stay) as check_out,
                currency, version
         from reservation_room where id = $1 and property_id = $2
         for update",
    )
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    let stay = row.ok_or(ReservationsError::NotFound("reservation room"))?;
    if stay.version != expected_version {
        return Err(ReservationsError::VersionMismatch("reservation room"));
    }
    let status = RoomStatus::parse(&stay.status).ok_or_else(|| decode_error("status", &stay.status))?;
    Ok((stay, status))
}

/// The stay's room and meal amounts, all nights.
async fn total(tx: &mut Tx, id: Uuid) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "select coalesce(sum(room_amount + meal_amount), 0)::bigint from reservation_night where reservation_room_id = $1",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await
}

/// The parts of a `reservation_room` that delaying and extending read.
#[derive(sqlx::FromRow)]
struct StayRow {
    reservation_id: Uuid,
    room_type_id: Uuid,
    room_id: Option<Uuid>,
    status: String,
    check_in: Date,
    check_out: Date,
    currency: String,
    version: i32,
}
