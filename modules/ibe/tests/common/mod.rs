#![allow(dead_code)]
//! A tenant with an enabled booking site, shared by the booking and manage tests.

use db::testing::app_pool;
use db::{Scope, TenantId, Tx, UserId, begin};
use ibe::{BookRoom, Booked, Booker, BookingError, BookingRequest, RoomAsk, SearchRequest};
use rates::{
    CancellationRule, DepositDue, DepositKind, DepositRule, MealPlan, NewCancellationPolicy, NewRatePlan, Penalty,
    PenaltyKind, PlanKind, RatePlan, Residency, Segment,
};
use rooms::{NewRoomType, RoomType};
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use std::collections::BTreeMap;
use time::{Date, Duration};
use uuid::Uuid;

/// A tenant with one property `GAL` in LKR with an enabled booking site, a `DLX` room type (2 adults and a child, 3 in
/// all) with `rooms` rooms, and an `IBE` plan `BAR` for anyone at 10 000 a night, room only or with breakfast, with a
/// cancellation policy and a deposit rule.
pub struct Hotel {
    pub pool: PgPool,
    /// The superuser, which reads the outbox.
    pub owner: PgPool,
    pub tenant: TenantId,
    pub user: UserId,
    pub property: Uuid,
    pub business_date: Date,
    pub deluxe: RoomType,
    pub bar: RatePlan,
}

impl Hotel {
    pub async fn new(opts: PgConnectOptions, rooms: u32) -> Self {
        let owner = PgPool::connect_with(opts.clone()).await.unwrap();
        let pool = app_pool(opts, 4).await;
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
        sqlx::query(
            "insert into ibe_site (tenant_id, property_id, enabled, public_code, contact_email)
             values ($1, $2, true, 'GAL', 'desk@gallefort.example')",
        )
        .bind(tenant.0)
        .bind(property.id)
        .execute(&mut *tx)
        .await
        .unwrap();
        let deluxe = NewRoomType {
            code: "DLX".into(),
            name: "Deluxe".into(),
            base_occupancy: 2,
            max_adults: 2,
            max_children: 1,
            max_occupancy: 3,
            overbooking: 0,
            bed_config: vec![],
            amenities: vec![],
        };
        let deluxe = rooms::create_room_type(&mut tx, tenant, user, property.id, deluxe).await.unwrap();
        for n in 1..=rooms {
            let room =
                rooms::NewRoom { room_type_id: deluxe.id, number: format!("10{n}"), floor: None, section_id: None };
            rooms::create_room(&mut tx, tenant, user, property.id, room).await.unwrap();
        }
        let policy = NewCancellationPolicy {
            name: "Flexible".into(),
            rules: vec![CancellationRule {
                days_before_arrival: 3,
                penalty: Penalty { kind: PenaltyKind::Nights, value: 1 },
            }],
            no_show: Penalty { kind: PenaltyKind::Nights, value: 1 },
        };
        let policy = rates::create_cancellation_policy(&mut tx, tenant, user, property.id, policy).await.unwrap();
        let bar = NewRatePlan {
            cancellation_policy_id: Some(policy.id),
            deposit_rule: Some(DepositRule {
                kind: DepositKind::Percent,
                value: 5_000,
                due: DepositDue::DaysBeforeArrival { days: 1 },
            }),
            ..new_plan("BAR", deluxe.id)
        };
        let bar = create_plan(&mut tx, (tenant, user, property.id), deluxe.id, property.business_date, bar).await;
        tx.commit().await.unwrap();
        Self { pool, owner, tenant, user, property: property.id, business_date: property.business_date, deluxe, bar }
    }

    pub async fn tx(&self) -> Tx {
        begin(&self.pool, Scope::tenant(self.tenant)).await.unwrap()
    }

    pub fn day(&self, days: i64) -> Date {
        self.business_date + Duration::days(days)
    }

    /// Creates `input`, priced at 10 000 a night from one adult for 60 days.
    pub async fn plan(&self, input: NewRatePlan) -> RatePlan {
        let mut tx = self.tx().await;
        let owner = (self.tenant, self.user, self.property);
        let plan = create_plan(&mut tx, owner, self.deluxe.id, self.business_date, input).await;
        tx.commit().await.unwrap();
        plan
    }

