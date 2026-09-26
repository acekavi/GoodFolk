#![allow(dead_code)] // each test binary uses a different subset

use db::testing::app_pool;
use db::{Scope, TenantId, Tx, UserId, begin};
use rates::{ChangeMode, MealPlan, NewRatePlan, PlanKind, RatePlan, RatesError, Segment};
use rooms::{NewRoomType, RoomType};
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use time::{Date, Duration};
use uuid::Uuid;

/// A tenant with one user, one property and two room types: `DLX` (up to 2 adults and 1 child) and `STD`
/// (up to 2 adults).
pub struct Hotel {
    pub pool: PgPool,
    pub tenant: TenantId,
    pub user: UserId,
    pub property: Uuid,
    pub business_date: Date,
    pub deluxe: RoomType,
    pub standard: RoomType,
}

impl Hotel {
    pub async fn new(opts: PgConnectOptions) -> Self {
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
            name: "Galle".into(),
            timezone: "Asia/Colombo".into(),
            base_currency: "LKR".into(),
        };
        let property = property::create_property(&mut tx, tenant, user, hotel).await.unwrap();
        let deluxe = NewRoomType {
            code: "DLX".into(),
            name: "Deluxe".into(),
            base_occupancy: 2,
            max_adults: 2,
            max_children: 1,
            max_occupancy: 3,
            bed_config: vec![],
            amenities: vec![],
        };
        let standard = NewRoomType {
            code: "STD".into(),
            name: "Standard".into(),
            base_occupancy: 1,
            max_adults: 2,
            max_children: 0,
            max_occupancy: 2,
            ..deluxe.clone()
        };
        let deluxe = rooms::create_room_type(&mut tx, tenant, user, property.id, deluxe).await.unwrap();
        let standard = rooms::create_room_type(&mut tx, tenant, user, property.id, standard).await.unwrap();
        tx.commit().await.unwrap();
        Self { pool, tenant, user, property: property.id, business_date: property.business_date, deluxe, standard }
    }

    pub async fn tx(&self) -> Tx {
        begin(&self.pool, Scope::tenant(self.tenant)).await.unwrap()
    }

    /// The business date plus `days`.
    pub fn day(&self, days: i64) -> Date {
        self.business_date + Duration::days(days)
    }

    /// A standard plan in `currency` selling both room types.
    pub fn standard_plan(&self, code: &str, currency: &str) -> NewRatePlan {
        NewRatePlan {
            code: code.into(),
            name: format!("Plan {code}"),
            kind: PlanKind::Standard,
            segment: Segment::Ibe,
            residency: None,
            currency: currency.into(),
            parent_id: None,
            derive_mode: None,
            derive_value: None,
            rounding_step: 1,
            extra_adult_amount: 0,
            inherit_restrictions: false,
            allowed_meal_plans: vec![MealPlan::Ro, MealPlan::Bb],
            cancellation_policy_id: None,
            room_type_ids: vec![self.deluxe.id, self.standard.id],
        }
    }

    /// A plan derived from `parent` by `value` of `mode`, in the parent's currency and room types.
    pub fn derived_plan(&self, code: &str, parent: &RatePlan, mode: ChangeMode, value: i64) -> NewRatePlan {
        NewRatePlan {
            kind: PlanKind::Derived,
            segment: Segment::Ota,
            parent_id: Some(parent.id),
            derive_mode: Some(mode),
            derive_value: Some(value),
            room_type_ids: parent.room_type_ids.clone(),
            ..self.standard_plan(code, &parent.currency)
        }
    }

    /// Creates a plan in its own transaction, committed if it succeeds.
    pub async fn try_plan(&self, input: NewRatePlan) -> Result<RatePlan, RatesError> {
        let mut tx = self.tx().await;
        let created = rates::create_rate_plan(&mut tx, self.tenant, self.user, self.property, input).await?;
        tx.commit().await.unwrap();
        Ok(created)
    }

    pub async fn plan(&self, input: NewRatePlan) -> RatePlan {
        self.try_plan(input).await.unwrap()
    }

    /// Changes a plan in its own transaction, committed if it succeeds.
    pub async fn try_update(&self, plan: &RatePlan, changes: rates::RatePlanChanges) -> Result<RatePlan, RatesError> {
        let mut tx = self.tx().await;
        let updated =
            rates::update_rate_plan(&mut tx, self.tenant, self.user, self.property, plan.id, plan.version, changes)
                .await?;
        tx.commit().await.unwrap();
        Ok(updated)
    }

    pub async fn plans(&self) -> Vec<RatePlan> {
        rates::list_rate_plans(&mut self.tx().await, self.property).await.unwrap()
    }
}

