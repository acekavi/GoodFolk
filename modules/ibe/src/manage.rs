//! A guest's manage link: the booking it opens, shown without anything internal, and cancelling it online.
//!
//! The link's token is looked up by its SHA-256 in one indexed query that also checks its expiry and the site's
//! property, so a wrong, expired or other property's token is the same [`ManageError::Gone`], found with the same work.

use crate::IbeError;
use crate::booking::token_hash;
use crate::confirmation::booked_rooms;
use crate::search::terms_text;
use db::{TenantId, Tx};
use domain::RoomStatus;
use rates::MealPlan;
use reservations::{CancellationTerms, ReservationsError, SITE_ACTOR, cancellation_penalty};
use serde::Serialize;
use sqlx::types::Json;
use std::collections::BTreeMap;
use time::Date;
use utoipa::ToSchema;
use uuid::Uuid;

/// The reason a guest's own cancellation is recorded with.
pub const CANCEL_REASON: &str = "Cancelled by the guest online";
/// What a guest is told when the booking can only be cancelled by the hotel.
const CONTACT_HOTEL: &str = "This booking can no longer be cancelled online; please contact the hotel";

/// A booking as its manage page shows it. Amounts are in minor units of their currency, with the taxes on the bill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ManageView {
    pub property_name: String,
    /// To change the booking, the guest contacts the hotel.
    pub contact_email: Option<String>,
    pub contact_phone: Option<String>,
    pub confirmation_no: String,
    pub check_in: Date,
    pub check_out: Date,
    /// The reservation's status: `tentative`, `confirmed`, `checked_in`, `checked_out`, `cancelled` or `no_show`.
    pub status: String,
    pub rooms: Vec<ManagedRoom>,
    pub totals: BTreeMap<String, i64>,
    /// Each distinct wording of the rooms' booked cancellation terms, in room order.
    pub cancellation: Vec<String>,
    /// Whether the guest can cancel online: every room tentative or confirmed, and check-in after the business date.
    pub cancellable: bool,
    /// What cancelling now costs per currency, while the booking is cancellable.
    pub penalty_if_cancelled: BTreeMap<String, i64>,
    /// What cancelling cost per currency, for the rooms cancelled.
    pub cancellation_penalty: BTreeMap<String, i64>,
}

