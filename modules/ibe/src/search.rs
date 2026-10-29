//! The booking site's search: each active room type with rooms free on every night for the rooms asked that it can
//! hold, and every `IBE` plan open for the stay, priced per room with the taxes the folio engine would put on the bill.

use crate::IbeError;
use db::Tx;
use rates::{
    CancellationPolicy, MealPlan, Offer, OfferData, OfferRequest, PenaltyKind, Quote, RatePlan, Residency, Segment,
};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use time::{Date, Duration, Month};
use utoipa::ToSchema;
use uuid::Uuid;

/// Most nights a stay booked on the site may have.
pub const MAX_NIGHTS: i64 = 30;
/// Most rooms one booking on the site may have.
pub const MAX_ROOMS: usize = 4;
/// How far past the business date a stay may start, in months.
pub const MONTHS_AHEAD: u8 = 18;
/// How many later check-ins a search with no result tries for [`SearchResult::next_available`].
pub const NEXT_AVAILABLE_DAYS: i64 = 14;

/// A stay to search: `[check_in, check_out)` for guests of `residency`, in `rooms` rooms.
#[derive(Debug, Clone)]
pub struct SearchRequest {
    pub check_in: Date,
    pub check_out: Date,
    pub residency: Residency,
    pub rooms: Vec<RoomAsk>,
}

/// The guests of one room asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RoomAsk {
    pub adults: i32,
    pub children: i32,
}

/// What the site offers for a stay. With no room type, `next_available` is the nearest later check-in, within
/// [`NEXT_AVAILABLE_DAYS`], for the same nights and rooms that has one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SearchResult {
    pub room_types: Vec<RoomTypeResult>,
    pub next_available: Option<Date>,
}

/// A room type with rooms free for every room asked that it can hold, and the plans it is sold on for the stay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct RoomTypeResult {
    pub room_type_id: Uuid,
    /// The fewest rooms free on any night of the stay.
    pub available: i32,
    /// By plan code.
    pub plans: Vec<PlanOffer>,
}

/// A rate plan open for the stay, in its own currency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct PlanOffer {
    pub rate_plan_id: Uuid,
    pub name: String,
    pub currency: String,
    /// The cancellation policy in plain words.
    pub cancellation: String,
    /// RO, BB, HB, FB, each once per occupancy it is priced for.
    pub meal_plans: Vec<MealOffer>,
}

/// The price of one meal plan for the rooms asked with one occupancy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct MealOffer {
    #[schema(value_type = String)]
    pub meal_plan: MealPlan,
    /// The rooms asked (0-based, in the order asked) this price is for: those with the same adults and children.
    pub rooms: Vec<u8>,
    /// The stay for one of those rooms, with the taxes on the bill, in minor units.
    pub total: i64,
    pub nights: Vec<NightPrice>,
    /// The taxes and service charge the bill shows over the stay, for one room, in the order they apply.
    pub taxes: Vec<TaxLine>,
}

/// One night's price, in minor units.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct NightPrice {
    pub date: Date,
    /// The room and the meal plan as priced: before taxes, or including them on a tax-inclusive plan.
    pub room: i64,
    pub meal: i64,
    /// With the taxes on the bill.
    pub total: i64,
}

/// One tax, levy or service charge over a stay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TaxLine {
    pub name: String,
    pub amount: i64,
}

/// What a stay costs to cancel, in plain words, from `policy`'s rules (amounts in `currency`, the plan's). Without a
/// policy cancelling is free. Each rule charges from its number of days before arrival on (as cancelling applies it),
/// so the time before the furthest one is free.
pub fn cancellation_text(policy: Option<&CancellationPolicy>, currency: &str) -> String {
    let mut rules: Vec<_> = policy.map(|policy| policy.rules.clone()).unwrap_or_default();
    rules.sort_by_key(|rule| std::cmp::Reverse(rule.days_before_arrival));
    let Some((first, later)) = rules.split_first() else { return "Free cancellation.".into() };
    let when = |days: i32| match days {
        0 => "the day of arrival".to_owned(),
        1 => "1 day before arrival".to_owned(),
        days => format!("{days} days before arrival"),
    };
    let charge = |penalty: rates::Penalty| match penalty.kind {
        PenaltyKind::Nights if penalty.value == 1 => "one night is charged".to_owned(),
        PenaltyKind::Nights => format!("{} nights are charged", penalty.value),
        PenaltyKind::Percent if penalty.value >= 10_000 => "the whole stay is charged".to_owned(),
        PenaltyKind::Percent => format!("{}% of the stay is charged", decimal(penalty.value, 2)),
        PenaltyKind::Amount => format!("{currency} {} is charged", money(penalty.value, fx::exponent(currency))),
    };
    let mut text =
        format!("Free cancellation until {}; after that, {}", when(first.days_before_arrival), charge(first.penalty));
    for rule in later {
        text.push_str(&format!("; from {}, {}", when(rule.days_before_arrival), charge(rule.penalty)));
    }
    text.push('.');
    text
}

