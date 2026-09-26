//! Price quotes: what a stay costs on a plan, night by night, and every reason it cannot be sold. [`quote`] is
//! a pure function of rows [`load_quote`] reads, so reservations (Phase 3) and the booking engine (Phase 9)
//! reuse it with the same rules.

use crate::plans::{MealPlan, RatePlan, Residency, list_rate_plans};
use crate::{MealSupplement, Price, RatesError, Restriction};
use db::Tx;
use serde::Serialize;
use time::{Date, Duration};
use uuid::Uuid;

/// Maximum number of nights in a single stay.
pub const MAX_STAY_NIGHTS: i64 = 730;

/// A stay to price: `[check_in, check_out)`, for guests of `residency`.
#[derive(Debug, Clone)]
pub struct QuoteRequest {
    pub room_type_id: Uuid,
    pub rate_plan_id: Uuid,
    pub meal_plan: MealPlan,
    pub check_in: Date,
    pub check_out: Date,
    pub adults: i32,
    pub children: i32,
    pub residency: Residency,
}

/// The room type's capacity, as the quote needs it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct QuoteRoomType {
    pub id: Uuid,
    pub code: String,
    pub max_adults: i32,
    pub max_children: i32,
    pub max_occupancy: i32,
}

/// The rows a quote reads: the plan's prices for the room type on the stay's nights, its restrictions on those
/// nights and the departure date, and the supplements for the chosen meal plan in the plan's currency.
#[derive(Debug, Clone)]
pub struct QuoteData {
    pub plan: RatePlan,
    pub room_type: QuoteRoomType,
    pub prices: Vec<Price>,
    pub restrictions: Vec<Restriction>,
    pub supplements: Vec<MealSupplement>,
}

/// One night: the room and the meal supplement, in the plan's currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct QuoteNight {
    pub date: Date,
    pub room: i64,
    pub meal: i64,
}

text_enum!(
    /// Why a stay cannot be sold as quoted.
    ViolationKind {
        InvalidStay = "invalid_stay",
        Inactive = "inactive",
        Residency = "residency",
        RoomTypeNotSold = "room_type_not_sold",
        Occupancy = "occupancy",
        MealPlanNotAllowed = "meal_plan_not_allowed",
        NoPrice = "no_price",
        NoMealSupplement = "no_meal_supplement",
        Closed = "closed",
        MinStay = "min_stay",
        MaxStay = "max_stay",
        ClosedToArrival = "closed_to_arrival",
        ClosedToDeparture = "closed_to_departure",
    }
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Violation {
    pub kind: ViolationKind,
    /// The night (or arrival or departure date) it concerns, if it concerns one.
    pub date: Option<Date>,
    pub message: String,
}

/// What a stay costs. `restrictions_ok` is true when `violations` is empty: nothing stops the sale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Quote {
    pub nights: Vec<QuoteNight>,
    pub total: i64,
    pub currency: String,
    pub restrictions_ok: bool,
    pub violations: Vec<Violation>,
}

/// The nights of `[check_in, check_out)`.
fn nights(check_in: Date, check_out: Date) -> Vec<Date> {
    std::iter::successors(Some(check_in), |night| Some(*night + Duration::days(1)))
        .take_while(|night| *night < check_out)
        .collect()
}

/// The room price for `adults` on `date`: the price for that occupancy, or else the nearest lower one plus
/// the plan's extra-adult amount for each adult above it.
fn room_price(plan: &RatePlan, prices: &[Price], date: Date, adults: i32) -> Option<i64> {
    prices
        .iter()
        .filter(|price| price.date == date && price.occupancy <= adults)
        .max_by_key(|price| price.occupancy)
        .map(|price| price.amount + i64::from(adults - price.occupancy) * plan.extra_adult_amount)
}

