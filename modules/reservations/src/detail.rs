//! One reservation as its detail view shows it: its rooms with their nights and terms, what cancelling each
//! would cost today, and its history.

use crate::accounts::AccountKind;
use crate::guests::COLUMNS as GUEST_COLUMNS;
use crate::reservations::totals;
use crate::{CancellationTerms, Guest, ReservationsError, Source, Total, business_date, cancellation_penalty};
use db::Tx;
use domain::{Action, RoomStatus};
use rates::MealPlan;
use sqlx::types::Json;
use std::collections::HashMap;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationDetail {
    pub id: Uuid,
    pub confirmation_no: String,
    /// Derived from the rooms' statuses ([`domain::reservation_status`]).
    pub status: RoomStatus,
    pub source: Source,
    pub notes: String,
    pub created_at: OffsetDateTime,
    pub version: i32,
    pub booker: Guest,
    /// The company or travel agent this reservation is billed to; `None` if it is billed to the guest.
    pub account: Option<AccountRef>,
    /// What the rooms that are not cancelled cost, per currency, in the order the rooms first use each.
    pub totals: Vec<Total>,
    /// In the order they were booked.
    pub rooms: Vec<RoomDetail>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomDetail {
    pub id: Uuid,
    pub version: i32,
    pub status: RoomStatus,
    pub room_type: RoomTypeRef,
    /// `None` until a room is assigned.
    pub room: Option<RoomRef>,
    pub check_in: Date,
    pub check_out: Date,
    pub adults: i32,
    pub children: i32,
    pub rate_plan: RatePlanRef,
    pub meal_plan: MealPlan,
    pub primary_guest: Guest,
    /// Other guests staying in the room, besides the primary guest, masked the same way.
    pub occupants: Vec<Guest>,
    /// Each night's price as booked, by date.
    pub nights: Vec<Night>,
    pub total: i64,
    pub currency: String,
    /// The plan's cancellation policy when the room was booked; `None` if it had none.
    pub cancellation_terms: Option<CancellationTerms>,
    /// What cancelling on the business date would cost; `None` if the room can't be cancelled.
    pub cancellation_penalty: Option<i64>,
    pub cancelled_at: Option<OffsetDateTime>,
    /// The penalty recorded when the room was cancelled.
    pub recorded_penalty: Option<i64>,
    pub checked_in_at: Option<OffsetDateTime>,
    pub checked_in_business_date: Option<Date>,
    pub checked_out_at: Option<OffsetDateTime>,
    /// Whether [`crate::check_in`] would accept this room right now, per [`crate::stay::can_check_in`] -- so
    /// the SPA never has to re-derive the rule to decide whether to show the button.
    pub can_check_in: bool,
    /// As [`Self::can_check_in`], for [`crate::undo_check_in`] via [`crate::stay::can_undo_check_in`].
    pub can_undo_check_in: bool,
    /// As [`Self::can_check_in`], for [`crate::check_out`] via [`crate::stay::can_check_out`].
    pub can_check_out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomTypeRef {
    pub id: Uuid,
    pub code: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomRef {
    pub id: Uuid,
    pub number: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RatePlanRef {
    pub id: Uuid,
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRef {
    pub id: Uuid,
    pub name: String,
    pub kind: AccountKind,
}

/// A night's price as booked, in minor units of the room's currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Night {
    pub date: Date,
    pub room: i64,
    pub meal: i64,
}

/// One audit entry of a reservation or of one of its rooms.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub action: String,
    pub at: OffsetDateTime,
    /// `None` once the user is deleted.
    pub actor_name: Option<String>,
    pub data: serde_json::Value,
}

#[derive(sqlx::FromRow)]
struct ReservationRow {
    confirmation_no: String,
    source: String,
    notes: String,
    created_at: OffsetDateTime,
    version: i32,
    booker_guest_id: Uuid,
    account_id: Option<Uuid>,
}

#[derive(sqlx::FromRow)]
struct RoomRow {
    id: Uuid,
    version: i32,
    status: String,
    room_type_id: Uuid,
    room_type_code: String,
    room_type_name: String,
    room_id: Option<Uuid>,
    room_number: Option<String>,
    check_in: Date,
    check_out: Date,
    adults: i32,
    children: i32,
    rate_plan_id: Uuid,
    rate_plan_code: String,
    meal_plan: String,
    primary_guest_id: Uuid,
    currency: String,
    cancellation_terms: Option<Json<CancellationTerms>>,
    cancelled_at: Option<OffsetDateTime>,
    cancellation_penalty: Option<i64>,
    checked_in_at: Option<OffsetDateTime>,
    checked_in_business_date: Option<Date>,
    checked_out_at: Option<OffsetDateTime>,
}

/// The reservation `id` of the property, in six queries whatever its size (seven when it is billed to an
/// account). `NotFound` if the property has no such reservation.
pub async fn get_reservation(tx: &mut Tx, property: Uuid, id: Uuid) -> Result<ReservationDetail, ReservationsError> {
    let reservation: ReservationRow = sqlx::query_as(
        "select confirmation_no, source, notes, created_at, version, booker_guest_id, account_id
         from reservation where id = $1 and property_id = $2",
    )
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ReservationsError::NotFound("reservation"))?;
    let today = business_date(tx, property).await?;
    let account = match reservation.account_id {
        Some(account_id) => {
            let (name, kind): (String, String) = sqlx::query_as("select name, kind from account where id = $1")
                .bind(account_id)
                .fetch_one(&mut **tx)
                .await?;
            Some(AccountRef {
                id: account_id,
                name,
                kind: AccountKind::parse(&kind).ok_or_else(|| crate::decode_error("kind", &kind))?,
            })
        }
        None => None,
    };
    let rooms: Vec<RoomRow> = sqlx::query_as(
        "select rr.id, rr.version, rr.status, rt.id as room_type_id, rt.code as room_type_code,
                rt.name as room_type_name, room.id as room_id, room.number as room_number,
                lower(rr.stay) as check_in, upper(rr.stay) as check_out, rr.adults, rr.children,
                rp.id as rate_plan_id, rp.code as rate_plan_code, rr.meal_plan, rr.primary_guest_id, rr.currency,
                rr.cancellation_terms, rr.cancelled_at, rr.cancellation_penalty,
                rr.checked_in_at, rr.checked_in_business_date, rr.checked_out_at
         from reservation_room rr
         join room_type rt on rt.id = rr.room_type_id
         join rate_plan rp on rp.id = rr.rate_plan_id
         left join room on room.id = rr.room_id
         where rr.reservation_id = $1
         order by rr.id",
    )
    .bind(id)
    .fetch_all(&mut **tx)
    .await?;
    let room_ids: Vec<Uuid> = rooms.iter().map(|room| room.id).collect();
    let night_rows: Vec<(Uuid, Date, i64, i64)> = sqlx::query_as(
        "select reservation_room_id, date, room_amount, meal_amount from reservation_night
         where reservation_room_id = any($1)
         order by reservation_room_id, date",
    )
    .bind(&room_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut nights: HashMap<Uuid, Vec<Night>> = HashMap::new();
    for (room, date, room_amount, meal) in night_rows {
        nights.entry(room).or_default().push(Night { date, room: room_amount, meal });
    }
    let occupant_links: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "select reservation_room_id, guest_id from reservation_guest
         where reservation_room_id = any($1)
         order by reservation_room_id, guest_id",
    )
    .bind(&room_ids)
    .fetch_all(&mut **tx)
    .await?;

    let mut guest_ids: Vec<Uuid> = rooms.iter().map(|room| room.primary_guest_id).collect();
    guest_ids.push(reservation.booker_guest_id);
    guest_ids.extend(occupant_links.iter().map(|(_, guest_id)| *guest_id));
    let guests: Vec<Guest> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("select {GUEST_COLUMNS} from guest where id = any($1)")))
            .bind(&guest_ids)
            .fetch_all(&mut **tx)
            .await?;
    let guests: HashMap<Uuid, Guest> = guests.into_iter().map(|guest| (guest.id, guest)).collect();
    let guest = |id: Uuid| guests.get(&id).cloned().ok_or_else(|| crate::decode_error("guest", &id.to_string()));
    let mut occupants_by_room: HashMap<Uuid, Vec<Guest>> = HashMap::new();
    for (room_id, guest_id) in occupant_links {
        occupants_by_room.entry(room_id).or_default().push(guest(guest_id)?);
    }

    let mut details = Vec::with_capacity(rooms.len());
    for row in rooms {
        let status = RoomStatus::parse(&row.status).ok_or_else(|| crate::decode_error("status", &row.status))?;
        let meal_plan =
            MealPlan::parse(&row.meal_plan).ok_or_else(|| crate::decode_error("meal_plan", &row.meal_plan))?;
        let nights = nights.remove(&row.id).unwrap_or_default();
        let terms = row.cancellation_terms.map(|terms| terms.0);
        let cancellation_penalty = domain::transition(status, Action::Cancel).ok().map(|_| {
            let stay: Vec<(Date, i64, i64)> = nights.iter().map(|night| (night.date, night.room, night.meal)).collect();
            cancellation_penalty(terms.as_ref(), &stay, row.check_in, today)
        });
        let room_assigned = row.room_id.is_some();
        details.push(RoomDetail {
            id: row.id,
            version: row.version,
            status,
            room_type: RoomTypeRef { id: row.room_type_id, code: row.room_type_code, name: row.room_type_name },
            room: row.room_id.zip(row.room_number).map(|(id, number)| RoomRef { id, number }),
            check_in: row.check_in,
            check_out: row.check_out,
            adults: row.adults,
            children: row.children,
            rate_plan: RatePlanRef { id: row.rate_plan_id, code: row.rate_plan_code },
            meal_plan,
            primary_guest: guest(row.primary_guest_id)?,
            occupants: occupants_by_room.remove(&row.id).unwrap_or_default(),
            total: nights.iter().map(|night| night.room + night.meal).sum(),
            nights,
            currency: row.currency,
            cancellation_terms: terms,
            cancellation_penalty,
            cancelled_at: row.cancelled_at,
            recorded_penalty: row.cancellation_penalty,
            checked_in_at: row.checked_in_at,
            checked_in_business_date: row.checked_in_business_date,
            checked_out_at: row.checked_out_at,
            can_check_in: crate::stay::can_check_in(status, row.check_in, today, room_assigned),
            can_undo_check_in: crate::stay::can_undo_check_in(status, row.checked_in_business_date, today),
            can_check_out: crate::stay::can_check_out(status),
        });
    }

    let statuses: Vec<RoomStatus> = details.iter().map(|room| room.status).collect();
    let status = domain::reservation_status(&statuses).ok_or_else(|| crate::decode_error("status", "no rooms"))?;
    let totals = totals(
        details
            .iter()
            .filter(|room| room.status != RoomStatus::Cancelled)
            .map(|room| (room.currency.as_str(), room.total)),
    );
    Ok(ReservationDetail {
        id,
        confirmation_no: reservation.confirmation_no,
        status,
        source: Source::parse(&reservation.source).ok_or_else(|| crate::decode_error("source", &reservation.source))?,
        notes: reservation.notes,
        created_at: reservation.created_at,
        version: reservation.version,
        booker: guest(reservation.booker_guest_id)?,
        account,
        totals,
        rooms: details,
    })
}

/// The audit entries of the reservation `id` of the property and of its rooms, newest first. Empty if the
/// property has no such reservation.
pub async fn reservation_history(tx: &mut Tx, property: Uuid, id: Uuid) -> Result<Vec<HistoryEntry>, sqlx::Error> {
    let rows: Vec<(String, OffsetDateTime, Option<String>, serde_json::Value)> = sqlx::query_as(
        "select a.action, a.at, u.display_name, a.data
         from audit_log a left join app_user u on u.id = a.actor_user_id
         where a.entity_id = any(array(
                 select r.id from reservation r where r.id = $1 and r.property_id = $2
                 union all
                 select rr.id from reservation_room rr where rr.reservation_id = $1 and rr.property_id = $2))
           and a.entity in ('reservation', 'reservation_room')
         order by a.at desc, a.id desc",
    )
    .bind(id)
    .bind(property)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(|(action, at, actor_name, data)| HistoryEntry { action, at, actor_name, data }).collect())
}