/// `value` with `places` implied decimals, without trailing zeros after the point: 1250 with 2 places is `12.5`.
fn decimal(value: i64, places: u32) -> String {
    let fixed = money(value, places);
    if fixed.contains('.') { fixed.trim_end_matches('0').trim_end_matches('.').to_owned() } else { fixed }
}

/// `value` minor units with `places` decimals, all shown: 5000 with 2 places is `50.00`.
fn money(value: i64, places: u32) -> String {
    let scale = 10_i64.pow(places);
    let (whole, fraction) = (value / scale, value % scale);
    if places == 0 {
        return whole.to_string();
    }
    let width = usize::try_from(places).expect("a currency exponent");
    format!("{whole}.{fraction:0width$}")
}

/// The date `months` months after `date`, on the same day or the month's last if it is shorter.
fn add_months(date: Date, months: u8) -> Date {
    let index = i32::from(u8::from(date.month())) - 1 + i32::from(months);
    let year = date.year() + index / 12;
    let month = Month::try_from(u8::try_from(index % 12 + 1).expect("a month")).expect("a month");
    let day = date.day().min(time::util::days_in_month(month, year));
    Date::from_calendar_date(year, month, day).expect("a valid date")
}

#[derive(sqlx::FromRow)]
struct PropertyRow {
    business_date: Date,
    vat_registered: bool,
}

/// One search's data: the IBE plans, rooms free per room type and night, and the offers' rows.
struct Stock {
    free: HashMap<(Uuid, Date), i32>,
    data: OfferData,
}

/// A meal plan's sellable quote for each occupancy asked that has one.
type MealQuotes = (MealPlan, Vec<(RoomAsk, Quote)>);

/// A room type that qualifies for a stay, with the sellable quotes per plan and meal plan for each occupancy it holds.
struct Match<'a> {
    room_type_id: Uuid,
    available: i32,
    plans: Vec<(&'a RatePlan, Vec<MealQuotes>)>,
}

/// What the property's booking site offers for `request` (see [`SearchResult`]). The stay must be 1 to [`MAX_NIGHTS`]
/// nights starting on the business date or up to [`MONTHS_AHEAD`] months after it, for 1 to [`MAX_ROOMS`] rooms of at
/// least one adult each (`Invalid`). Offered plans are active `IBE` plans for the guest's residency or anyone, with no
/// restriction broken; their nights are taxed as [`folio::charge_taxes`] taxes them when posted.
///
/// A bounded number of queries: the property, the free rooms, the plans and their rows (as
/// [`rates::read_offer_data`]), then the tax rules, the plans' tax inclusion and the cancellation policies, or, with no
/// result, one more read of free rooms and plan rows covering every later check-in tried.
pub async fn search(tx: &mut Tx, property: Uuid, request: &SearchRequest) -> Result<SearchResult, IbeError> {
    let row: PropertyRow = sqlx::query_as("select business_date, vat_registered from property where id = $1")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(IbeError::NotFound("property"))?;
    let last_check_in = add_months(row.business_date, MONTHS_AHEAD);
    check(request, row.business_date, last_check_in)?;
    let nights = request.check_out - request.check_in;

    let plans: Vec<RatePlan> = rates::list_rate_plans(tx, property)
        .await?
        .into_iter()
        .filter(|plan| {
            plan.active
                && plan.segment == Segment::Ibe
                && plan.residency.is_none_or(|residency| residency == request.residency)
        })
        .collect();
    let stock = read_stock(tx, property, plans.clone(), request.check_in, request.check_out).await?;
    let qualifying = matches(&stock, request);
    if qualifying.is_empty() {
        let from = request.check_in + Duration::days(1);
        let last = (request.check_in + Duration::days(NEXT_AVAILABLE_DAYS)).min(last_check_in);
        if from > last {
            return Ok(SearchResult { room_types: Vec::new(), next_available: None });
        }
        let later = read_stock(tx, property, plans, from, last + nights).await?;
        let next_available = std::iter::successors(Some(from), |day| Some(*day + Duration::days(1)))
            .take_while(|day| *day <= last)
            .find(|day| {
                let stay = SearchRequest { check_in: *day, check_out: *day + nights, ..request.clone() };
                !matches(&later, &stay).is_empty()
            });
        return Ok(SearchResult { room_types: Vec::new(), next_available });
    }

    let rules = folio::load_rules(tx, property).await?;
    let plan_ids: Vec<Uuid> = stock.data.plans().iter().map(|plan| plan.id).collect();
    let inclusive: HashMap<Uuid, bool> =
        sqlx::query_as::<_, (Uuid, bool)>("select id, prices_include_tax from rate_plan where id = any($1)")
            .bind(&plan_ids)
            .fetch_all(&mut **tx)
            .await?
            .into_iter()
            .collect();
    let policies: HashMap<Uuid, CancellationPolicy> =
        rates::list_cancellation_policies(tx, property).await?.into_iter().map(|policy| (policy.id, policy)).collect();

    let mut room_types = Vec::with_capacity(qualifying.len());
    for found in qualifying {
        let mut plans = Vec::with_capacity(found.plans.len());
        for (plan, meal_plans) in found.plans {
            let pricing = Pricing {
                rules: &rules,
                vat_registered: row.vat_registered,
                resident: plan.residency.map(|residency| residency == Residency::Resident),
                currency: &plan.currency,
                inclusive: inclusive.get(&plan.id).copied().unwrap_or(false),
            };
            let mut offers = Vec::new();
            for (meal_plan, quotes) in meal_plans {
                for (ask, quote) in quotes {
                    offers.push(pricing.meal_offer(meal_plan, room_indices(&request.rooms, ask), &quote)?);
                }
            }
            let policy = plan.cancellation_policy_id.and_then(|id| policies.get(&id));
            plans.push(PlanOffer {
                rate_plan_id: plan.id,
                name: plan.name.clone(),
                currency: plan.currency.clone(),
                cancellation: cancellation_text(policy, &plan.currency),
                meal_plans: offers,
            });
        }
        room_types.push(RoomTypeResult { room_type_id: found.room_type_id, available: found.available, plans });
    }
    Ok(SearchResult { room_types, next_available: None })
}

