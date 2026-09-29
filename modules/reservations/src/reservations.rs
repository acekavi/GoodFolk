//! Reservations: booking one or more rooms under a confirmation number, with every night's price fixed at
//! booking and the inventory counters kept exact under concurrent bookings.

use crate::accounts::check_account;
use crate::guests::notes;
use crate::{ReservationsError, SELLABLE, audit, check_window, notify, reservation_key, reservations_key};
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
    /// The company or travel agent this reservation is billed to, if any. Must be an active account of this
    /// tenant.
    pub account_id: Option<Uuid>,
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
    /// The room's version, for `If-Match` on its commands (cancel, assign, unassign).
    pub version: i32,
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
/// Every stay must be inside the counter window and arrive on or after the business date, and every guest,
/// room type and rate plan named must exist (`Invalid`, like any other malformed request); a named
/// `account_id` must be an active account of this tenant. The counters of
/// every requested room type are locked over all the stays at
/// once ([`rooms::lock_days`]); a night with no room left to sell of the type (`physical - sold -
/// out_of_order + overbooking <= 0`), counting the rooms this request already takes, is a `Conflict`. Each
/// room is priced by [`rates::load_quote`] for its
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
    if let Some(account) = input.account_id {
        check_account(tx, account).await?;
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
        let quote = match rates::load_quote(tx, property, &request).await {
            Err(rates::RatesError::NotFound("rate plan")) => return Err(invalid("no such rate plan".into())),
            other => other?,
        };
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
                                  created_by, account_id)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
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
    .bind(input.account_id)
    .fetch_one(&mut **tx)
    .await?;

    let mut created = Vec::with_capacity(input.rooms.len());
    for (room, quote) in input.rooms.iter().zip(quotes) {
        let room_id = Uuid::now_v7();
        let room_version: i32 = sqlx::query_scalar(
            "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, stay, adults,
                                           children, rate_plan_id, meal_plan, status, primary_guest_id, currency,
                                           cancellation_terms)
             values ($1, $2, $3, $4, $5, daterange($6, $7), $8, $9, $10, $11, 'confirmed', $12, $13, $14)
             returning version",
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
        .fetch_one(&mut **tx)
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
            version: room_version,
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

    Ok(CreatedReservation {
        id,
        confirmation_no,
        version,
        totals: totals(created.iter().map(|room| (room.currency.as_str(), room.total))),
        rooms: created,
    })
}

/// A change to a reservation's account or notes. `None` leaves a field unchanged; `account_id` is nullable, so
/// `Some(None)` clears it (bills no one) and `Some(Some(..))` sets or replaces it, as [`crate::GuestChanges`].
#[derive(Debug, Clone, Default)]
pub struct ReservationChanges {
    pub account_id: Option<Option<Uuid>>,
    pub notes: Option<String>,
}

/// A reservation after [`update_reservation`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct UpdatedReservation {
    pub id: Uuid,
    pub version: i32,
    pub account_id: Option<Uuid>,
    pub notes: String,
}

