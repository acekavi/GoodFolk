//! Reservations: booking one or more rooms under a confirmation number, with every night's price fixed at
//! booking and the inventory counters kept exact under concurrent bookings.

use crate::guests::notes;
use crate::{ReservationsError, audit, check_window, notify, reservation_key, reservations_key};
use db::{TenantId, Tx, UserId};
use rates::{MealPlan, QuoteRequest, Residency};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use time::Date;
use uuid::Uuid;

/// Most rooms one reservation books.
pub const MAX_ROOMS_PER_RESERVATION: usize = 10;

db::text_enum!(
    /// Where a booking came from. Staff create `front_desk`, `phone` and `email` bookings; `ibe` and `channel`
    /// bookings arrive through the booking engine (Phase 9) and channels (Phase 8).
    Source { FrontDesk = "front_desk", Ibe = "ibe", Channel = "channel", Phone = "phone", Email = "email" }
);

/// A booking to make: `rooms` (1 to [`MAX_ROOMS_PER_RESERVATION`]) for the guest `booker_guest_id`.
#[derive(Debug, Clone)]
pub struct NewReservation {
    pub booker_guest_id: Uuid,
    pub source: Source,
    pub notes: String,
    pub rooms: Vec<NewReservationRoom>,
}

/// One room of a booking: a room type on a rate plan and meal plan for `[check_in, check_out)`.
#[derive(Debug, Clone)]
pub struct NewReservationRoom {
    pub room_type_id: Uuid,
    pub rate_plan_id: Uuid,
    pub meal_plan: MealPlan,
    pub check_in: Date,
    pub check_out: Date,
    pub adults: i32,
    pub children: i32,
    /// Who stays in the room; the booker when `None`. The room is priced for this guest's residency.
    pub primary_guest_id: Option<Uuid>,
}

/// A booked room and what its stay costs, in its plan's currency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct CreatedRoom {
    pub id: Uuid,
    pub room_type_id: Uuid,
    pub rate_plan_id: Uuid,
    pub meal_plan: MealPlan,
    pub check_in: Date,
    pub check_out: Date,
    pub adults: i32,
    pub children: i32,
    pub total: i64,
    pub currency: String,
}

/// The sum of the rooms booked in one currency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Total {
    pub currency: String,
    pub amount: i64,
}

/// A new reservation. Rooms may be in different currencies, so `totals` has one entry per currency, in the
/// order the rooms first use them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct CreatedReservation {
    pub id: Uuid,
    pub confirmation_no: String,
    pub version: i32,
    pub rooms: Vec<CreatedRoom>,
    pub totals: Vec<Total>,
}