/// Prices a stay and lists every reason it cannot be sold. Pure: everything it needs is in `data`.
pub fn quote(request: &QuoteRequest, data: &QuoteData) -> Quote {
    let (plan, room_type) = (&data.plan, &data.room_type);

    // Early exit for invalid stays: empty or too long
    if request.check_out <= request.check_in {
        return Quote {
            nights: vec![],
            total: 0,
            currency: plan.currency.clone(),
            restrictions_ok: false,
            violations: vec![Violation {
                kind: ViolationKind::InvalidStay,
                date: None,
                message: "check-out must be after check-in".into(),
            }],
        };
    }

    // Count nights safely using next_day() to avoid Date::MAX panic
    let mut night_count = 0i64;
    let mut current = request.check_in;
    while current < request.check_out {
        night_count += 1;
        if let Some(next) = current.next_day() {
            current = next;
        } else {
            break;
        }
    }

    if night_count > MAX_STAY_NIGHTS {
        return Quote {
            nights: vec![],
            total: 0,
            currency: plan.currency.clone(),
            restrictions_ok: false,
            violations: vec![Violation {
                kind: ViolationKind::InvalidStay,
                date: None,
                message: format!("stay of {night_count} nights exceeds maximum of {MAX_STAY_NIGHTS}"),
            }],
        };
    }

    let mut violations = Vec::new();
    let mut violation = |kind: ViolationKind, date: Option<Date>, message: String| {
        violations.push(Violation { kind, date, message });
    };
    if !plan.active {
        violation(ViolationKind::Inactive, None, format!("{} is not sold any more", plan.code));
    }
    if plan.residency.is_some_and(|residency| residency != request.residency) {
        let who = if request.residency == Residency::Resident { "non-residents" } else { "residents" };
        violation(ViolationKind::Residency, None, format!("{} is sold to {who} only", plan.code));
    }
    if !plan.room_type_ids.contains(&room_type.id) {
        violation(ViolationKind::RoomTypeNotSold, None, format!("{} does not sell {}", plan.code, room_type.code));
    }
    let (adults, children) = (request.adults, request.children);
    let occupancy_valid = adults >= 1
        && adults <= room_type.max_adults
        && children >= 0
        && children <= room_type.max_children
        && adults + children <= room_type.max_occupancy;
    if !occupancy_valid {
        violation(
            ViolationKind::Occupancy,
            None,
            format!(
                "{} takes 1 to {} adults and up to {} children, {} guests in all",
                room_type.code, room_type.max_adults, room_type.max_children, room_type.max_occupancy
            ),
        );
    }
    if !plan.allowed_meal_plans.contains(&request.meal_plan) {
        let meal_plan = request.meal_plan.as_str();
        violation(ViolationKind::MealPlanNotAllowed, None, format!("{} is not sold with {meal_plan}", plan.code));
    }

    let stay = nights(request.check_in, request.check_out);
    let length = i32::try_from(stay.len()).unwrap_or(i32::MAX);
    let restriction = |date: Date| data.restrictions.iter().find(|r| r.date == date);
    let mut quoted = Vec::with_capacity(stay.len());

    // Only price nights if occupancy is valid
    if occupancy_valid {
        for &date in &stay {
            let room = room_price(plan, &data.prices, date, adults).unwrap_or_else(|| {
                violation(ViolationKind::NoPrice, Some(date), format!("no price for {adults} adults on {date}"));
                0
            });
            let meal = if request.meal_plan == MealPlan::Ro {
                0
            } else {
                let covers = |s: &&MealSupplement| s.from <= date && s.to.is_none_or(|to| date < to);
                match data.supplements.iter().find(covers) {
                    Some(supplement) => {
                        i64::from(adults) * supplement.adult_amount + i64::from(children) * supplement.child_amount
                    }
                    None => {
                        let meal_plan = request.meal_plan.as_str();
                        violation(
                            ViolationKind::NoMealSupplement,
                            Some(date),
                            format!("no {meal_plan} supplement in {} on {date}", plan.currency),
                        );
                        0
                    }
                }
            };

            if let Some(r) = restriction(date) {
                if r.closed {
                    violation(ViolationKind::Closed, Some(date), format!("{} is closed on {date}", plan.code));
                }
                if let Some(min) = r.min_stay.filter(|min| length < *min) {
                    violation(
                        ViolationKind::MinStay,
                        Some(date),
                        format!("stays over {date} are at least {min} nights"),
                    );
                }
                if let Some(max) = r.max_stay.filter(|max| length > *max) {
                    violation(
                        ViolationKind::MaxStay,
                        Some(date),
                        format!("stays over {date} are at most {max} nights"),
                    );
                }
            }
            quoted.push(QuoteNight { date, room, meal });
        }
    } else {
        // When occupancy is invalid, still check restrictions but don't price nights
        for &date in &stay {
            if let Some(r) = restriction(date) {
                if r.closed {
                    violation(ViolationKind::Closed, Some(date), format!("{} is closed on {date}", plan.code));
                }
                if let Some(min) = r.min_stay.filter(|min| length < *min) {
                    violation(
                        ViolationKind::MinStay,
                        Some(date),
                        format!("stays over {date} are at least {min} nights"),
                    );
                }
                if let Some(max) = r.max_stay.filter(|max| length > *max) {
                    violation(
                        ViolationKind::MaxStay,
                        Some(date),
                        format!("stays over {date} are at most {max} nights"),
                    );
                }
            }
        }
    }
    if restriction(request.check_in).is_some_and(|r| r.closed_to_arrival) {
        let date = request.check_in;
        violation(ViolationKind::ClosedToArrival, Some(date), format!("arrivals are closed on {date}"));
    }
    if restriction(request.check_out).is_some_and(|r| r.closed_to_departure) {
        let date = request.check_out;
        violation(ViolationKind::ClosedToDeparture, Some(date), format!("departures are closed on {date}"));
    }

    Quote {
        total: quoted.iter().map(|night| night.room + night.meal).sum(),
        nights: quoted,
        currency: plan.currency.clone(),
        restrictions_ok: violations.is_empty(),
        violations,
    }
}