/// `Invalid` unless `request` is a stay the site sells: see [`search`].
fn check(request: &SearchRequest, business_date: Date, last_check_in: Date) -> Result<(), IbeError> {
    let nights = (request.check_out - request.check_in).whole_days();
    if !(1..=MAX_NIGHTS).contains(&nights) {
        return Err(IbeError::Invalid(format!("a stay is 1 to {MAX_NIGHTS} nights")));
    }
    if request.check_in < business_date || request.check_in > last_check_in {
        return Err(IbeError::Invalid(format!("check-in is from {business_date} to {last_check_in}")));
    }
    if !(1..=MAX_ROOMS).contains(&request.rooms.len()) {
        return Err(IbeError::Invalid(format!("a booking is 1 to {MAX_ROOMS} rooms")));
    }
    if request.rooms.iter().any(|room| room.adults < 1 || room.children < 0) {
        return Err(IbeError::Invalid("each room has at least one adult".into()));
    }
    Ok(())
}

/// The rooms free per active room type and night of `[from, to)`, and the rows to price `plans` for stays inside it,
/// for the active room types only.
async fn read_stock(
    tx: &mut Tx,
    property: Uuid,
    plans: Vec<RatePlan>,
    from: Date,
    to: Date,
) -> Result<Stock, IbeError> {
    let rows: Vec<(Uuid, Date, i32)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select i.room_type_id, i.date, {} as free
         from inventory_day i join room_type rt on rt.id = i.room_type_id
         where rt.property_id = $1 and rt.active and i.date >= $2 and i.date < $3",
        reservations::SELLABLE
    )))
    .bind(property)
    .bind(from)
    .bind(to)
    .fetch_all(&mut **tx)
    .await?;
    let mut active: Vec<Uuid> = rows.iter().map(|(room_type, _, _)| *room_type).collect();
    active.sort_unstable();
    active.dedup();
    let free = rows.into_iter().map(|(room_type, date, free)| ((room_type, date), free)).collect();
    let data = rates::read_offer_data(tx, property, plans, Some(&active), from, to).await?;
    Ok(Stock { free, data })
}