/// Sets or clears the reservation `id` of the property's billed-to account and notes, at `expected_version`.
///
/// Locks only the `reservation` row. Every reservation-room command (`cancel_room`, `assign_room`,
/// `unassign_room`) takes `reservation` LAST, after `reservation_room` (and, for `assign_room`, `room`) --
/// see "Room assignment lock order" in `docs/design/api-conventions.md`. This command never locks
/// `reservation_room` or `room` at all, so taking `reservation` first (and only) here cannot form a cycle with
/// those commands: nothing that holds `reservation_room` waits on this command, and this command waits on
/// nothing after it takes `reservation`.
pub async fn update_reservation(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    changes: ReservationChanges,
) -> Result<UpdatedReservation, ReservationsError> {
    let row: Option<(i32, Option<Uuid>, String)> = sqlx::query_as(
        "select version, account_id, notes from reservation where id = $1 and property_id = $2 for update",
    )
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    let (version, current_account, current_notes) = row.ok_or(ReservationsError::NotFound("reservation"))?;
    if version != expected_version {
        return Err(ReservationsError::VersionMismatch("reservation"));
    }
    if let Some(Some(account)) = changes.account_id {
        check_account(tx, account).await?;
    }
    let fields: Vec<&str> = [("account_id", changes.account_id.is_some()), ("notes", changes.notes.is_some())]
        .into_iter()
        .filter_map(|(field, changed)| changed.then_some(field))
        .collect();
    let account_id = changes.account_id.unwrap_or(current_account);
    let notes_value = changes.notes.map(notes).transpose()?.unwrap_or(current_notes);
    let updated_version: i32 = sqlx::query_scalar(
        "update reservation set account_id = $2, notes = $3, version = version + 1 where id = $1 returning version",
    )
    .bind(id)
    .bind(account_id)
    .bind(&notes_value)
    .fetch_one(&mut **tx)
    .await?;
    let mut data = serde_json::json!({ "fields": fields });
    if changes.account_id.is_some() {
        data["account_id"] = serde_json::json!(account_id);
    }
    audit(tx, tenant, actor, "reservation.updated", "reservation", id, data).await?;
    notify(tx, tenant, property, vec![reservations_key(property), reservation_key(id)]).await?;
    Ok(UpdatedReservation { id, version: updated_version, account_id, notes: notes_value })
}

fn invalid(message: String) -> ReservationsError {
    ReservationsError::Invalid(message)
}

/// The residency of the booker and of every primary guest, by guest id. `Invalid` if one is not a guest of
/// this tenant. Reads the rows `for share`, so a residency this booking is about to price by cannot change
/// under it before the transaction commits.
async fn residencies(tx: &mut Tx, input: &NewReservation) -> Result<HashMap<Uuid, Residency>, ReservationsError> {
    let mut ids: Vec<Uuid> = input.rooms.iter().filter_map(|room| room.primary_guest_id).collect();
    ids.push(input.booker_guest_id);
    ids.sort_unstable();
    ids.dedup();
    let rows: Vec<(Uuid, String)> = sqlx::query_as("select id, residency from guest where id = any($1) for share")
        .bind(&ids)
        .fetch_all(&mut **tx)
        .await?;
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

/// The code of every requested room type, by id. `Invalid` ("no such room type") if one is not in the
/// property, like an unknown guest id; `Invalid` if one is no longer sold (retired room types are never
/// bookable, even though a quote on their own nights could still price them).
async fn room_type_codes(
    tx: &mut Tx,
    property: Uuid,
    rooms: &[NewReservationRoom],
) -> Result<HashMap<Uuid, String>, ReservationsError> {
    let ids: BTreeSet<Uuid> = rooms.iter().map(|room| room.room_type_id).collect();
    let ids: Vec<Uuid> = ids.into_iter().collect();
    let rows: Vec<(Uuid, String, bool)> =
        sqlx::query_as("select id, code, active from room_type where property_id = $1 and id = any($2)")
            .bind(property)
            .bind(&ids)
            .fetch_all(&mut **tx)
            .await?;
    if rows.len() != ids.len() {
        return Err(invalid("no such room type".into()));
    }
    if let Some((_, code, _)) = rows.iter().find(|(_, _, active)| !active) {
        return Err(invalid(format!("{code} is no longer sold")));
    }
    Ok(rows.into_iter().map(|(id, code, _)| (id, code)).collect())
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
    let rows: Vec<(Uuid, Date, i32)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select i.room_type_id, i.date, {SELLABLE} from inventory_day i join room_type rt on rt.id = i.room_type_id
         where i.property_id = $1 and i.room_type_id = any($2) and i.date >= $3 and i.date < $4"
    )))
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

/// Sums `(currency, amount)` pairs per currency, in the order each currency first appears.
pub(crate) fn totals<'a>(amounts: impl IntoIterator<Item = (&'a str, i64)>) -> Vec<Total> {
    let mut totals: Vec<Total> = Vec::new();
    for (currency, amount) in amounts {
        match totals.iter_mut().find(|total| total.currency == currency) {
            Some(total) => total.amount += amount,
            None => totals.push(Total { currency: currency.to_owned(), amount }),
        }
    }
    totals
}