    /// A booking of `rooms` (adults, children) on `plan`, room only, for `[D + 1, D + 3)`, by Kamala Perera at
    /// `email`, for `residency`, at the totals the search shows for it.
    pub async fn request(
        &self,
        plan: &RatePlan,
        residency: Residency,
        rooms: &[(i32, i32)],
        email: &str,
    ) -> BookingRequest {
        let rooms: Vec<BookRoom> = rooms
            .iter()
            .map(|&(adults, children)| BookRoom {
                room_type_id: self.deluxe.id,
                rate_plan_id: plan.id,
                meal_plan: MealPlan::Ro,
                adults,
                children,
                occupant_name: None,
            })
            .collect();
        let mut request = BookingRequest {
            residency,
            booker: Booker {
                first_name: "Kamala".into(),
                last_name: "Perera".into(),
                email: email.into(),
                phone: "+94 77 123 4567".into(),
                country: "LK".into(),
                arrival_time: None,
            },
            rooms,
            check_in: self.day(1),
            check_out: self.day(3),
            special_requests: "A quiet room, please".into(),
            expected_totals: BTreeMap::new(),
            agreed: true,
        };
        request.expected_totals = self.totals(&request).await;
        request
    }

    /// The totals per currency the search shows for `request`'s rooms, or none when it offers one of them no more.
    pub async fn totals(&self, request: &BookingRequest) -> BTreeMap<String, i64> {
        let search = SearchRequest {
            check_in: request.check_in,
            check_out: request.check_out,
            residency: request.residency,
            rooms: request.rooms.iter().map(|room| RoomAsk { adults: room.adults, children: room.children }).collect(),
        };
        let result = ibe::search(&mut self.tx().await, self.property, &search).await.unwrap();
        let mut totals = BTreeMap::new();
        for (index, room) in request.rooms.iter().enumerate() {
            let offer = result
                .room_types
                .iter()
                .flat_map(|offered| offered.plans.iter())
                .filter(|plan| plan.rate_plan_id == room.rate_plan_id)
                .flat_map(|plan| plan.meal_plans.iter().map(move |offer| (plan, offer)))
                .find(|(_, offer)| offer.meal_plan == room.meal_plan && offer.rooms.contains(&(index as u8)));
            if let Some((plan, offer)) = offer {
                *totals.entry(plan.currency.clone()).or_default() += offer.total;
            }
        }
        totals
    }

    pub async fn book(&self, request: &BookingRequest) -> Result<Booked, BookingError> {
        let mut tx = self.tx().await;
        let booked = ibe::book(&mut tx, self.tenant, self.property, request).await?;
        tx.commit().await.unwrap();
        Ok(booked)
    }

    pub async fn reservations(&self) -> i64 {
        sqlx::query_scalar("select count(*) from reservation").fetch_one(&mut *self.tx().await).await.unwrap()
    }

    /// A transaction holding the `DLX` counter rows from D to D + 30 as a booking locks them, so a booking that
    /// reaches its inventory lock waits until it ends.
    pub async fn hold_inventory(&self) -> Tx {
        let mut tx = self.tx().await;
        rooms::lock_days(&mut tx, self.property, &[self.deluxe.id], self.day(0), self.day(30)).await.unwrap();
        tx
    }

    /// Waits until `count` sessions of this test's database wait on a lock, as bookings past their search do at the
    /// inventory lock [`Hotel::hold_inventory`] holds.
    pub async fn wait_for_lock_waiters(&self, count: i64) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let waiting: i64 = sqlx::query_scalar(
                "select count(*) from pg_stat_activity where datname = current_database() and wait_event_type = 'Lock'",
            )
            .fetch_one(&self.owner)
            .await
            .unwrap();
            if waiting >= count {
                return;
            }
            assert!(tokio::time::Instant::now() < deadline, "{waiting} sessions wait on a lock, not {count}");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}

/// A standard `IBE` plan `code` for anyone in LKR selling `room_type` room only or with breakfast.
pub fn new_plan(code: &str, room_type: Uuid) -> NewRatePlan {
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
        room_type_ids: vec![room_type],
    }
}

/// Creates `input` in the property of `(tenant, user, property)`, priced for `room_type` at 10 000 a night from one
/// adult for 60 days from `from`.
pub async fn create_plan(
    tx: &mut Tx,
    (tenant, user, property): (TenantId, UserId, Uuid),
    room_type: Uuid,
    from: Date,
    input: NewRatePlan,
) -> RatePlan {
    let plan = rates::create_rate_plan(tx, tenant, user, property, input).await.unwrap();
    let prices: Vec<rates::Price> = (0..60)
        .map(|day| rates::Price {
            room_type_id: room_type,
            date: from + Duration::days(day),
            occupancy: 1,
            amount: 10_000,
        })
        .collect();
    rates::set_prices(tx, tenant, user, property, plan.id, &prices).await.unwrap();
    plan
}
