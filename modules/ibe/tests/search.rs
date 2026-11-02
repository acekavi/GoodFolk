//! The booking site's search and site: which plans and room types it offers, their prices with the bill's taxes, the
//! cancellation wording, the nearest later check-in, and what the site shows.

use db::testing::{app_pool, guest_id_key};
use db::{Scope, TenantId, Tx, UserId, begin};
use ibe::{MealOffer, RoomAsk, SearchRequest, SearchResult};
use rates::{
    CancellationPolicy, CancellationRule, MealPlan, NewMealSupplement, NewRatePlan, Penalty, PenaltyKind, PlanKind,
    RatePlan, Residency, RestrictionChange, Segment,
};
use reservations::{NewGuest, NewReservation, NewReservationRoom, Source};
use rooms::{NewRoomType, RoomType};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::collections::BTreeMap;
use time::{Date, Duration};
use uuid::Uuid;

/// A tenant with one user and one property in LKR (VAT-registered), with room types `DLX` (3 rooms: 2 adults and a
/// child, 3 in all), `FAM` (4 rooms: 4 adults and 2 children, 6 in all) and `STD` (1 room: 2 adults).
struct Hotel {
    pool: PgPool,
    tenant: TenantId,
    user: UserId,
    property: Uuid,
    business_date: Date,
    deluxe: RoomType,
    family: RoomType,
    standard: RoomType,
}

impl Hotel {
    async fn new(opts: PgConnectOptions) -> Self {
        let pool = app_pool(opts, 2).await;
        let tenant = TenantId(Uuid::now_v7());
        let user = UserId(Uuid::now_v7());
        let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
        sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(tenant.0).execute(&mut *tx).await.unwrap();
        sqlx::query("insert into app_user (id, email, password_hash, display_name) values ($1, $2, 'x', 'U')")
            .bind(user.0)
            .bind(format!("{}@example.com", user.0))
            .execute(&mut *tx)
            .await
            .unwrap();
        let hotel = property::NewProperty {
            code: "GAL".into(),
            name: "Galle Fort Hotel".into(),
            timezone: "Asia/Colombo".into(),
            base_currency: "LKR".into(),
        };
        let property = property::create_property(&mut tx, tenant, user, hotel).await.unwrap();
        sqlx::query("update property set vat_registered = true where id = $1")
            .bind(property.id)
            .execute(&mut *tx)
            .await
            .unwrap();
        let room_type = |code: &str, adults: i32, children: i32, occupancy: i32| NewRoomType {
            code: code.into(),
            name: format!("Room {code}"),
            base_occupancy: 2,
            max_adults: adults,
            max_children: children,
            max_occupancy: occupancy,
            overbooking: 0,
            bed_config: vec![],
            amenities: vec![],
        };
        let mut created = Vec::new();
        for (input, rooms) in
            [(room_type("DLX", 2, 1, 3), 3), (room_type("FAM", 4, 2, 6), 4), (room_type("STD", 2, 0, 2), 1)]
        {
            let code = input.code.clone();
            let room_type = rooms::create_room_type(&mut tx, tenant, user, property.id, input).await.unwrap();
            for n in 1..=rooms {
                let room = rooms::NewRoom {
                    room_type_id: room_type.id,
                    number: format!("{code}{n}"),
                    floor: None,
                    section_id: None,
                };
                rooms::create_room(&mut tx, tenant, user, property.id, room).await.unwrap();
            }
            created.push(room_type);
        }
        tx.commit().await.unwrap();
        let standard = created.pop().unwrap();
        let family = created.pop().unwrap();
        let deluxe = created.pop().unwrap();
        Self {
            pool,
            tenant,
            user,
            property: property.id,
            business_date: property.business_date,
            deluxe,
            family,
            standard,
        }
    }

    async fn tx(&self) -> Tx {
        begin(&self.pool, Scope::tenant(self.tenant)).await.unwrap()
    }

    fn day(&self, days: i64) -> Date {
        self.business_date + Duration::days(days)
    }