/// One booked room as the guest sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ManagedRoom {
    pub room_type: String,
    pub plan: String,
    #[schema(value_type = String)]
    pub meal_plan: MealPlan,
    pub adults: i32,
    pub children: i32,
    pub status: String,
    pub currency: String,
    pub total: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum ManageError {
    /// The token is wrong, expired, or another property's: all alike, so a token cannot be probed.
    #[error("This link has expired or is not valid")]
    Gone,
    /// The booking cannot be cancelled online, such as once a room is checked in.
    #[error("{0}")]
    NotCancellable(String),
    #[error("This booking is already cancelled")]
    AlreadyCancelled,
    #[error(transparent)]
    Ibe(#[from] IbeError),
}

impl From<sqlx::Error> for ManageError {
    fn from(err: sqlx::Error) -> Self {
        Self::Ibe(err.into())
    }
}

impl From<ReservationsError> for ManageError {
    fn from(err: ReservationsError) -> Self {
        match err {
            // A room that cannot be cancelled here, such as a penalty with no rate to post it at: the hotel's to settle.
            ReservationsError::Conflict(_) => Self::NotCancellable(CONTACT_HOTEL.into()),
            ReservationsError::NotFound(what) => Self::Ibe(IbeError::NotFound(what)),
            ReservationsError::Database(err) => err.into(),
            other @ (ReservationsError::Invalid(_) | ReservationsError::VersionMismatch(_)) => {
                Self::Ibe(IbeError::Invalid(other.to_string()))
            }
        }
    }
}

#[derive(sqlx::FromRow)]
struct Found {
    reservation_id: Uuid,
    property_name: String,
    vat_registered: bool,
    business_date: Date,
    contact_email: Option<String>,
    contact_phone: Option<String>,
    confirmation_no: String,
}

/// The booking of the unexpired `token` on `property`'s site, or `Gone`.
async fn find(tx: &mut Tx, property: Uuid, token: &str) -> Result<Found, ManageError> {
    let found: Option<Found> = sqlx::query_as(
        "select r.id as reservation_id, p.name as property_name, p.vat_registered, p.business_date,
                s.contact_email, s.contact_phone, r.confirmation_no
         from reservation_access_token t
         join reservation r on r.id = t.reservation_id
         join property p on p.id = r.property_id
         left join ibe_site s on s.property_id = r.property_id
         where t.sha256 = $1 and t.expires_at > now() and r.property_id = $2 and r.source = 'ibe'",
    )
    .bind(token_hash(token))
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    found.ok_or(ManageError::Gone)
}

/// The booking whose manage link carries `token`, on `property`'s site. `Gone` for a token that is wrong, expired
/// (after the end of the 30th day after check-out, in the property's time zone) or another property's.
pub async fn manage(tx: &mut Tx, property: Uuid, token: &str) -> Result<ManageView, ManageError> {
    let found = find(tx, property, token).await?;
    Ok(view(tx, property, &found).await?)
}

/// Cancels the whole booking whose manage link carries `token`, as the guest: every room cancelled under its booked
/// terms (the penalty is never waived) with the reason [`CANCEL_REASON`] and no staff actor, `ibe.booking_cancelled`
/// audited, and the cancellation email queued. All or nothing: the caller's rollback undoes a failure.
///
/// The rooms are locked first, so of two cancels at once the second finds the booking cancelled. `Gone` as
/// [`manage`] has it; `AlreadyCancelled` when every room is cancelled; `NotCancellable` unless every room is
/// tentative or confirmed with check-in after the business date.
pub async fn cancel(tx: &mut Tx, tenant: TenantId, property: Uuid, token: &str) -> Result<ManageView, ManageError> {
    let found = find(tx, property, token).await?;
    let rooms: Vec<(Uuid, String, i32, Date)> = sqlx::query_as(
        "select id, status, version, lower(stay) from reservation_room where reservation_id = $1
         order by id for update",
    )
    .bind(found.reservation_id)
    .fetch_all(&mut **tx)
    .await?;
    let today: Date = sqlx::query_scalar("select business_date from property where id = $1 for share")
        .bind(property)
        .fetch_one(&mut **tx)
        .await?;
    let mut open = Vec::with_capacity(rooms.len());
    let mut stays = Vec::with_capacity(rooms.len());
    for (id, status, version, check_in) in &rooms {
        let status = RoomStatus::parse(status).ok_or_else(|| room_status_error(status))?;
        stays.push((status, *check_in));
        if status != RoomStatus::Cancelled {
            open.push((*id, *version));
        }
    }
    match closing(stays, today) {
        Closing::Done => return Err(ManageError::AlreadyCancelled),
        Closing::Blocked => return Err(ManageError::NotCancellable(CONTACT_HOTEL.into())),
        Closing::Open => {}
    }
    for &(id, version) in &open {
        reservations::cancel_room(tx, tenant, None, property, id, version, Some(CANCEL_REASON)).await.inspect_err(
            |err| {
                if let ReservationsError::Conflict(message) = err {
                    tracing::warn!(reservation = %found.reservation_id, room = %id, conflict = %message,
                        "a guest's online cancellation was refused; the hotel has to settle it");
                }
            },
        )?;
    }
    let closed: Vec<Uuid> = open.iter().map(|&(id, _)| id).collect();
    let job = mail::MailJob::IbeCancellation {
        tenant_id: tenant.0,
        reservation_id: found.reservation_id,
        rooms: closed.clone(),
    };
    let payload = serde_json::to_value(&job).expect("a mail job always serializes");
    events::enqueue(tx, mail::SEND_TOPIC, &payload).await.map_err(IbeError::from)?;
    sqlx::query(
        "insert into audit_log (id, tenant_id, actor_user_id, action, entity, entity_id, data)
         values ($1, $2, null, 'ibe.booking_cancelled', 'reservation', $3, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(found.reservation_id)
    .bind(serde_json::json!({ "actor": SITE_ACTOR, "rooms": closed.len(), "reason": CANCEL_REASON }))
    .execute(&mut **tx)
    .await?;
    Ok(view(tx, property, &found).await?)
}

/// What a guest's cancel does to a booking.
#[derive(Debug, PartialEq, Eq)]
enum Closing {
    /// Every room is cancelled already.
    Done,
    /// A room is checked in, checked out or a no-show, or is due to arrive on the business date or before: the hotel's
    /// to settle.
    Blocked,
    /// The rooms not cancelled yet can be cancelled online.
    Open,
}

/// How a guest's cancel treats rooms of the given status and check-in on the business date `today`. Rooms already
/// cancelled are left as they are; the rest must all be tentative or confirmed with check-in after `today`. The one
/// rule both the page and the cancel go by.
fn closing(rooms: impl IntoIterator<Item = (RoomStatus, Date)>, today: Date) -> Closing {
    let mut open = false;
    for (status, check_in) in rooms {
        match status {
            RoomStatus::Cancelled => {}
            RoomStatus::Tentative | RoomStatus::Confirmed if check_in > today => open = true,
            _ => return Closing::Blocked,
        }
    }
    if open { Closing::Open } else { Closing::Done }
}

fn room_status_error(value: &str) -> sqlx::Error {
    sqlx::Error::ColumnDecode { index: "status".into(), source: format!("unknown value {value:?}").into() }
}

/// `found`'s booking as the manage page shows it.
async fn view(tx: &mut Tx, property: Uuid, found: &Found) -> Result<ManageView, IbeError> {
    let rooms = booked_rooms(tx, property, found.vat_registered, found.reservation_id).await?;
    let ids: Vec<Uuid> = rooms.iter().map(|(room, ..)| room.id).collect();
    let nights: Vec<(Uuid, Date, i64, i64)> = sqlx::query_as(
        "select reservation_room_id, date, room_amount, meal_amount from reservation_night
         where reservation_room_id = any($1) order by date",
    )
    .bind(&ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut statuses = Vec::with_capacity(rooms.len());
    let mut managed = Vec::with_capacity(rooms.len());
    let mut totals: BTreeMap<String, i64> = BTreeMap::new();
    let mut cancellation: Vec<String> = Vec::new();
    let mut penalty_if_cancelled: BTreeMap<String, i64> = BTreeMap::new();
    let mut cancellation_fees: BTreeMap<String, i64> = BTreeMap::new();
    for (room, meal_plan, total) in &rooms {
        let status = RoomStatus::parse(&room.status).ok_or_else(|| room_status_error(&room.status))?;
        statuses.push(status);
        *totals.entry(room.currency.clone()).or_default() += total;
        let terms = room.cancellation_terms.as_ref().map(|Json(terms): &Json<CancellationTerms>| terms);
        let wording = terms.map_or_else(
            || "Free cancellation.".to_owned(),
            |terms| terms_text(&terms.rules, terms.no_show, &room.currency),
        );
        if !cancellation.contains(&wording) {
            cancellation.push(wording);
        }
        let booked: Vec<(Date, i64, i64)> =
            nights.iter().filter(|(id, ..)| *id == room.id).map(|&(_, date, room, meal)| (date, room, meal)).collect();
        if status == RoomStatus::Cancelled {
            *cancellation_fees.entry(room.currency.clone()).or_default() += room.cancellation_penalty.unwrap_or(0);
        } else {
            let now = cancellation_penalty(terms, &booked, room.check_in, found.business_date);
            *penalty_if_cancelled.entry(room.currency.clone()).or_default() += now;
        }
        managed.push(ManagedRoom {
            room_type: room.room_type.clone(),
            plan: room.plan.clone(),
            meal_plan: *meal_plan,
            adults: room.adults,
            children: room.children,
            status: status.as_str().to_owned(),
            currency: room.currency.clone(),
            total: *total,
        });
    }
    let cancellable =
        closing(rooms.iter().zip(&statuses).map(|((room, ..), status)| (*status, room.check_in)), found.business_date)
            == Closing::Open;
    if !cancellable {
        penalty_if_cancelled.clear();
    }
    let status = domain::reservation_status(&statuses).ok_or(IbeError::NotFound("reservation room"))?;
    let (check_in, check_out) = (
        rooms.iter().map(|(room, ..)| room.check_in).min().ok_or(IbeError::NotFound("reservation room"))?,
        rooms.iter().map(|(room, ..)| room.check_out).max().ok_or(IbeError::NotFound("reservation room"))?,
    );
    Ok(ManageView {
        property_name: found.property_name.clone(),
        contact_email: found.contact_email.clone(),
        contact_phone: found.contact_phone.clone(),
        confirmation_no: found.confirmation_no.clone(),
        check_in,
        check_out,
        status: status.as_str().to_owned(),
        rooms: managed,
        totals,
        cancellation,
        cancellable,
        penalty_if_cancelled,
        cancellation_penalty: cancellation_fees,
    })
}

/// A cancelled site booking as its cancellation email says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cancellation {
    pub property_name: String,
    pub contact_email: Option<String>,
    pub contact_phone: Option<String>,
    pub confirmation_no: String,
    pub booker_name: String,
    pub booker_email: String,
    pub check_in: Date,
    pub check_out: Date,
    /// What cancelling cost per currency; empty when it was free.
    pub penalty: BTreeMap<String, i64>,
}

#[derive(sqlx::FromRow)]
struct CancelledRow {
    property_name: String,
    contact_email: Option<String>,
    contact_phone: Option<String>,
    confirmation_no: String,
    first_name: String,
    last_name: String,
    email: Option<String>,
}

/// The cancellation of site booking `reservation`, charging only the `closed` rooms (those the guest's cancel closed,
/// not any the hotel cancelled before), or `None` when the tenant has no such reservation from the site, a room of it
/// is not cancelled, or the booker has no email.
pub async fn cancellation(tx: &mut Tx, reservation: Uuid, closed: &[Uuid]) -> Result<Option<Cancellation>, IbeError> {
    let row: Option<CancelledRow> = sqlx::query_as(
        "select p.name as property_name, s.contact_email, s.contact_phone, r.confirmation_no, g.first_name,
                g.last_name, g.email
         from reservation r
         join property p on p.id = r.property_id
         join guest g on g.id = r.booker_guest_id
         left join ibe_site s on s.property_id = r.property_id
         where r.id = $1 and r.source = 'ibe'",
    )
    .bind(reservation)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(CancelledRow {
        property_name,
        contact_email,
        contact_phone,
        confirmation_no,
        first_name,
        last_name,
        email,
    }) = row
    else {
        return Ok(None);
    };
    let rooms: Vec<(String, String, Option<i64>, Date, Date, Uuid)> = sqlx::query_as(
        "select status, currency::text, cancellation_penalty, lower(stay), upper(stay), id from reservation_room
         where reservation_id = $1",
    )
    .bind(reservation)
    .fetch_all(&mut **tx)
    .await?;
    let cancelled = RoomStatus::Cancelled.as_str();
    let (Some(booker_email), false) = (email, rooms.is_empty() || rooms.iter().any(|room| room.0 != cancelled)) else {
        return Ok(None);
    };
    let mut penalty: BTreeMap<String, i64> = BTreeMap::new();
    for (_, currency, fee, _, _, id) in &rooms {
        if closed.contains(id) {
            *penalty.entry(currency.clone()).or_default() += fee.unwrap_or(0);
        }
    }
    penalty.retain(|_, fee| *fee > 0);
    Ok(Some(Cancellation {
        property_name,
        contact_email,
        contact_phone,
        confirmation_no,
        booker_name: format!("{first_name} {last_name}").trim().to_owned(),
        booker_email,
        check_in: rooms.iter().map(|room| room.3).min().expect("at least one room"),
        check_out: rooms.iter().map(|room| room.4).max().expect("at least one room"),
        penalty,
    }))
}

#[cfg(test)]
mod tests {
    use super::{Closing, closing};
    use domain::RoomStatus::{Cancelled, CheckedIn, Confirmed, NoShow, Tentative};
    use time::macros::date;

    #[test]
    fn rooms_cancelled_already_are_skipped_and_the_rest_must_be_open_and_in_the_future() {
        let (today, later) = (date!(2026 - 10 - 31), date!(2026 - 11 - 1));
        assert_eq!(closing([(Confirmed, later), (Tentative, later)], today), Closing::Open);
        assert_eq!(closing([(Cancelled, today), (Confirmed, later)], today), Closing::Open);
        assert_eq!(closing([(Cancelled, later), (Cancelled, today)], today), Closing::Done);
        assert_eq!(closing([(Confirmed, today)], today), Closing::Blocked, "arriving today");
        assert_eq!(closing([(Cancelled, later), (CheckedIn, later)], today), Closing::Blocked);
        assert_eq!(closing([(Confirmed, later), (NoShow, later)], today), Closing::Blocked);
    }
}
