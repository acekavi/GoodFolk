#![allow(dead_code)] // each test binary uses a different subset

use db::crypto::GuestIdKey;
use db::testing::{app_pool, guest_id_key};
use db::{Scope, TenantId, Tx, UserId, begin};
use rates::Residency;
use reservations::{Guest, GuestChanges, IdDocType, NewGuest, ReservationsError};
use rooms::{NewRoomType, RoomType};
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use time::{Date, Duration};
use uuid::Uuid;

/// A tenant with one user, one property and two room types: `DLX` (up to 2 adults and 1 child) and `STD`
/// (up to 2 adults), and the test guest ID key.
pub struct Hotel {
    pub pool: PgPool,
    pub tenant: TenantId,
    pub user: UserId,
    pub property: Uuid,
    pub business_date: Date,
    pub deluxe: RoomType,
    pub standard: RoomType,
    pub key: GuestIdKey,
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
        Self {
            pool,
            tenant,
            user,
            property: property.id,
            business_date: property.business_date,
            deluxe,
            standard,
            key: guest_id_key(),
        }
    }

    pub async fn tx(&self) -> Tx {
        begin(&self.pool, Scope::tenant(self.tenant)).await.unwrap()
    }

    /// The business date plus `days`.
    pub fn day(&self, days: i64) -> Date {
        self.business_date + Duration::days(days)
    }
}

/// A non-resident guest with only a name.
pub fn new_guest(first_name: &str, last_name: &str) -> NewGuest {
    NewGuest {
        first_name: first_name.into(),
        last_name: last_name.into(),
        email: None,
        phone: None,
        country: None,
        residency: Residency::NonResident,
        notes: String::new(),
        id_doc: None,
    }
}

/// `new_guest` with a passport numbered `number`.
pub fn with_passport(guest: NewGuest, number: &str) -> NewGuest {
    NewGuest { id_doc: Some((IdDocType::Passport, number.into())), ..guest }
}

impl Hotel {
    /// Creates a guest in its own transaction, committed if it succeeds.
    pub async fn try_guest(&self, input: NewGuest) -> Result<Guest, ReservationsError> {
        let mut tx = self.tx().await;
        let created = reservations::create_guest(&mut tx, self.tenant, self.user, &self.key, input).await?;
        tx.commit().await.unwrap();
        Ok(created)
    }

    pub async fn guest(&self, input: NewGuest) -> Guest {
        self.try_guest(input).await.unwrap()
    }

    /// Changes a guest in its own transaction, committed if it succeeds.
    pub async fn try_update_guest(&self, guest: &Guest, changes: GuestChanges) -> Result<Guest, ReservationsError> {
        let mut tx = self.tx().await;
        let updated =
            reservations::update_guest(&mut tx, self.tenant, self.user, &self.key, guest.id, guest.version, changes)
                .await?;
        tx.commit().await.unwrap();
        Ok(updated)
    }

    pub async fn search(&self, text: &str) -> Vec<Guest> {
        reservations::search_guests(&mut self.tx().await, text, 20).await.unwrap()
    }
}

impl Hotel {
    /// Rooms numbered `numbers`, all of `room_type`.
    pub async fn rooms(&self, room_type: Uuid, numbers: &[&str]) -> Vec<rooms::Room> {
        let mut tx = self.tx().await;
        let mut created = Vec::new();
        for number in numbers {
            let room =
                rooms::NewRoom { room_type_id: room_type, number: (*number).into(), floor: None, section_id: None };
            created.push(rooms::create_room(&mut tx, self.tenant, self.user, self.property, room).await.unwrap());
        }
        tx.commit().await.unwrap();
        created
    }

    /// A standard plan in `currency` for any guest, selling `room_types` room only or with breakfast.
    pub fn rate_plan(&self, code: &str, currency: &str, room_types: &[Uuid]) -> rates::NewRatePlan {
        rates::NewRatePlan {
            code: code.into(),
            name: format!("Plan {code}"),
            kind: rates::PlanKind::Standard,
            segment: rates::Segment::Ibe,
            residency: None,
            currency: currency.into(),
            parent_id: None,
            derive_mode: None,
            derive_value: None,
            rounding_step: 1,
            extra_adult_amount: 0,
            inherit_restrictions: false,
            allowed_meal_plans: vec![rates::MealPlan::Ro, rates::MealPlan::Bb],
            cancellation_policy_id: None,
            room_type_ids: room_types.to_vec(),
        }
    }

    /// Creates `input` and prices every room type it sells at `amount` for 2 adults on each day in `[from, to)`.
    pub async fn priced_plan(&self, input: rates::NewRatePlan, from: i64, to: i64, amount: i64) -> rates::RatePlan {
        let mut tx = self.tx().await;
        let plan = rates::create_rate_plan(&mut tx, self.tenant, self.user, self.property, input).await.unwrap();
        let prices: Vec<rates::Price> = plan
            .room_type_ids
            .iter()
            .flat_map(|room_type| {
                (from..to).map(|day| rates::Price {
                    room_type_id: *room_type,
                    date: self.day(day),
                    occupancy: 2,
                    amount,
                })
            })
            .collect();
        rates::set_prices(&mut tx, self.tenant, self.user, self.property, plan.id, &prices).await.unwrap();
        tx.commit().await.unwrap();
        plan
    }
}