/// Books `input`'s rooms, confirmed, under the property's next confirmation number.
///
/// Every stay must be inside the counter window and arrive on or after the business date, and every guest
/// named must exist (`Invalid`). The counters of every requested room type are locked over all the stays at
/// once ([`rooms::lock_days`]); a night without a free room of the type, counting the rooms this request
/// already takes, is a `Conflict` (no overbooking). Each room is priced by [`rates::load_quote`] for its
/// primary guest's residency and its nights are stored as quoted; any reason the quote gives not to sell is
/// `Invalid`, with every reason listed. Only then is the confirmation number taken, so a refused booking never
/// uses one.
pub async fn create_reservation(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    mut input: NewReservation,
) -> Result<CreatedReservation, ReservationsError> {
    if !(1..=MAX_ROOMS_PER_RESERVATION).contains(&input.rooms.len()) {
        return Err(invalid(format!("a reservation has 1 to {MAX_ROOMS_PER_RESERVATION} rooms")));
    }
    if !matches!(input.source, Source::FrontDesk | Source::Phone | Source::Email) {
        return Err(invalid("bookings made here come from the front desk, phone or email".into()));
    }
    let notes = notes(std::mem::take(&mut input.notes))?;
    if input.rooms.iter().any(|room| room.check_out <= room.check_in) {
        return Err(invalid("check-out is after check-in".into()));
    }
    let (code, today): (String, Date) = sqlx::query_as("select code, business_date from property where id = $1")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ReservationsError::NotFound("property"))?;
    for room in &input.rooms {
        check_window(today, room.check_in, room.check_out)?;
    }
    let residencies = residencies(tx, &input).await?;
    let room_types = room_type_codes(tx, property, &input.rooms).await?;

    // The locked range covers every night any room sells; it is inside the window, so every row exists.
    let from = input.rooms.iter().map(|room| room.check_in).min().expect("at least one room");
    let to = input.rooms.iter().map(|room| room.check_out).max().expect("at least one room");
    let type_ids: Vec<Uuid> = room_types.keys().copied().collect();
    rooms::extend_window(tx, property).await?;
    rooms::lock_days(tx, property, &type_ids, from, to).await?;
    check_free(tx, property, &type_ids, from, to, &input.rooms, &room_types).await?;

    let mut quotes = Vec::with_capacity(input.rooms.len());
    let mut reasons = Vec::new();
    for room in &input.rooms {
        let request = QuoteRequest {
            room_type_id: room.room_type_id,
            rate_plan_id: room.rate_plan_id,
            meal_plan: room.meal_plan,
            check_in: room.check_in,
            check_out: room.check_out,
            adults: room.adults,
            children: room.children,
            residency: residencies[&room.primary_guest_id.unwrap_or(input.booker_guest_id)],
        };
        let quote = rates::load_quote(tx, property, &request).await?;
        reasons.extend(quote.violations.iter().map(|violation| violation.message.clone()));
        quotes.push(quote);
    }
    if !reasons.is_empty() {
        return Err(invalid(reasons.join("; ")));
    }
    let terms = cancellation_terms(tx, property, &input.rooms).await?;

    let number: i64 = sqlx::query_scalar(
        "insert into property_counter (tenant_id, property_id, name, value) values ($1, $2, 'confirmation', 1)
         on conflict (property_id, name) do update set value = property_counter.value + 1
         returning value",
    )
    .bind(tenant.0)
    .bind(property)
    .fetch_one(&mut **tx)
    .await?;
    let confirmation_no = format!("{code}-{number:06}");
    let id = Uuid::now_v7();
    let version: i32 = sqlx::query_scalar(
        "insert into reservation (id, tenant_id, property_id, confirmation_no, source, booker_guest_id, notes,
                                  created_by)
         values ($1, $2, $3, $4, $5, $6, $7, $8)
         returning version",
    )
    .bind(id)
    .bind(tenant.0)
    .bind(property)
    .bind(&confirmation_no)
    .bind(input.source.as_str())
    .bind(input.booker_guest_id)
    .bind(notes)
    .bind(actor.0)
    .fetch_one(&mut **tx)
    .await?;

    let mut created = Vec::with_capacity(input.rooms.len());
    for (room, quote) in input.rooms.iter().zip(quotes) {
        let room_id = Uuid::now_v7();
        sqlx::query(
            "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, stay, adults,
                                           children, rate_plan_id, meal_plan, status, primary_guest_id, currency,
                                           cancellation_terms)
             values ($1, $2, $3, $4, $5, daterange($6, $7), $8, $9, $10, $11, 'confirmed', $12, $13, $14)",
        )
        .bind(room_id)
        .bind(tenant.0)
        .bind(property)
        .bind(id)
        .bind(room.room_type_id)
        .bind(room.check_in)
        .bind(room.check_out)
        .bind(room.adults)
        .bind(room.children)
        .bind(room.rate_plan_id)
        .bind(room.meal_plan.as_str())
        .bind(room.primary_guest_id.unwrap_or(input.booker_guest_id))
        .bind(&quote.currency)
        .bind(terms.get(&room.rate_plan_id))
        .execute(&mut **tx)
        .await?;
        let dates: Vec<Date> = quote.nights.iter().map(|night| night.date).collect();
        let room_amounts: Vec<i64> = quote.nights.iter().map(|night| night.room).collect();
        let meal_amounts: Vec<i64> = quote.nights.iter().map(|night| night.meal).collect();
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
        .bind(&quote.currency)
        .execute(&mut **tx)
        .await?;
        // Inside the rows locked above: this room's nights are within [from, to).
        sqlx::query(
            "update inventory_day set sold = sold + 1
             where property_id = $1 and room_type_id = $2 and date >= $3 and date < $4",
        )
        .bind(property)
        .bind(room.room_type_id)
        .bind(room.check_in)
        .bind(room.check_out)
        .execute(&mut **tx)
        .await?;
        created.push(CreatedRoom {
            id: room_id,
            room_type_id: room.room_type_id,
            rate_plan_id: room.rate_plan_id,
            meal_plan: room.meal_plan,
            check_in: room.check_in,
            check_out: room.check_out,
            adults: room.adults,
            children: room.children,
            total: quote.total,
            currency: quote.currency,
        });
    }

    let data = serde_json::json!({
        "confirmation_no": confirmation_no,
        "rooms": created.iter().map(|room| room.id).collect::<Vec<_>>(),
    });
    audit(tx, tenant, actor, "reservation.created", "reservation", id, data).await?;
    let months: BTreeSet<String> =
        input.rooms.iter().flat_map(|room| rooms::month_keys(property, room.check_in, room.check_out)).collect();
    let keys = [reservations_key(property), reservation_key(id)].into_iter().chain(months).collect();
    notify(tx, tenant, property, keys).await?;

    Ok(CreatedReservation { id, confirmation_no, version, totals: totals(&created), rooms: created })
}

