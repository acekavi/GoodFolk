//! What a site booking's confirmation email says: the stay, its rooms and their totals with the bill's taxes, the
//! cancellation terms the rooms were booked under, any advisory deposit, and how to reach the hotel.

use crate::IbeError;
use crate::booking::token_hash;
use crate::search::{Pricing, terms_text};
use db::Tx;
use rates::{MealPlan, QuoteNight, Residency};
use reservations::{CancellationTerms, DepositDueLine};
use sqlx::types::Json;
use std::collections::BTreeMap;
use time::Date;
use uuid::Uuid;

/// A site booking as its confirmation email shows it. Amounts are in minor units of their currency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmation {
    pub property_name: String,
    /// The site's code, which its pages are addressed by.
    pub code: String,
    pub contact_email: Option<String>,
    pub contact_phone: Option<String>,
    pub confirmation_no: String,
    pub booker_name: String,
    pub booker_email: String,
    pub check_in: Date,
    pub check_out: Date,
    pub rooms: Vec<ConfirmedRoom>,
    /// With the taxes on the bill, per currency.
    pub totals: BTreeMap<String, i64>,
    /// Each distinct wording of the rooms' booked cancellation terms, in room order.
    pub cancellation: Vec<String>,
    /// The advisory deposit per currency, if any room's plan asks for one.
    pub deposits: Vec<DepositDueLine>,
}

/// One booked room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedRoom {
    pub room_type: String,
    pub plan: String,
    pub meal_plan: MealPlan,
    pub adults: i32,
    pub children: i32,
    pub currency: String,
    /// The stay with the taxes on the bill.
    pub total: i64,
}

#[derive(sqlx::FromRow)]
struct BookingRow {
    property_id: Uuid,
    property_name: String,
    vat_registered: bool,
    code: Option<String>,
    contact_email: Option<String>,
    contact_phone: Option<String>,
    confirmation_no: String,
    first_name: String,
    last_name: String,
    email: Option<String>,
    sha256: Vec<u8>,
}

#[derive(sqlx::FromRow)]
struct RoomRow {
    id: Uuid,
    room_type: String,
    plan: String,
    residency: Option<String>,
    prices_include_tax: bool,
    meal_plan: String,
    adults: i32,
    children: i32,
    currency: String,
    check_in: Date,
    check_out: Date,
    cancellation_terms: Option<Json<CancellationTerms>>,
}

/// The confirmation of site booking `reservation`, or `None` when the tenant has no such reservation from the site,
/// its manage token is not `token` (or it has none), or the booker has no email. Room totals are its booked nights
/// taxed with the property's rules, as the search priced them.
pub async fn confirmation(tx: &mut Tx, reservation: Uuid, token: &str) -> Result<Option<Confirmation>, IbeError> {
    let row: Option<BookingRow> = sqlx::query_as(
        "select r.property_id, p.name as property_name, p.vat_registered, s.public_code as code, s.contact_email,
                s.contact_phone, r.confirmation_no, g.first_name, g.last_name, g.email, t.sha256
         from reservation r
         join property p on p.id = r.property_id
         join guest g on g.id = r.booker_guest_id
         join reservation_access_token t on t.reservation_id = r.id
         left join ibe_site s on s.property_id = r.property_id
         where r.id = $1 and r.source = 'ibe'",
    )
    .bind(reservation)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row.filter(|row| row.sha256 == token_hash(token)) else { return Ok(None) };
    let (Some(code), Some(booker_email)) = (row.code, row.email) else { return Ok(None) };
    let rooms = booked_rooms(tx, row.property_id, row.vat_registered, reservation).await?;
    let mut confirmed = Vec::with_capacity(rooms.len());
    let mut totals: BTreeMap<String, i64> = BTreeMap::new();
    let mut cancellation: Vec<String> = Vec::new();
    for (room, meal_plan, total) in &rooms {
        *totals.entry(room.currency.clone()).or_default() += total;
        let terms = room.cancellation_terms.as_ref().map_or_else(
            || "Free cancellation.".to_owned(),
            |Json(terms)| terms_text(&terms.rules, terms.no_show, &room.currency),
        );
        if !cancellation.contains(&terms) {
            cancellation.push(terms);
        }
        confirmed.push(ConfirmedRoom {
            room_type: room.room_type.clone(),
            plan: room.plan.clone(),
            meal_plan: *meal_plan,
            adults: room.adults,
            children: room.children,
            currency: room.currency.clone(),
            total: *total,
        });
    }
    let deposits = reservations::deposit_due(tx, reservation).await.map_err(|err| match err {
        reservations::ReservationsError::Database(err) => IbeError::Database(err),
        other => IbeError::Invalid(other.to_string()),
    })?;
    let (check_in, check_out) = (
        rooms.iter().map(|(room, ..)| room.check_in).min().ok_or(IbeError::NotFound("reservation room"))?,
        rooms.iter().map(|(room, ..)| room.check_out).max().ok_or(IbeError::NotFound("reservation room"))?,
    );
    Ok(Some(Confirmation {
        property_name: row.property_name,
        code,
        contact_email: row.contact_email,
        contact_phone: row.contact_phone,
        confirmation_no: row.confirmation_no,
        booker_name: format!("{} {}", row.first_name, row.last_name).trim().to_owned(),
        booker_email,
        check_in,
        check_out,
        rooms: confirmed,
        totals,
        cancellation,
        deposits,
    }))
}