/// Reads what a quote needs and prices the stay.
pub async fn load_quote(tx: &mut Tx, property: Uuid, request: &QuoteRequest) -> Result<Quote, RatesError> {
    if request.check_out <= request.check_in {
        return Err(RatesError::Invalid("check-out is after check-in".into()));
    }

    // Count nights safely before any queries
    let mut night_count = 0i64;
    let mut current = request.check_in;
    while current < request.check_out {
        night_count += 1;
        if let Some(next) = current.next_day() {
            current = next;
        } else {
            break;
        }
    }

    if night_count > MAX_STAY_NIGHTS {
        return Err(RatesError::Invalid(format!("stay of {night_count} nights exceeds maximum of {MAX_STAY_NIGHTS}")));
    }
    let plan = list_rate_plans(tx, property)
        .await?
        .into_iter()
        .find(|plan| plan.id == request.rate_plan_id)
        .ok_or(RatesError::NotFound("rate plan"))?;
    let room_type: QuoteRoomType = sqlx::query_as(
        "select id, code, max_adults, max_children, max_occupancy from room_type where id = $1 and property_id = $2",
    )
    .bind(request.room_type_id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(RatesError::NotFound("room type"))?;
    let prices: Vec<Price> = sqlx::query_as(
        "select room_type_id, date, occupancy, amount from rate_day
         where rate_plan_id = $1 and room_type_id = $2 and date >= $3 and date < $4",
    )
    .bind(plan.id)
    .bind(room_type.id)
    .bind(request.check_in)
    .bind(request.check_out)
    .fetch_all(&mut **tx)
    .await?;
    let restrictions: Vec<Restriction> = sqlx::query_as(
        "select room_type_id, date, closed, min_stay, max_stay, closed_to_arrival, closed_to_departure
         from rate_restriction where rate_plan_id = $1 and room_type_id = $2 and date >= $3 and date <= $4",
    )
    .bind(plan.id)
    .bind(room_type.id)
    .bind(request.check_in)
    .bind(request.check_out)
    .fetch_all(&mut **tx)
    .await?;
    let supplements: Vec<MealSupplement> = sqlx::query_as(
        "select id, property_id, meal_plan, currency::text as currency, adult_amount, child_amount,
                lower(valid) as \"from\", upper(valid) as \"to\", version
         from meal_supplement
         where property_id = $1 and meal_plan = $2 and currency = $3 and valid && daterange($4, $5)",
    )
    .bind(property)
    .bind(request.meal_plan.as_str())
    .bind(&plan.currency)
    .bind(request.check_in)
    .bind(request.check_out)
    .fetch_all(&mut **tx)
    .await?;
    Ok(quote(request, &QuoteData { plan, room_type, prices, restrictions, supplements }))
}

#[cfg(test)]
mod tests {
    use super::{QuoteData, QuoteNight, QuoteRequest, QuoteRoomType, ViolationKind, quote};
    use crate::plans::{MealPlan, PlanKind, RatePlan, Residency, Segment};
    use crate::{MealSupplement, Price, Restriction};
    use time::Date;
    use time::macros::date;
    use uuid::Uuid;

    const DELUXE: Uuid = Uuid::from_u128(1);
    const ARRIVAL: Date = date!(2026 - 10 - 01);

    fn day(offset: i64) -> Date {
        ARRIVAL + time::Duration::days(offset)
    }

    fn plan() -> RatePlan {
        RatePlan {
            id: Uuid::from_u128(2),
            property_id: Uuid::from_u128(3),
            code: "BAR".into(),
            name: "Best available".into(),
            kind: PlanKind::Standard,
            segment: Segment::Ibe,
            residency: None,
            currency: "USD".into(),
            parent_id: None,
            depth: 0,
            derive_mode: None,
            derive_value: None,
            rounding_step: 1,
            extra_adult_amount: 2_500,
            inherit_restrictions: false,
            allowed_meal_plans: vec![MealPlan::Ro, MealPlan::Bb],
            cancellation_policy_id: None,
            room_type_ids: vec![DELUXE],
            active: true,
            version: 1,
        }
    }

    fn price(offset: i64, occupancy: i32, amount: i64) -> Price {
        Price { room_type_id: DELUXE, date: day(offset), occupancy, amount }
    }

    fn restriction(offset: i64) -> Restriction {
        Restriction {
            room_type_id: DELUXE,
            date: day(offset),
            closed: false,
            min_stay: None,
            max_stay: None,
            closed_to_arrival: false,
            closed_to_departure: false,
        }
    }

    fn breakfast() -> MealSupplement {
        MealSupplement {
            id: Uuid::from_u128(4),
            property_id: Uuid::from_u128(3),
            meal_plan: MealPlan::Bb,
            currency: "USD".into(),
            adult_amount: 1_500,
            child_amount: 750,
            from: day(-30),
            to: None,
            version: 1,
        }
    }

    /// Three nights of two adults and a child in a deluxe room, bed and breakfast.
    fn stay(nights: i64) -> QuoteRequest {
        QuoteRequest {
            room_type_id: DELUXE,
            rate_plan_id: Uuid::from_u128(2),
            meal_plan: MealPlan::Bb,
            check_in: ARRIVAL,
            check_out: day(nights),
            adults: 2,
            children: 1,
            residency: Residency::NonResident,
        }
    }

    fn data() -> QuoteData {
        QuoteData {
            plan: plan(),
            room_type: QuoteRoomType {
                id: DELUXE,
                code: "DLX".into(),
                max_adults: 2,
                max_children: 1,
                max_occupancy: 3,
            },
            prices: vec![price(0, 2, 10_000), price(1, 2, 10_000), price(2, 2, 12_000), price(3, 2, 12_000)],
            restrictions: vec![],
            supplements: vec![breakfast()],
        }
    }

    fn kinds(request: &QuoteRequest, data: &QuoteData) -> Vec<(ViolationKind, Option<Date>)> {
        quote(request, data).violations.into_iter().map(|v| (v.kind, v.date)).collect()
    }

    #[test]
    fn nights_add_the_room_and_the_meal_supplement_per_person() {
        let quoted = quote(&stay(3), &data());

        let meal = 2 * 1_500 + 750;
        assert_eq!(
            quoted.nights,
            [
                QuoteNight { date: day(0), room: 10_000, meal },
                QuoteNight { date: day(1), room: 10_000, meal },
                QuoteNight { date: day(2), room: 12_000, meal },
            ]
        );
        assert_eq!(quoted.total, 32_000 + 3 * meal);
        assert_eq!(quoted.currency, "USD");
        assert!(quoted.restrictions_ok, "{:?}", quoted.violations);
    }

    #[test]
    fn room_only_has_no_supplement_and_a_missing_supplement_is_a_violation() {
        let room_only = quote(&QuoteRequest { meal_plan: MealPlan::Ro, ..stay(1) }, &data());
        let without_supplement = QuoteData { supplements: vec![], ..data() };

        assert_eq!(room_only.total, 10_000);
        assert_eq!(kinds(&stay(1), &without_supplement), [(ViolationKind::NoMealSupplement, Some(day(0)))]);
    }

    #[test]
    fn a_missing_occupancy_falls_back_to_the_nearest_lower_one_plus_extra_adults() {
        let single_priced = QuoteData { prices: vec![price(0, 1, 8_000), price(0, 3, 30_000)], ..data() };
        let only_triple = QuoteData { prices: vec![price(0, 3, 30_000)], ..data() };

        let quoted = quote(&stay(1), &single_priced);

        assert_eq!(quoted.nights[0].room, 8_000 + 2_500, "one adult's price plus one extra adult");
        assert_eq!(kinds(&stay(1), &only_triple), [(ViolationKind::NoPrice, Some(day(0)))]);
    }

    #[test]
    fn a_minimum_stay_on_any_night_of_the_stay_applies() {
        let second_night =
            QuoteData { restrictions: vec![Restriction { min_stay: Some(3), ..restriction(1) }], ..data() };
        let after_the_stay =
            QuoteData { restrictions: vec![Restriction { min_stay: Some(3), ..restriction(2) }], ..data() };

        assert_eq!(kinds(&stay(2), &second_night), [(ViolationKind::MinStay, Some(day(1)))]);
        assert!(quote(&stay(3), &second_night).restrictions_ok);
        assert!(quote(&stay(2), &after_the_stay).restrictions_ok, "the departure date is not a night");
    }

    #[test]
    fn a_maximum_stay_and_a_closed_night_apply_to_the_nights_of_the_stay() {
        let restrictions =
            vec![Restriction { max_stay: Some(2), ..restriction(0) }, Restriction { closed: true, ..restriction(2) }];
        let data = QuoteData { restrictions, ..data() };

        assert_eq!(
            kinds(&stay(3), &data),
            [(ViolationKind::MaxStay, Some(day(0))), (ViolationKind::Closed, Some(day(2)))]
        );
        assert!(quote(&stay(2), &data).restrictions_ok);
    }

    #[test]
    fn closed_to_arrival_counts_on_the_arrival_date_only() {
        let on_arrival =
            QuoteData { restrictions: vec![Restriction { closed_to_arrival: true, ..restriction(0) }], ..data() };
        let mid_stay =
            QuoteData { restrictions: vec![Restriction { closed_to_arrival: true, ..restriction(1) }], ..data() };

        assert_eq!(kinds(&stay(2), &on_arrival), [(ViolationKind::ClosedToArrival, Some(day(0)))]);
        assert!(quote(&stay(2), &mid_stay).restrictions_ok);
    }

    #[test]
    fn closed_to_departure_counts_on_the_departure_date_only() {
        let on_departure =
            QuoteData { restrictions: vec![Restriction { closed_to_departure: true, ..restriction(2) }], ..data() };
        let mid_stay =
            QuoteData { restrictions: vec![Restriction { closed_to_departure: true, ..restriction(1) }], ..data() };

        assert_eq!(kinds(&stay(2), &on_departure), [(ViolationKind::ClosedToDeparture, Some(day(2)))]);
        assert!(quote(&stay(2), &mid_stay).restrictions_ok);
    }

    #[test]
    fn residency_occupancy_meal_plan_room_type_and_activity_are_checked() {
        let residents_only = QuoteData { plan: RatePlan { residency: Some(Residency::Resident), ..plan() }, ..data() };
        let retired = QuoteData { plan: RatePlan { active: false, room_type_ids: vec![], ..plan() }, ..data() };

        assert_eq!(kinds(&stay(1), &residents_only), [(ViolationKind::Residency, None)]);
        assert_eq!(quote(&stay(1), &residents_only).violations[0].message, "BAR is sold to residents only");
        assert!(quote(&QuoteRequest { residency: Residency::Resident, ..stay(1) }, &residents_only).restrictions_ok);
        assert_eq!(kinds(&QuoteRequest { adults: 3, children: 0, ..stay(1) }, &data())[0].0, ViolationKind::Occupancy);
        assert_eq!(kinds(&QuoteRequest { children: 2, ..stay(1) }, &data())[0].0, ViolationKind::Occupancy);
        assert_eq!(
            kinds(&QuoteRequest { meal_plan: MealPlan::Hb, ..stay(1) }, &data())[0].0,
            ViolationKind::MealPlanNotAllowed
        );
        assert_eq!(
            kinds(&stay(1), &retired),
            [(ViolationKind::Inactive, None), (ViolationKind::RoomTypeNotSold, None)]
        );
    }

    #[test]
    fn empty_and_backwards_stays_give_invalid_stay() {
        let empty = QuoteRequest { check_out: ARRIVAL, ..stay(1) };
        let backwards = QuoteRequest { check_in: day(2), check_out: day(1), ..stay(1) };

        let empty_quoted = quote(&empty, &data());
        let backwards_quoted = quote(&backwards, &data());

        assert!(empty_quoted.nights.is_empty());
        assert_eq!(empty_quoted.total, 0);
        assert!(!empty_quoted.restrictions_ok);
        assert_eq!(empty_quoted.violations.len(), 1);
        assert_eq!(empty_quoted.violations[0].kind, ViolationKind::InvalidStay);

        assert!(backwards_quoted.nights.is_empty());
        assert_eq!(backwards_quoted.total, 0);
        assert!(!backwards_quoted.restrictions_ok);
        assert_eq!(backwards_quoted.violations.len(), 1);
        assert_eq!(backwards_quoted.violations[0].kind, ViolationKind::InvalidStay);
    }

    #[test]
    fn a_stay_over_max_stay_nights_gives_invalid_stay() {
        let too_long = QuoteRequest { check_out: ARRIVAL + time::Duration::days(731), ..stay(1) };

        let quoted = quote(&too_long, &data());

        assert!(quoted.nights.is_empty());
        assert_eq!(quoted.total, 0);
        assert!(!quoted.restrictions_ok);
        assert_eq!(quoted.violations.len(), 1);
        assert_eq!(quoted.violations[0].kind, ViolationKind::InvalidStay);
        assert!(quoted.violations[0].message.contains("731"));
    }

    #[test]
    fn check_out_at_date_max_does_not_panic() {
        let at_max = QuoteRequest { check_out: Date::MAX, ..stay(1) };

        let _ = quote(&at_max, &data());
        // If we got here without panicking, the test passes
    }

    #[test]
    fn zero_adults_gives_occupancy_violation_without_no_price() {
        let zero_adults = QuoteRequest { adults: 0, ..stay(1) };
        let quoted = quote(&zero_adults, &data());

        let violations: Vec<ViolationKind> = quoted.violations.iter().map(|v| v.kind).collect();
        assert_eq!(violations, vec![ViolationKind::Occupancy]);
        assert!(quoted.nights.is_empty());
    }

    #[test]
    fn negative_children_gives_occupancy_violation() {
        let negative_children = QuoteRequest { children: -1, ..stay(1) };
        let quoted = quote(&negative_children, &data());

        let violations: Vec<ViolationKind> = quoted.violations.iter().map(|v| v.kind).collect();
        assert_eq!(violations, vec![ViolationKind::Occupancy]);
    }

    #[test]
    fn fallback_across_two_extra_adults() {
        let room_type_3 = QuoteRoomType { max_adults: 3, ..data().room_type };
        let only_single = QuoteData { prices: vec![price(0, 1, 8_000)], room_type: room_type_3, ..data() };
        let three_adults = QuoteRequest { adults: 3, children: 0, ..stay(1) };

        let quoted = quote(&three_adults, &only_single);

        assert_eq!(quoted.nights[0].room, 8_000 + 2 * 2_500, "price for 1 adult plus 2 extra");
        assert!(quoted.restrictions_ok);
    }
}
