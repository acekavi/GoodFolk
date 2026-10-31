//! Booking on the site: a guest's confirmed, pay-at-the-hotel reservation, priced again on the server, with a manage
//! link whose token is returned once and stored only as its SHA-256, and the confirmation email queued with it.

use crate::confirmation::booked_totals;
use crate::search::{MAX_ROOMS, SearchRequest, search};
use crate::{IbeError, RoomAsk};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use db::{TenantId, Tx};
use rates::{MealPlan, Residency, Segment};
use reservations::{NewGuest, NewReservation, NewReservationRoom, ReservationsError, Source};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use time::{Date, Time};
use utoipa::ToSchema;
use uuid::Uuid;

/// Most characters of a booking's special requests.
pub const MAX_SPECIAL_REQUESTS: usize = 500;
/// How many days after check-out a booking's manage link works, to the end of the last one in the property's time
/// zone.
pub const MANAGE_DAYS_AFTER_CHECK_OUT: i64 = 30;

time::serde::format_description!(hour_minute, Time, "[hour]:[minute]");

/// A booking as the site's form sends it. Prices are never taken from it: `expected_totals` is what the guest saw.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct BookingRequest {
    /// The guest's answer to "Are you a resident of Sri Lanka?"; the booker's residency, which every room is priced for.
    pub residency: Residency,
    pub booker: Booker,
    pub rooms: Vec<BookRoom>,
    pub check_in: Date,
    pub check_out: Date,
    /// Up to 500 characters, kept as the reservation's notes.
    #[serde(default)]
    pub special_requests: String,
    /// The total the guest agreed to per currency, in minor units, taxes included.
    pub expected_totals: BTreeMap<String, i64>,
    /// The guest agreed to the hotel's booking terms and privacy notice.
    pub agreed: bool,
}

/// The person booking, who gets the confirmation email.
#[derive(Clone, Deserialize, ToSchema)]
pub struct Booker {
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub phone: String,
    /// The country of residence, ISO 3166-1 alpha-2 such as `LK`.
    pub country: String,
    /// When the guest expects to arrive, `HH:MM`.
    #[serde(default, with = "hour_minute::option")]
    #[schema(value_type = Option<String>, example = "14:30")]
    pub arrival_time: Option<Time>,
}

/// The booker's name, email and phone stay out of logs.
impl std::fmt::Debug for Booker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Booker").field("country", &self.country).finish_non_exhaustive()
    }
}

/// One room of a booking, as one of the search's offers.
#[derive(Clone, Deserialize, ToSchema)]
pub struct BookRoom {
    pub room_type_id: Uuid,
    pub rate_plan_id: Uuid,
    #[schema(value_type = String)]
    pub meal_plan: MealPlan,
    pub adults: i32,
    pub children: i32,
    /// Who stays in the room; the booker when absent.
    #[serde(default)]
    pub occupant_name: Option<String>,
}

