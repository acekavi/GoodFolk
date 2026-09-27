//! Offers: every way a stay can be sold at a property, each priced by [`quote`]. [`load_offers`] reads the rows
//! for all plans and room types at once, so its number of queries does not grow with the number of offers.

use crate::plans::{MealPlan, Residency, list_rate_plans};
use crate::quote::{
    Quote, QuoteData, QuoteRequest, quote, read_prices, read_restrictions, read_room_types, read_supplements,
    stay_nights,
};
use crate::{MealSupplement, Price, RatesError, Restriction};
use db::Tx;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};
use time::Date;
use uuid::Uuid;

/// Meal plans in the order offers list them.
const MEAL_PLAN_ORDER: [MealPlan; 4] = [MealPlan::Ro, MealPlan::Bb, MealPlan::Hb, MealPlan::Fb];

/// A stay to find offers for: `[check_in, check_out)`, for guests of `residency`, in `room_type_ids` (`None`:
/// every room type a plan sells).
#[derive(Debug, Clone)]
pub struct OfferRequest {
    pub check_in: Date,
    pub check_out: Date,
    pub adults: i32,
    pub children: i32,
    pub residency: Residency,
    pub room_type_ids: Option<Vec<Uuid>>,
}

/// One room type sold on one plan with one meal plan, and what the stay costs that way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Offer {
    pub room_type_id: Uuid,
    pub rate_plan_id: Uuid,
    pub rate_plan_code: String,
    pub meal_plan: MealPlan,
    pub quote: Quote,
}

/// Every offer for the stay: each active plan that sells to `residency`, for each room type it sells (among
/// `room_type_ids`), with each meal plan it allows. Each quote is the one [`load_quote`](crate::load_quote)
/// gives for that combination, violations included, so unsellable offers come back with their reasons.
/// Sorted by room type display order, then plan code, then meal plan (RO, BB, HB, FB).
///
/// Room types are not filtered on being active, as [`load_quote`](crate::load_quote) does not: callers that
/// sell only active types pass them in `room_type_ids`.
///
/// Reads plans, room types, prices, restrictions and supplements in five queries whatever the number of plans
/// and room types, then prices each combination in memory.
pub async fn load_offers(tx: &mut Tx, property: Uuid, request: &OfferRequest) -> Result<Vec<Offer>, RatesError> {
    stay_nights(request.check_in, request.check_out).map_err(RatesError::Invalid)?;
    let wanted = |room_type: &Uuid| request.room_type_ids.as_ref().is_none_or(|ids| ids.contains(room_type));
    let mut plans: Vec<_> = list_rate_plans(tx, property)
        .await?
        .into_iter()
        .filter(|plan| plan.active && plan.residency.is_none_or(|residency| residency == request.residency))
        .collect();
    plans.sort_by(|a, b| a.code.cmp(&b.code));

    let room_type_ids: Vec<Uuid> = plans
        .iter()
        .flat_map(|plan| plan.room_type_ids.iter().copied().filter(wanted))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let plan_ids: Vec<Uuid> = plans.iter().map(|plan| plan.id).collect();
    let meal_plans: Vec<MealPlan> = plans
        .iter()
        .flat_map(|plan| plan.allowed_meal_plans.iter().copied())
        .filter(|meal_plan| *meal_plan != MealPlan::Ro)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let currencies: Vec<String> =
        plans.iter().map(|plan| plan.currency.clone()).collect::<BTreeSet<_>>().into_iter().collect();
    let (check_in, check_out) = (request.check_in, request.check_out);

    let room_types = read_room_types(tx, property, &room_type_ids).await?;
    let mut prices: HashMap<(Uuid, Uuid), Vec<Price>> = HashMap::new();
    for row in read_prices(tx, &plan_ids, &room_type_ids, check_in, check_out).await? {
        prices.entry((row.rate_plan_id, row.price.room_type_id)).or_default().push(row.price);
    }
    let mut restrictions: HashMap<(Uuid, Uuid), Vec<Restriction>> = HashMap::new();
    for row in read_restrictions(tx, &plan_ids, &room_type_ids, check_in, check_out).await? {
        restrictions.entry((row.rate_plan_id, row.restriction.room_type_id)).or_default().push(row.restriction);
    }
    let mut supplements: HashMap<(MealPlan, String), Vec<MealSupplement>> = HashMap::new();
    for supplement in read_supplements(tx, property, &meal_plans, &currencies, check_in, check_out).await? {
        supplements.entry((supplement.meal_plan, supplement.currency.clone())).or_default().push(supplement);
    }

    let mut offers = Vec::new();
    for room_type in &room_types {
        for plan in plans.iter().filter(|plan| plan.room_type_ids.contains(&room_type.id)) {
            let mut allowed = plan.allowed_meal_plans.clone();
            allowed.sort_by_key(|meal_plan| MEAL_PLAN_ORDER.iter().position(|m| m == meal_plan));
            for meal_plan in allowed {
                let key = (plan.id, room_type.id);
                let data = QuoteData {
                    plan: plan.clone(),
                    room_type: room_type.clone(),
                    prices: prices.get(&key).cloned().unwrap_or_default(),
                    restrictions: restrictions.get(&key).cloned().unwrap_or_default(),
                    supplements: supplements.get(&(meal_plan, plan.currency.clone())).cloned().unwrap_or_default(),
                };
                let single = QuoteRequest {
                    room_type_id: room_type.id,
                    rate_plan_id: plan.id,
                    meal_plan,
                    check_in,
                    check_out,
                    adults: request.adults,
                    children: request.children,
                    residency: request.residency,
                };
                offers.push(Offer {
                    room_type_id: room_type.id,
                    rate_plan_id: plan.id,
                    rate_plan_code: plan.code.clone(),
                    meal_plan,
                    quote: quote(&single, &data),
                });
            }
        }
    }
    Ok(offers)
}