/// `app.derive_amount` computed exactly: `base` plus `value` basis points (`percent`) or minor units
/// (`amount`), at least 0, rounded half-up to a multiple of `step`.
pub fn derived(base: i64, mode: ChangeMode, value: i64, step: i64) -> i64 {
    let (numerator, denominator) = match mode {
        ChangeMode::Percent => (i128::from(base) * (10_000 + i128::from(value)), 10_000_i128),
        ChangeMode::Amount => (i128::from(base) + i128::from(value), 1),
    };
    let unit = denominator * i128::from(step);
    let (whole, rest) = (numerator.max(0) / unit, numerator.max(0) % unit);
    let steps = if 2 * rest >= unit { whole + 1 } else { whole };
    i64::try_from(steps * i128::from(step)).unwrap()
}

impl Hotel {
    /// Sets `plan`'s prices in their own transaction, committed if it succeeds.
    pub async fn try_prices(&self, plan: &RatePlan, prices: &[rates::Price]) -> Result<(), RatesError> {
        let mut tx = self.tx().await;
        rates::set_prices(&mut tx, self.tenant, self.user, self.property, plan.id, prices).await?;
        tx.commit().await.unwrap();
        Ok(())
    }

    /// `amount` for `occupancy` adults in `room_type` on each day in `[from, to)`.
    pub fn prices(&self, room_type: Uuid, from: i64, to: i64, occupancy: i32, amount: i64) -> Vec<rates::Price> {
        (from..to).map(|day| rates::Price { room_type_id: room_type, date: self.day(day), occupancy, amount }).collect()
    }

    /// `plan`'s prices over the whole rate window, by date, room type and occupancy.
    pub async fn stored(&self, plan: &RatePlan) -> Vec<rates::Price> {
        let window = (self.day(0), self.day(rooms::WINDOW_DAYS));
        rates::list_prices(&mut self.tx().await, self.property, plan.id, window.0, window.1).await.unwrap()
    }
}

impl Hotel {
    /// Restrictions on every day of `[from, to)` for `room_types`, with no field set yet.
    pub fn restrict(&self, room_types: &[Uuid], from: i64, to: i64) -> rates::RestrictionChange {
        rates::RestrictionChange {
            from: self.day(from),
            to: self.day(to),
            weekdays: vec![],
            room_type_ids: room_types.to_vec(),
            closed: None,
            min_stay: None,
            max_stay: None,
            closed_to_arrival: None,
            closed_to_departure: None,
        }
    }

    /// Sets restrictions in their own transaction, committed if it succeeds.
    pub async fn try_restrict(&self, plan: &RatePlan, change: rates::RestrictionChange) -> Result<u64, RatesError> {
        let mut tx = self.tx().await;
        let changed = rates::set_restrictions(&mut tx, self.tenant, self.user, self.property, plan.id, &change).await?;
        tx.commit().await.unwrap();
        Ok(changed)
    }

    /// `plan`'s restrictions over the whole rate window, by date and room type.
    pub async fn restrictions(&self, plan: &RatePlan) -> Vec<rates::Restriction> {
        let window = (self.day(0), self.day(rooms::WINDOW_DAYS));
        rates::list_restrictions(&mut self.tx().await, self.property, plan.id, window.0, window.1).await.unwrap()
    }
}