/// The rooms of `reservation` in `property` (`vat_registered` as the property is), each with its meal plan and its
/// booked nights' total with the taxes on the bill, priced as the search prices a stay.
async fn booked_rooms(
    tx: &mut Tx,
    property: Uuid,
    vat_registered: bool,
    reservation: Uuid,
) -> Result<Vec<(RoomRow, MealPlan, i64)>, IbeError> {
    let rooms: Vec<RoomRow> = sqlx::query_as(
        "select rr.id, rt.name as room_type, rp.name as plan, rp.residency, rp.prices_include_tax, rr.meal_plan,
                rr.adults, rr.children, rr.currency::text as currency, lower(rr.stay) as check_in,
                upper(rr.stay) as check_out, rr.cancellation_terms
         from reservation_room rr
         join room_type rt on rt.id = rr.room_type_id
         join rate_plan rp on rp.id = rr.rate_plan_id
         where rr.reservation_id = $1
         order by rr.id",
    )
    .bind(reservation)
    .fetch_all(&mut **tx)
    .await?;
    let ids: Vec<Uuid> = rooms.iter().map(|room| room.id).collect();
    let nights: Vec<(Uuid, Date, i64, i64)> = sqlx::query_as(
        "select reservation_room_id, date, room_amount, meal_amount from reservation_night
         where reservation_room_id = any($1) order by date",
    )
    .bind(&ids)
    .fetch_all(&mut **tx)
    .await?;
    let rules = folio::load_rules(tx, property).await?;
    let mut priced = Vec::with_capacity(rooms.len());
    for room in rooms {
        let decode = |column: &str, value: &str| sqlx::Error::ColumnDecode {
            index: column.into(),
            source: format!("unknown value {value:?}").into(),
        };
        let meal_plan = MealPlan::parse(&room.meal_plan).ok_or_else(|| decode("meal_plan", &room.meal_plan))?;
        let residency = room
            .residency
            .as_deref()
            .map(|value| Residency::parse(value).ok_or_else(|| decode("residency", value)))
            .transpose()?;
        let pricing = Pricing {
            rules: &rules,
            vat_registered,
            resident: residency.map(|residency| residency == Residency::Resident),
            currency: &room.currency,
            inclusive: room.prices_include_tax,
        };
        let booked: Vec<QuoteNight> = nights
            .iter()
            .filter(|(id, ..)| *id == room.id)
            .map(|&(_, date, room, meal)| QuoteNight { date, room, meal })
            .collect();
        let total = pricing.meal_offer(meal_plan, Vec::new(), &booked)?.total;
        priced.push((room, meal_plan, total));
    }
    Ok(priced)
}

/// What `reservation` in `property` costs per currency with the taxes on the bill, from its booked nights.
pub(crate) async fn booked_totals(
    tx: &mut Tx,
    property: Uuid,
    reservation: Uuid,
) -> Result<BTreeMap<String, i64>, IbeError> {
    let vat_registered: bool = sqlx::query_scalar("select vat_registered from property where id = $1")
        .bind(property)
        .fetch_one(&mut **tx)
        .await?;
    let mut totals: BTreeMap<String, i64> = BTreeMap::new();
    for (room, _, total) in booked_rooms(tx, property, vat_registered, reservation).await? {
        *totals.entry(room.currency).or_default() += total;
    }
    Ok(totals)
}