    /// An active standard plan `code` of `segment` for `residency`, in `currency`, selling every room type room only
    /// or with breakfast, priced at `amount` a night from one adult for 60 days.
    async fn plan(&self, code: &str, segment: Segment, residency: Option<Residency>, currency: &str) -> RatePlan {
        self.plan_with(NewRatePlan { segment, residency, currency: currency.into(), ..self.new_plan(code) }, 10_000)
            .await
    }

    fn new_plan(&self, code: &str) -> NewRatePlan {
        NewRatePlan {
            code: code.into(),
            name: format!("Plan {code}"),
            kind: PlanKind::Standard,
            segment: Segment::Ibe,
            residency: None,
            currency: "LKR".into(),
            parent_id: None,
            derive_mode: None,
            derive_value: None,
            rounding_step: 1,
            extra_adult_amount: 1_000,
            inherit_restrictions: false,
            allowed_meal_plans: vec![MealPlan::Ro, MealPlan::Bb],
            cancellation_policy_id: None,
            deposit_rule: None,
            room_type_ids: vec![self.deluxe.id, self.family.id, self.standard.id],
        }
    }

    async fn plan_with(&self, input: NewRatePlan, amount: i64) -> RatePlan {
        let mut tx = self.tx().await;
        let plan = rates::create_rate_plan(&mut tx, self.tenant, self.user, self.property, input).await.unwrap();
        let prices: Vec<rates::Price> = plan
            .room_type_ids
            .iter()
            .flat_map(|room_type| {
                (0..60).map(|day| rates::Price { room_type_id: *room_type, date: self.day(day), occupancy: 1, amount })
            })
            .collect();
        rates::set_prices(&mut tx, self.tenant, self.user, self.property, plan.id, &prices).await.unwrap();
        tx.commit().await.unwrap();
        plan
    }

    /// A breakfast supplement of 1500 an adult and 500 a child in `currency`.
    async fn breakfast(&self, currency: &str) {
        let mut tx = self.tx().await;
        let breakfast = NewMealSupplement {
            meal_plan: MealPlan::Bb,
            currency: currency.into(),
            adult_amount: 1_500,
            child_amount: 500,
            from: self.day(0),
            to: None,
        };
        rates::create_meal_supplement(&mut tx, self.tenant, self.user, self.property, breakfast).await.unwrap();
        tx.commit().await.unwrap();
    }

    async fn restrict(&self, plan: &RatePlan, change: RestrictionChange) {
        let mut tx = self.tx().await;
        rates::set_restrictions(&mut tx, self.tenant, self.user, self.property, plan.id, &change).await.unwrap();
        tx.commit().await.unwrap();
    }

    /// No restriction on `[from, to)` of every room type yet.
    fn days(&self, from: i64, to: i64) -> RestrictionChange {
        RestrictionChange {
            from: self.day(from),
            to: self.day(to),
            weekdays: vec![],
            room_type_ids: vec![self.deluxe.id, self.family.id, self.standard.id],
            closed: None,
            min_stay: None,
            max_stay: None,
            closed_to_arrival: None,
            closed_to_departure: None,
        }
    }

    async fn search(&self, residency: Residency, from: i64, to: i64, rooms: &[(i32, i32)]) -> SearchResult {
        let request = SearchRequest {
            check_in: self.day(from),
            check_out: self.day(to),
            residency,
            rooms: rooms.iter().map(|&(adults, children)| RoomAsk { adults, children }).collect(),
        };
        ibe::search(&mut self.tx().await, self.property, &request).await.unwrap()
    }

    /// The plan names offered for each room type code.
    fn offered(&self, result: &SearchResult) -> BTreeMap<String, Vec<String>> {
        let code = |id: Uuid| {
            [&self.deluxe, &self.family, &self.standard].into_iter().find(|t| t.id == id).unwrap().code.clone()
        };
        result
            .room_types
            .iter()
            .map(|t| (code(t.room_type_id), t.plans.iter().map(|plan| plan.name.clone()).collect()))
            .collect()
    }