fn invalid(message: String) -> ReservationsError {
    ReservationsError::Invalid(message)
}

/// The residency of the booker and of every primary guest, by guest id. `Invalid` if one is not a guest of
/// this tenant.
async fn residencies(tx: &mut Tx, input: &NewReservation) -> Result<HashMap<Uuid, Residency>, ReservationsError> {
    let mut ids: Vec<Uuid> = input.rooms.iter().filter_map(|room| room.primary_guest_id).collect();
    ids.push(input.booker_guest_id);
    ids.sort_unstable();
    ids.dedup();
    let rows: Vec<(Uuid, String)> =
        sqlx::query_as("select id, residency from guest where id = any($1)").bind(&ids).fetch_all(&mut **tx).await?;
    if rows.len() != ids.len() {
        return Err(invalid("no such guest".into()));
    }
    rows.into_iter()
        .map(|(id, residency)| {
            let parsed = Residency::parse(&residency).ok_or_else(|| sqlx::Error::ColumnDecode {
                index: "residency".into(),
                source: format!("unknown {residency:?}").into(),
            })?;
            Ok((id, parsed))
        })
        .collect()
}

/// The code of every requested room type, by id. `NotFound` if one is not in the property.
async fn room_type_codes(
    tx: &mut Tx,
    property: Uuid,
    rooms: &[NewReservationRoom],
) -> Result<HashMap<Uuid, String>, ReservationsError> {
    let ids: BTreeSet<Uuid> = rooms.iter().map(|room| room.room_type_id).collect();
    let ids: Vec<Uuid> = ids.into_iter().collect();
    let rows: Vec<(Uuid, String)> =
        sqlx::query_as("select id, code from room_type where property_id = $1 and id = any($2)")
            .bind(property)
            .bind(&ids)
            .fetch_all(&mut **tx)
            .await?;
    if rows.len() != ids.len() {
        return Err(ReservationsError::NotFound("room type"));
    }
    Ok(rows.into_iter().collect())
}

/// Refuses the booking if some night has no free room of a requested type left for it, counting the rooms
/// earlier in the request. Reads counters the caller has locked.
async fn check_free(
    tx: &mut Tx,
    property: Uuid,
    room_types: &[Uuid],
    from: Date,
    to: Date,
    rooms: &[NewReservationRoom],
    codes: &HashMap<Uuid, String>,
) -> Result<(), ReservationsError> {
    let rows: Vec<(Uuid, Date, i32)> = sqlx::query_as(
        "select room_type_id, date, physical - sold - out_of_order from inventory_day
         where property_id = $1 and room_type_id = any($2) and date >= $3 and date < $4",
    )
    .bind(property)
    .bind(room_types)
    .bind(from)
    .bind(to)
    .fetch_all(&mut **tx)
    .await?;
    let mut free: HashMap<(Uuid, Date), i32> =
        rows.into_iter().map(|(room_type, date, available)| ((room_type, date), available)).collect();
    for room in rooms {
        let mut night = room.check_in;
        while night < room.check_out {
            let left = free.entry((room.room_type_id, night)).or_insert(0);
            if *left < 1 {
                return Err(ReservationsError::Conflict(format!(
                    "no {} rooms left on {night}",
                    codes[&room.room_type_id]
                )));
            }
            *left -= 1;
            night = night.next_day().expect("stays end inside the counter window");
        }
    }
    Ok(())
}

/// Each requested plan's cancellation policy as the terms a booking keeps (`{"rules": [...], "no_show":
/// {...}}`), by plan id. Plans without a policy are left out.
async fn cancellation_terms(
    tx: &mut Tx,
    property: Uuid,
    rooms: &[NewReservationRoom],
) -> Result<HashMap<Uuid, serde_json::Value>, sqlx::Error> {
    let plans: Vec<Uuid> = rooms.iter().map(|room| room.rate_plan_id).collect();
    let rows: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(
        "select rp.id, jsonb_build_object('rules', cp.rules, 'no_show', cp.no_show)
         from rate_plan rp join cancellation_policy cp on cp.id = rp.cancellation_policy_id
         where rp.property_id = $1 and rp.id = any($2)",
    )
    .bind(property)
    .bind(&plans)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().collect())
}

/// The rooms' totals per currency, in the order the rooms first use each currency.
fn totals(rooms: &[CreatedRoom]) -> Vec<Total> {
    let mut totals: Vec<Total> = Vec::new();
    for room in rooms {
        match totals.iter_mut().find(|total| total.currency == room.currency) {
            Some(total) => total.amount += room.total,
            None => totals.push(Total { currency: room.currency.clone(), amount: room.total }),
        }
    }
    totals
}