/// The room types of `stock` that qualify for `request`, in display order: those holding at least one room asked, with
/// at least as many rooms free on every night as the rooms asked they can hold, and a sellable quote on some plan.
fn matches<'a>(stock: &'a Stock, request: &SearchRequest) -> Vec<Match<'a>> {
    let mut asks: Vec<RoomAsk> = request.rooms.clone();
    asks.sort_unstable();
    asks.dedup();
    let offers: BTreeMap<RoomAsk, Vec<Offer>> = asks
        .iter()
        .map(|ask| {
            let offer_request = OfferRequest {
                check_in: request.check_in,
                check_out: request.check_out,
                adults: ask.adults,
                children: ask.children,
                residency: request.residency,
                room_type_ids: None,
            };
            (*ask, rates::price_offers(&stock.data, &offer_request))
        })
        .collect();
    let nights: Vec<Date> = std::iter::successors(Some(request.check_in), |night| Some(*night + Duration::days(1)))
        .take_while(|night| *night < request.check_out)
        .collect();

    let mut found = Vec::new();
    for room_type in stock.data.room_types() {
        let holds = |ask: &RoomAsk| {
            ask.adults <= room_type.max_adults
                && ask.children <= room_type.max_children
                && ask.adults + ask.children <= room_type.max_occupancy
        };
        let wanted = request.rooms.iter().filter(|ask| holds(ask)).count();
        let available =
            nights.iter().map(|night| stock.free.get(&(room_type.id, *night)).copied().unwrap_or(0)).min().unwrap_or(0);
        if wanted == 0 || usize::try_from(available).unwrap_or(0) < wanted {
            continue;
        }
        let mut plans = Vec::new();
        for plan in stock.data.plans().iter().filter(|plan| plan.room_type_ids.contains(&room_type.id)) {
            let mut meal_plans: Vec<MealQuotes> = Vec::new();
            for (ask, offers) in offers.iter().filter(|(ask, _)| holds(ask)) {
                let sellable = offers.iter().filter(|offer| {
                    offer.room_type_id == room_type.id && offer.rate_plan_id == plan.id && offer.quote.restrictions_ok
                });
                for offer in sellable {
                    match meal_plans.iter_mut().find(|(meal_plan, _)| *meal_plan == offer.meal_plan) {
                        Some((_, quotes)) => quotes.push((*ask, offer.quote.clone())),
                        None => meal_plans.push((offer.meal_plan, vec![(*ask, offer.quote.clone())])),
                    }
                }
            }
            if !meal_plans.is_empty() {
                meal_plans.sort_by_key(|(meal_plan, _)| MEAL_PLAN_ORDER.iter().position(|m| m == meal_plan));
                plans.push((plan, meal_plans));
            }
        }
        if !plans.is_empty() {
            found.push(Match { room_type_id: room_type.id, available, plans });
        }
    }
    found
}

/// Meal plans in the order offers list them.
const MEAL_PLAN_ORDER: [MealPlan; 4] = [MealPlan::Ro, MealPlan::Bb, MealPlan::Hb, MealPlan::Fb];

/// The indices of the rooms of `rooms` asked for exactly `ask`.
fn room_indices(rooms: &[RoomAsk], ask: RoomAsk) -> Vec<u8> {
    rooms
        .iter()
        .enumerate()
        .filter(|(_, room)| **room == ask)
        .map(|(index, _)| u8::try_from(index).expect("at most MAX_ROOMS rooms"))
        .collect()
}

/// How one plan's nights are taxed: as posting taxes them, with the property's rules for each night's date, the room
/// and meal-plan categories, the plan's residency and currency, and its tax inclusion.
struct Pricing<'a> {
    rules: &'a [tax::Rule],
    vat_registered: bool,
    resident: Option<bool>,
    currency: &'a str,
    inclusive: bool,
}

impl Pricing<'_> {
    /// `quote`'s nights with their taxes: each night's room charge, and its meal-plan charge when above zero, taxed
    /// as a line of its own, as the night audit posts them.
    fn meal_offer(&self, meal_plan: MealPlan, rooms: Vec<u8>, quote: &Quote) -> Result<MealOffer, IbeError> {
        let mut taxes: Vec<TaxLine> = Vec::new();
        let mut nights = Vec::with_capacity(quote.nights.len());
        for night in &quote.nights {
            let mut total = 0;
            let lines = [("room", night.room), ("meal_plan", night.meal)];
            for (category, amount) in lines.into_iter().filter(|(category, amount)| *category == "room" || *amount > 0)
            {
                let context = tax::Context {
                    date: night.date,
                    category: category.to_owned(),
                    vat_registered: self.vat_registered,
                    resident: self.resident,
                    currency: self.currency.to_owned(),
                };
                let breakdown = folio::charge_taxes(amount, self.inclusive, self.rules, &context)?;
                total += breakdown.total;
                for component in breakdown.components.iter().filter(|c| c.presentation == tax::Presentation::OnBill) {
                    match taxes.iter_mut().find(|line| line.name == component.name) {
                        Some(line) => line.amount += component.amount,
                        None => taxes.push(TaxLine { name: component.name.clone(), amount: component.amount }),
                    }
                }
            }
            nights.push(NightPrice { date: night.date, room: night.room, meal: night.meal, total });
        }
        Ok(MealOffer { meal_plan, rooms, total: nights.iter().map(|night| night.total).sum(), nights, taxes })
    }
}