    fn room_type<'a>(&self, result: &'a SearchResult, room_type: &RoomType) -> &'a ibe::RoomTypeResult {
        result.room_types.iter().find(|t| t.room_type_id == room_type.id).expect("the room type is offered")
    }
}

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn residency_filters_plans(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.plan("BAR", Segment::Ibe, None, "LKR").await;
    hotel.plan("FIT-F", Segment::Ibe, Some(Residency::NonResident), "USD").await;
    hotel.plan("FIT-L", Segment::Ibe, Some(Residency::Resident), "LKR").await;

    let residents = hotel.search(Residency::Resident, 1, 3, &[(2, 0)]).await;
    let foreigners = hotel.search(Residency::NonResident, 1, 3, &[(2, 0)]).await;

    assert_eq!(hotel.offered(&residents)["DLX"], names(&["Plan BAR", "Plan FIT-L"]));
    assert_eq!(hotel.offered(&foreigners)["DLX"], names(&["Plan BAR", "Plan FIT-F"]));
    let fit_f = &hotel.room_type(&foreigners, &hotel.deluxe).plans[1];
    assert_eq!(fit_f.currency, "USD");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn only_ibe_segment_plans(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.plan("BAR", Segment::Ibe, None, "LKR").await;
    hotel.plan("OTA", Segment::Ota, None, "LKR").await;
    hotel.plan("TA", Segment::Ta, None, "LKR").await;
    let retired = hotel.plan("OLD", Segment::Ibe, None, "LKR").await;
    let mut tx = hotel.tx().await;
    sqlx::query("update rate_plan set active = false where id = $1").bind(retired.id).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();

    let result = hotel.search(Residency::Resident, 1, 3, &[(2, 0)]).await;

    for offered in hotel.offered(&result).values() {
        assert_eq!(offered, &names(&["Plan BAR"]));
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn restrictions_hide_plan(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.plan("BAR", Segment::Ibe, None, "LKR").await;
    let no_arrival = hotel.plan("CTA", Segment::Ibe, None, "LKR").await;
    let long_stays = hotel.plan("MIN", Segment::Ibe, None, "LKR").await;
    let closed = hotel.plan("SHUT", Segment::Ibe, None, "LKR").await;
    hotel.restrict(&no_arrival, RestrictionChange { closed_to_arrival: Some(true), ..hotel.days(1, 2) }).await;
    hotel.restrict(&long_stays, RestrictionChange { min_stay: Some(Some(3)), ..hotel.days(2, 3) }).await;
    hotel.restrict(&closed, RestrictionChange { closed: Some(true), ..hotel.days(2, 3) }).await;

    let restricted = hotel.search(Residency::Resident, 1, 3, &[(2, 0)]).await;
    let later = hotel.search(Residency::Resident, 3, 5, &[(2, 0)]).await;

    assert_eq!(hotel.offered(&restricted)["DLX"], names(&["Plan BAR"]));
    assert_eq!(hotel.offered(&later)["DLX"], names(&["Plan BAR", "Plan CTA", "Plan MIN", "Plan SHUT"]));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn four_mixed_rooms(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.plan("BAR", Segment::Ibe, None, "LKR").await;
    hotel.breakfast("LKR").await;

    let result = hotel.search(Residency::Resident, 1, 3, &[(2, 0), (2, 0), (4, 0), (2, 1)]).await;

    // DLX holds the three rooms of up to two adults and a child and has three; FAM holds all four and has four; STD
    // holds the two rooms of two adults but has only one.
    assert_eq!(hotel.offered(&result).keys().cloned().collect::<Vec<_>>(), names(&["DLX", "FAM"]));
    let deluxe = hotel.room_type(&result, &hotel.deluxe);
    let family = hotel.room_type(&result, &hotel.family);
    assert_eq!((deluxe.available, family.available), (3, 4));
    let shape = |offers: &[MealOffer]| -> Vec<(MealPlan, Vec<u8>, i64)> {
        offers.iter().map(|offer| (offer.meal_plan, offer.rooms.clone(), offer.nights[0].room)).collect()
    };
    assert_eq!(
        shape(&deluxe.plans[0].meal_plans),
        [
            (MealPlan::Ro, vec![0, 1], 11_000),
            (MealPlan::Ro, vec![3], 11_000),
            (MealPlan::Bb, vec![0, 1], 11_000),
            (MealPlan::Bb, vec![3], 11_000),
        ]
    );
    assert_eq!(
        shape(&family.plans[0].meal_plans),
        [
            (MealPlan::Ro, vec![0, 1], 11_000),
            (MealPlan::Ro, vec![3], 11_000),
            (MealPlan::Ro, vec![2], 13_000),
            (MealPlan::Bb, vec![0, 1], 11_000),
            (MealPlan::Bb, vec![3], 11_000),
            (MealPlan::Bb, vec![2], 13_000),
        ]
    );
    let breakfast_for_four =
        family.plans[0].meal_plans.iter().find(|offer| offer.rooms == [2] && offer.meal_plan == MealPlan::Bb);
    assert_eq!(breakfast_for_four.unwrap().nights[0].meal, 4 * 1_500);
}

/// The lines posted for `stay` on `date`: `(category, total)`, and the bill's taxes on them by name.
async fn posted(hotel: &Hotel, stay: Uuid, date: Date) -> (i64, BTreeMap<String, i64>) {
    let keys = [format!("room:{stay}:{date}"), format!("meal_plan:{stay}:{date}")];
    let mut tx = hotel.tx().await;
    let total: i64 = sqlx::query_scalar("select sum(total)::bigint from folio_line where posting_key = any($1)")
        .bind(&keys)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let taxes: Vec<(String, i64)> = sqlx::query_as(
        "select t.name, sum(t.amount)::bigint from folio_line_tax t join folio_line l on l.id = t.line_id
         where l.posting_key = any($1) and t.presentation = 'on_bill' group by t.name",
    )
    .bind(&keys)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    (total, taxes.into_iter().collect())
}

/// Books two adults on `plan` with breakfast from the business date for 2 nights, checks in, and posts the second
/// night as the night audit does. Returns the stay.
async fn booked_and_posted(hotel: &Hotel, plan: &RatePlan) -> Uuid {
    let mut tx = hotel.tx().await;
    let guest = NewGuest {
        first_name: "Ada".into(),
        last_name: "Silva".into(),
        email: None,
        phone: None,
        country: None,
        residency: Residency::Resident,
        notes: String::new(),
        id_doc: None,
    };
    let guest = reservations::create_guest(&mut tx, hotel.tenant, hotel.user, &guest_id_key(), guest).await.unwrap();
    let room = NewReservationRoom {
        room_type_id: hotel.deluxe.id,
        rate_plan_id: plan.id,
        meal_plan: MealPlan::Bb,
        check_in: hotel.day(0),
        check_out: hotel.day(2),
        adults: 2,
        children: 0,
        primary_guest_id: None,
    };
    let input = NewReservation {
        booker_guest_id: guest.id,
        source: Source::Phone,
        notes: String::new(),
        account_id: None,
        rooms: vec![room],
    };
    let booked =
        reservations::create_reservation(&mut tx, hotel.tenant, hotel.user, hotel.property, input).await.unwrap();
    let stay = &booked.rooms[0];
    reservations::check_in(&mut tx, hotel.tenant, hotel.user, hotel.property, stay.id, stay.version, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let report =
        folio::post_business_date_charges(&hotel.pool, hotel.tenant, Some(hotel.user), hotel.property, hotel.day(1))
            .await
            .unwrap();
    assert_eq!((report.lines, report.failures.len()), (2, 0), "{report:?}");
    stay.id
}

/// The Sri Lanka preset, in force from the day after the business date.
async fn preset(hotel: &Hotel) {
    let mut tx = hotel.tx().await;
    folio::apply_preset(&mut tx, hotel.tenant, Some(hotel.user), hotel.property).await.unwrap();
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn tax_preview_matches_folio_posting(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let plan = hotel.plan("BAR", Segment::Ibe, None, "LKR").await;
    hotel.breakfast("LKR").await;
    preset(&hotel).await;
    let stay = booked_and_posted(&hotel, &plan).await;

    let result = hotel.search(Residency::Resident, 1, 2, &[(2, 0)]).await;

    let (total, taxes) = posted(&hotel, stay, hotel.day(1)).await;
    let offer = &hotel.room_type(&result, &hotel.deluxe).plans[0].meal_plans;
    let breakfast = offer.iter().find(|offer| offer.meal_plan == MealPlan::Bb).unwrap();
    let previewed: BTreeMap<String, i64> =
        breakfast.taxes.iter().map(|line| (line.name.clone(), line.amount)).collect();
    assert_eq!(previewed, taxes);
    assert_eq!(previewed.len(), 3, "TDL, SSCL and VAT on the room and the breakfast: {previewed:?}");
    assert_eq!(breakfast.total, total);
    assert_eq!(breakfast.nights[0].total, total);
    assert_eq!((breakfast.nights[0].room, breakfast.nights[0].meal), (11_000, 3_000));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn inclusive_plan_back_calculates(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let plan = hotel.plan("BAR", Segment::Ibe, None, "LKR").await;
    hotel.breakfast("LKR").await;
    preset(&hotel).await;
    let mut tx = hotel.tx().await;
    sqlx::query("update rate_plan set prices_include_tax = true where id = $1")
        .bind(plan.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let stay = booked_and_posted(&hotel, &plan).await;

    let result = hotel.search(Residency::Resident, 1, 3, &[(2, 0)]).await;

    let offers = &hotel.room_type(&result, &hotel.deluxe).plans[0].meal_plans;
    let breakfast = offers.iter().find(|offer| offer.meal_plan == MealPlan::Bb).unwrap();
    for night in &breakfast.nights {
        assert_eq!(night.total, night.room + night.meal, "the price already includes the taxes");
    }
    assert_eq!(breakfast.total, 2 * (11_000 + 3_000));
    assert!(breakfast.taxes.iter().all(|line| line.amount > 0) && breakfast.taxes.len() == 3, "{:?}", breakfast.taxes);
    let (total, taxes) = posted(&hotel, stay, hotel.day(1)).await;
    let one_night = hotel.search(Residency::Resident, 1, 2, &[(2, 0)]).await;
    let one_night = &hotel.room_type(&one_night, &hotel.deluxe).plans[0].meal_plans;
    let one_night = one_night.iter().find(|offer| offer.meal_plan == MealPlan::Bb).unwrap();
    assert_eq!(one_night.total, total);
    assert_eq!(one_night.taxes.iter().map(|line| (line.name.clone(), line.amount)).collect::<BTreeMap<_, _>>(), taxes);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_meal_line_is_taxed_as_a_meal_plan_not_a_room(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let plan = hotel.plan("BAR", Segment::Ibe, None, "LKR").await;
    hotel.breakfast("LKR").await;
    let room_tax = tax::Rule {
        code: "ROOMTAX".into(),
        name: "Room tax".into(),
        kind: tax::Kind::Tax,
        rate_bp: 1_000,
        applies_to: vec!["room".into()],
        base: vec!["net".into()],
        sequence: 10,
        presentation: tax::Presentation::OnBill,
        condition: tax::Condition::Always,
        effective_from: hotel.day(1),
        effective_to: None,
    };
    let mut tx = hotel.tx().await;
    folio::create_tax_rule(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, room_tax).await.unwrap();
    tx.commit().await.unwrap();
    let stay = booked_and_posted(&hotel, &plan).await;

    let result = hotel.search(Residency::Resident, 1, 2, &[(2, 0)]).await;

    let offers = &hotel.room_type(&result, &hotel.deluxe).plans[0].meal_plans;
    let breakfast = offers.iter().find(|offer| offer.meal_plan == MealPlan::Bb).unwrap();
    // The property's own levies apply to both lines; the room tax only to the room's 11000, not the breakfast's 3000.
    let room_tax = breakfast.taxes.iter().find(|line| line.name == "Room tax").unwrap();
    assert_eq!(room_tax.amount, 1_100);
    let (total, posted) = posted(&hotel, stay, hotel.day(1)).await;
    assert_eq!(posted["Room tax"], 1_100);
    assert_eq!(breakfast.total, total);
    assert_eq!(breakfast.taxes.iter().map(|line| (line.name.clone(), line.amount)).collect::<BTreeMap<_, _>>(), posted);
}

#[test]
fn cancellation_text_wording() {
    let policy = |rules: Vec<(i32, PenaltyKind, i64)>, no_show: (PenaltyKind, i64)| CancellationPolicy {
        id: Uuid::nil(),
        property_id: Uuid::nil(),
        name: "Policy".into(),
        rules: rules
            .into_iter()
            .map(|(days, kind, value)| CancellationRule { days_before_arrival: days, penalty: Penalty { kind, value } })
            .collect(),
        no_show: Penalty { kind: no_show.0, value: no_show.1 },
        version: 1,
    };
    let text = |rules, no_show| ibe::cancellation_text(Some(&policy(rules, no_show)), "USD");

    assert_eq!(
        text(vec![(3, PenaltyKind::Nights, 1)], (PenaltyKind::Nights, 1)),
        "Free cancellation until 3 days before arrival; after that, one night is charged; a no-show is charged one \
         night."
    );
    assert_eq!(
        text(
            vec![(1, PenaltyKind::Percent, 10_000), (14, PenaltyKind::Percent, 1_250), (7, PenaltyKind::Nights, 2)],
            (PenaltyKind::Percent, 10_000)
        ),
        "Free cancellation until 14 days before arrival; after that, 12.5% of the stay is charged; from 7 days before \
         arrival, 2 nights are charged; from 1 day before arrival, the whole stay is charged; a no-show is charged \
         the whole stay."
    );
    assert_eq!(
        text(vec![(0, PenaltyKind::Amount, 5_000)], (PenaltyKind::Nights, 2)),
        "Free cancellation until the day of arrival; after that, USD 50.00 is charged; a no-show is charged 2 nights."
    );
    let yen =
        ibe::cancellation_text(Some(&policy(vec![(2, PenaltyKind::Amount, 500)], (PenaltyKind::Amount, 900))), "JPY");
    assert!(yen.contains("JPY 500 is charged") && yen.ends_with("a no-show is charged JPY 900."), "{yen}");
    assert_eq!(text(vec![], (PenaltyKind::Percent, 5_000)), "Free cancellation; a no-show is charged 50% of the stay.");
    assert_eq!(ibe::cancellation_text(None, "USD"), "Free cancellation.");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_plans_deposit_rule_prices_each_offer_as_the_booked_room(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.breakfast("LKR").await;
    let rule = rates::DepositRule {
        kind: rates::DepositKind::Percent,
        value: 5_000,
        due: rates::DepositDue::DaysBeforeArrival { days: 7 },
    };
    let plan = hotel.plan_with(NewRatePlan { deposit_rule: Some(rule), ..hotel.new_plan("DEP") }, 10_000).await;
    hotel.plan("BAR", Segment::Ibe, None, "LKR").await;

    let result = hotel.search(Residency::Resident, 10, 13, &[(2, 0)]).await;

    let plans = &hotel.room_type(&result, &hotel.deluxe).plans;
    let offered = |name: &str| plans.iter().find(|offered| offered.name == name).unwrap();
    assert!(offered("Plan BAR").meal_plans.iter().all(|offer| offer.deposit.is_none()));
    let breakfast = offered("Plan DEP").meal_plans.iter().find(|offer| offer.meal_plan == MealPlan::Bb).unwrap();
    let deposit = breakfast.deposit.clone().expect("the plan has a deposit rule");
    let mut tx = hotel.tx().await;
    let guest = NewGuest {
        first_name: "Ada".into(),
        last_name: "Silva".into(),
        email: None,
        phone: None,
        country: None,
        residency: Residency::Resident,
        notes: String::new(),
        id_doc: None,
    };
    let guest = reservations::create_guest(&mut tx, hotel.tenant, hotel.user, &guest_id_key(), guest).await.unwrap();
    let room = NewReservationRoom {
        room_type_id: hotel.deluxe.id,
        rate_plan_id: plan.id,
        meal_plan: MealPlan::Bb,
        check_in: hotel.day(10),
        check_out: hotel.day(13),
        adults: 2,
        children: 0,
        primary_guest_id: None,
    };
    let input = NewReservation {
        booker_guest_id: guest.id,
        source: Source::Phone,
        notes: String::new(),
        account_id: None,
        rooms: vec![room],
    };
    let booked =
        reservations::create_reservation(&mut tx, hotel.tenant, hotel.user, hotel.property, input).await.unwrap();
    let due = reservations::deposit_due(&mut tx, booked.id).await.unwrap();

    // Half the stay before taxes, due a week before arrival: what the booked room's confirmation email shows.
    let before_taxes: i64 = breakfast.nights.iter().map(|night| night.room + night.meal).sum();
    assert_eq!(deposit, ibe::OfferDeposit { amount: before_taxes / 2, due_date: hotel.day(3) });
    assert_eq!((due.len(), due[0].amount, due[0].due_date), (1, deposit.amount, deposit.due_date));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn next_available_within_14_days(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan("BAR", Segment::Ibe, None, "LKR").await;
    // Closed on days 1 to 4, and every room sold on day 5: two nights from day 6 are the first that sell.
    hotel.restrict(&bar, RestrictionChange { closed: Some(true), ..hotel.days(1, 5) }).await;
    let mut tx = hotel.tx().await;
    sqlx::query("update inventory_day set sold = physical where date = $1")
        .bind(hotel.day(5))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let full = hotel.search(Residency::Resident, 1, 3, &[(2, 0)]).await;
    let open = hotel.search(Residency::Resident, 6, 8, &[(2, 0)]).await;
    hotel.restrict(&bar, RestrictionChange { closed: Some(true), ..hotel.days(1, 40) }).await;
    let none = hotel.search(Residency::Resident, 1, 3, &[(2, 0)]).await;

    assert!(full.room_types.is_empty());
    assert_eq!(full.next_available, Some(hotel.day(6)));
    assert_eq!((open.room_types.len(), open.next_available), (3, None));
    assert_eq!((none.room_types.len(), none.next_available), (0, None));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_outside_the_sites_limits_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let ask = |check_in: i64, check_out: i64, rooms: usize, adults: i32| SearchRequest {
        check_in: hotel.day(check_in),
        check_out: hotel.day(check_out),
        residency: Residency::Resident,
        rooms: vec![RoomAsk { adults, children: 0 }; rooms],
    };

    for refused in [
        ask(-1, 1, 1, 2),
        ask(1, 1, 1, 2),
        ask(1, 32, 1, 2),
        ask(1, 2, 0, 2),
        ask(1, 2, 5, 2),
        ask(1, 2, 1, 0),
        ask(560, 561, 1, 2),
    ] {
        let result = ibe::search(&mut hotel.tx().await, hotel.property, &refused).await;
        assert!(matches!(result, Err(ibe::IbeError::Invalid(_))), "{refused:?}: {result:?}");
    }
    assert!(ibe::search(&mut hotel.tx().await, hotel.property, &ask(0, 30, 4, 2)).await.is_ok());
}

/// A ready `purpose` asset of the property with `renditions`, in `room_type`'s gallery at `position` when given.
async fn asset(
    hotel: &Hotel,
    purpose: &str,
    status: &str,
    renditions: serde_json::Value,
    gallery: Option<(Uuid, i32)>,
) -> Uuid {
    let id = Uuid::now_v7();
    let mut tx = hotel.tx().await;
    sqlx::query(
        "insert into media_asset (id, tenant_id, property_id, purpose, visibility, status, filename, declared_bytes,
                                  sha256, width, height, renditions)
         values ($1, $2, $3, $4, 'public', $5, 'a.jpg', 10, sha256($1::text::bytea), 1600, 900, $6)",
    )
    .bind(id)
    .bind(hotel.tenant.0)
    .bind(hotel.property)
    .bind(purpose)
    .bind(status)
    .bind(renditions)
    .execute(&mut *tx)
    .await
    .unwrap();
    if let Some((room_type, position)) = gallery {
        sqlx::query(
            "insert into room_type_media (tenant_id, room_type_id, asset_id, position, caption) values ($1, $2, $3, $4, 'Sea view')",
        )
        .bind(hotel.tenant.0)
        .bind(room_type)
        .bind(id)
        .bind(position)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    id
}

fn rendition(size: &str, format: &str, key: &str) -> serde_json::Value {
    serde_json::json!({"size": size, "format": format, "width": 1600, "height": 900, "bytes": 10, "key": key,
                       "duration_ms": if format.ends_with("264") || format == "av1" { Some(12_000) } else { None }})
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn site_lists_gallery_and_video(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let photo = asset(
        &hotel,
        "gallery",
        "ready",
        serde_json::json!([rendition("md", "avif", "g/md.avif"), rendition("md", "webp", "g/md.webp")]),
        Some((hotel.deluxe.id, 0)),
    )
    .await;
    asset(
        &hotel,
        "gallery_video",
        "ready",
        serde_json::json!([
            rendition("lg", "webp", "v/poster.webp"),
            rendition("v720", "h264", "v/720.h264.mp4"),
            rendition("v1080", "h264", "v/1080.h264.mp4"),
            rendition("v1080", "av1", "v/1080.av1.mp4"),
        ]),
        Some((hotel.deluxe.id, 1)),
    )
    .await;
    asset(&hotel, "gallery", "processing", serde_json::json!([]), Some((hotel.deluxe.id, 2))).await;
    let logo =
        asset(&hotel, "ibe_logo", "ready", serde_json::json!([rendition("sm", "webp", "l/sm.webp")]), None).await;
    let mut tx = hotel.tx().await;
    let changes = property::IbeSiteChanges {
        enabled: true,
        custom_host: None,
        logo_asset_id: Some(logo),
        hero_asset_id: Some(photo),
        accent: Some("#1d4ed8".into()),
        description: "By the ramparts".into(),
        contact_phone: Some("+94 91 000 0000".into()),
        contact_email: Some("stay@galle.example".into()),
        terms: "No smoking.".into(),
        privacy: "We keep your name.".into(),
    };
    property::update_ibe_site(&mut tx, hotel.tenant, hotel.user, hotel.property, 0, changes).await.unwrap();
    sqlx::query("update room_type set active = false, description = 'Gone' where id = $1")
        .bind(hotel.standard.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("update room_type set description = 'Sea facing', amenities = '{wifi}' where id = $1")
        .bind(hotel.deluxe.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let site =
        ibe::site(&mut hotel.tx().await, hotel.property, |key| format!("https://media.example/{key}")).await.unwrap();

    assert_eq!((site.code.as_str(), site.name.as_str()), ("GAL", "Galle Fort Hotel"));
    assert_eq!(site.accent.as_deref(), Some("#1d4ed8"));
    assert_eq!(site.logo[0].url, "https://media.example/l/sm.webp");
    assert_eq!(site.hero.as_ref().unwrap().images.len(), 2);
    assert_eq!(site.room_types.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Room DLX", "Room FAM"]);
    let deluxe = &site.room_types[0];
    assert_eq!((deluxe.description.as_str(), deluxe.amenities.clone()), ("Sea facing", vec!["wifi".to_owned()]));
    assert_eq!((deluxe.max_adults, deluxe.max_children, deluxe.max_occupancy), (2, 1, 3));
    assert_eq!(deluxe.media.len(), 2, "the processing photo is left out");
    assert_eq!(deluxe.media[0].kind, ibe::MediaKind::Image);
    assert_eq!(deluxe.media[0].caption.as_deref(), Some("Sea view"));
    assert_eq!(deluxe.media[0].images[1].url, "https://media.example/g/md.webp");
    let video = &deluxe.media[1];
    assert_eq!(video.kind, ibe::MediaKind::Video);
    assert_eq!(video.duration_ms, Some(12_000));
    assert_eq!(
        video.images.iter().map(|image| image.url.as_str()).collect::<Vec<_>>(),
        ["https://media.example/v/poster.webp"]
    );
    assert_eq!(
        video.sources.iter().map(|source| source.url.as_str()).collect::<Vec<_>>(),
        [
            "https://media.example/v/1080.av1.mp4",
            "https://media.example/v/1080.h264.mp4",
            "https://media.example/v/720.h264.mp4"
        ]
    );
    assert!(video.sources[0].mime.starts_with("video/mp4; codecs=\"av01"));
}