/// The occupant's name stays out of logs.
impl std::fmt::Debug for BookRoom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BookRoom")
            .field("room_type_id", &self.room_type_id)
            .field("rate_plan_id", &self.rate_plan_id)
            .field("meal_plan", &self.meal_plan)
            .field("adults", &self.adults)
            .field("children", &self.children)
            .field("occupant_name", &self.occupant_name.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// A booking made. `token` is the manage link's, given out only here.
#[derive(Clone)]
pub struct Booked {
    pub reservation_id: Uuid,
    pub confirmation_no: String,
    pub token: String,
}

impl std::fmt::Debug for Booked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Booked")
            .field("reservation_id", &self.reservation_id)
            .field("confirmation_no", &self.confirmation_no)
            .field("token", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BookingError {
    /// The booking is not one the site sells, such as too many guests for a room or a plan for other guests.
    #[error("{0}")]
    Invalid(String),
    /// The stay costs something else now; the guest confirms the new totals.
    #[error("the price has changed")]
    PriceChanged { totals: BTreeMap<String, i64> },
    /// A room chosen is no longer offered for the stay.
    #[error("that room was just booked")]
    SoldOut,
    /// The guest did not agree to the booking terms and privacy notice.
    #[error("agree to the booking terms and the privacy notice to book")]
    NotAgreed,
    /// Anything else: the property's prices or data could not be read.
    #[error(transparent)]
    Ibe(#[from] IbeError),
}

impl From<sqlx::Error> for BookingError {
    fn from(err: sqlx::Error) -> Self {
        Self::Ibe(err.into())
    }
}

impl From<ReservationsError> for BookingError {
    fn from(err: ReservationsError) -> Self {
        match err {
            // The only conflict booking has: no room of a type left on a night.
            ReservationsError::Conflict(_) => Self::SoldOut,
            ReservationsError::Invalid(message) => Self::Invalid(message),
            ReservationsError::NotFound(what) => Self::Ibe(IbeError::NotFound(what)),
            ReservationsError::Database(err) => err.into(),
            ReservationsError::VersionMismatch(_) => Self::Invalid(err.to_string()),
        }
    }
}

/// A new manage-link token: 32 bytes from the OS random number generator, base64url without padding.
fn new_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS random number generator available");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// What a manage-link token is stored as.
pub fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Books `request` on `property`'s site for its tenant `tenant`: one confirmed reservation from the booking site, paid
/// at the hotel (no guarantee), with no staff member behind it.
///
/// Before anything is written:
/// - the guest agreed to the terms (`NotAgreed`), and the special requests fit;
/// - every room holds its guests and is on an active `IBE` plan selling its room type and meal plan for the guest's
///   residency (`Invalid`, as is a stay [`search`] refuses);
/// - [`search`] prices the stay again: a room it no longer offers is `SoldOut`, and totals per currency other than
///   `expected_totals` are `PriceChanged` with the new ones.
///
/// Then the booker becomes a new guest (never matched to another by email), each named occupant a guest with just a
/// name, and the reservation is made by [`reservations::create_reservation_from_site`]; a night with no room left is
/// `SoldOut`. That command prices the nights again under the inventory lock, so their totals with the taxes on the
/// bill are compared with `expected_totals` once more: a rate changed since the search is `PriceChanged` too, and the
/// caller's rollback undoes everything written. The manage token is stored as its SHA-256, valid to the end of the 30th day after check-out in the
/// property's time zone; the confirmation email is queued; and `ibe.booking_created` is audited with the room count,
/// never the email or phone. The arrival time, when given, heads the notes, as front desk reads them.
pub async fn book(
    tx: &mut Tx,
    tenant: TenantId,
    property: Uuid,
    request: &BookingRequest,
) -> Result<Booked, BookingError> {
    if !request.agreed {
        return Err(BookingError::NotAgreed);
    }
    if request.special_requests.chars().count() > MAX_SPECIAL_REQUESTS {
        return Err(BookingError::Invalid(format!("special requests are at most {MAX_SPECIAL_REQUESTS} characters")));
    }
    if !(1..=MAX_ROOMS).contains(&request.rooms.len()) {
        return Err(BookingError::Invalid(format!("a booking is 1 to {MAX_ROOMS} rooms")));
    }
    check_rooms(tx, property, request).await?;

    let asks = request.rooms.iter().map(|room| RoomAsk { adults: room.adults, children: room.children }).collect();
    let stay = SearchRequest {
        check_in: request.check_in,
        check_out: request.check_out,
        residency: request.residency,
        rooms: asks,
    };
    let offered = search(tx, property, &stay).await.map_err(|err| match err {
        IbeError::Invalid(message) => BookingError::Invalid(message),
        other => other.into(),
    })?;
    let mut totals: BTreeMap<String, i64> = BTreeMap::new();
    for (index, room) in request.rooms.iter().enumerate() {
        let index = u8::try_from(index).expect("at most MAX_ROOMS rooms");
        let (plan, offer) = offered
            .room_types
            .iter()
            .filter(|offered| offered.room_type_id == room.room_type_id)
            .flat_map(|offered| offered.plans.iter().filter(|plan| plan.rate_plan_id == room.rate_plan_id))
            .find_map(|plan| {
                let offer = plan
                    .meal_plans
                    .iter()
                    .find(|offer| offer.meal_plan == room.meal_plan && offer.rooms.contains(&index))?;
                Some((plan, offer))
            })
            .ok_or(BookingError::SoldOut)?;
        *totals.entry(plan.currency.clone()).or_default() += offer.total;
    }
    if totals != request.expected_totals {
        return Err(BookingError::PriceChanged { totals });
    }

    let booker = &request.booker;
    let required = |value: &str| Some(value.trim().to_owned()).filter(|value| !value.is_empty());
    let (Some(email), Some(phone), Some(country)) =
        (required(&booker.email), required(&booker.phone), required(&booker.country))
    else {
        return Err(BookingError::Invalid("the email, phone and country are required".into()));
    };
    let guest = |first_name: String, last_name: String| NewGuest {
        first_name,
        last_name,
        email: None,
        phone: None,
        country: None,
        residency: request.residency,
        notes: String::new(),
        id_doc: None,
    };
    let booker_guest = NewGuest {
        email: Some(email),
        phone: Some(phone),
        country: Some(country),
        ..guest(booker.first_name.clone(), booker.last_name.clone())
    };
    let booker_id = reservations::create_guest_from_site(tx, tenant, booker_guest).await?.id;
    let mut rooms = Vec::with_capacity(request.rooms.len());
    for room in &request.rooms {
        let occupant = match room.occupant_name.as_deref().map(str::trim).filter(|name| !name.is_empty()) {
            Some(name) => {
                let (first, last) = name.rsplit_once(char::is_whitespace).unwrap_or(("", name));
                let created = guest(first.trim().to_owned(), last.to_owned());
                Some(reservations::create_guest_from_site(tx, tenant, created).await?.id)
            }
            None => None,
        };
        rooms.push(NewReservationRoom {
            room_type_id: room.room_type_id,
            rate_plan_id: room.rate_plan_id,
            meal_plan: room.meal_plan,
            check_in: request.check_in,
            check_out: request.check_out,
            adults: room.adults,
            children: room.children,
            primary_guest_id: occupant,
        });
    }
    let notes = match booker.arrival_time {
        Some(time) => format!("Arrival time: {:02}:{:02}\n{}", time.hour(), time.minute(), request.special_requests),
        None => request.special_requests.clone(),
    };
    let input = NewReservation { booker_guest_id: booker_id, source: Source::Ibe, notes, account_id: None, rooms };
    let created = reservations::create_reservation_from_site(tx, tenant, property, input).await?;
    // The nights were priced again under the inventory lock: a rate changed since the search above is seen here, and
    // the error rolls the booking back.
    let booked = booked_totals(tx, property, created.id).await?;
    if booked != request.expected_totals {
        return Err(BookingError::PriceChanged { totals: booked });
    }

    let token = new_token();
    sqlx::query(
        "insert into reservation_access_token (tenant_id, reservation_id, sha256, expires_at)
         select $1, $2, $3, ($4::date + $5::int + 1)::timestamp at time zone p.timezone
         from property p where p.id = $6",
    )
    .bind(tenant.0)
    .bind(created.id)
    .bind(token_hash(&token))
    .bind(request.check_out)
    .bind(i32::try_from(MANAGE_DAYS_AFTER_CHECK_OUT).expect("a small number of days"))
    .bind(property)
    .execute(&mut **tx)
    .await?;
    let job = mail::MailJob::IbeConfirmation { tenant_id: tenant.0, reservation_id: created.id, token: token.clone() };
    let payload = serde_json::to_value(&job).expect("a mail job always serializes");
    events::enqueue(tx, mail::SEND_TOPIC, &payload).await?;
    sqlx::query(
        "insert into audit_log (id, tenant_id, actor_user_id, action, entity, entity_id, data)
         values ($1, $2, null, 'ibe.booking_created', 'reservation', $3, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(created.id)
    .bind(serde_json::json!({ "actor": reservations::SITE_ACTOR, "rooms": created.rooms.len() }))
    .execute(&mut **tx)
    .await?;
    Ok(Booked { reservation_id: created.id, confirmation_no: created.confirmation_no, token })
}

#[derive(sqlx::FromRow)]
struct RoomTypeLimits {
    id: Uuid,
    code: String,
    max_adults: i32,
    max_children: i32,
    max_occupancy: i32,
}

/// `Invalid` unless every room of `request` holds its guests and is on an active `IBE` plan of the property selling
/// its room type and meal plan to guests of the booking's residency.
async fn check_rooms(tx: &mut Tx, property: Uuid, request: &BookingRequest) -> Result<(), BookingError> {
    let ids: Vec<Uuid> = request.rooms.iter().map(|room| room.room_type_id).collect();
    let limits: HashMap<Uuid, RoomTypeLimits> = sqlx::query_as::<_, RoomTypeLimits>(
        "select id, code, max_adults, max_children, max_occupancy from room_type
         where property_id = $1 and id = any($2) and active",
    )
    .bind(property)
    .bind(&ids)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .map(|limits| (limits.id, limits))
    .collect();
    let plans = rates::list_rate_plans(tx, property).await?;
    let invalid = |message: String| Err(BookingError::Invalid(message));
    for room in &request.rooms {
        let Some(limits) = limits.get(&room.room_type_id) else { return invalid("no such room type".into()) };
        if room.adults < 1 || room.children < 0 {
            return invalid("each room has at least one adult".into());
        }
        if room.adults > limits.max_adults
            || room.children > limits.max_children
            || room.adults + room.children > limits.max_occupancy
        {
            return invalid(format!(
                "a {} room holds {} adults, {} children and {} guests in all",
                limits.code, limits.max_adults, limits.max_children, limits.max_occupancy
            ));
        }
        let plan = plans
            .iter()
            .find(|plan| plan.id == room.rate_plan_id && plan.active && plan.segment == Segment::Ibe)
            .ok_or_else(|| BookingError::Invalid("no such rate on the booking site".into()))?;
        if plan.residency.is_some_and(|residency| residency != request.residency) {
            let guests = match plan.residency {
                Some(Residency::Resident) => "residents of Sri Lanka",
                _ => "guests from abroad",
            };
            return invalid(format!("{} is only for {guests}", plan.name));
        }
        if !plan.room_type_ids.contains(&room.room_type_id) || !plan.allowed_meal_plans.contains(&room.meal_plan) {
            return invalid(format!("{} does not sell this room and meal plan", plan.name));
        }
    }
    Ok(())
}
