#![allow(dead_code)] // each test binary uses a different subset

use db::crypto::GuestIdKey;
use db::testing::{app_pool, guest_id_key};
use db::{Scope, TenantId, Tx, UserId, begin};
use rates::{
    CancellationRule, MealPlan, NewCancellationPolicy, NewMealSupplement, Penalty, PenaltyKind, RatePlan, Residency,
};
use reservations::{
    Account, AccountChanges, AccountContact, AccountKind, CreatedReservation, CreatedRoom, Guest, GuestChanges,
    IdDocType, NewAccount, NewGuest, NewReservation, NewReservationRoom, ReservationsError, Source,
};
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
            overbooking: 0,
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

/// A company account named `name`, active by default, no contact details, USD, no credit limit.
pub fn new_account(name: &str) -> NewAccount {
    NewAccount {
        kind: AccountKind::Company,
        name: name.into(),
        contact: AccountContact::default(),
        credit_limit: None,
        currency: "USD".into(),
    }
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
    /// Creates an account in its own transaction, committed if it succeeds.
    pub async fn try_account(&self, input: NewAccount) -> Result<Account, ReservationsError> {
        let mut tx = self.tx().await;
        let created = reservations::create_account(&mut tx, self.tenant, self.user, self.property, input).await?;
        tx.commit().await.unwrap();
        Ok(created)
    }

    pub async fn account(&self, input: NewAccount) -> Account {
        self.try_account(input).await.unwrap()
    }

    /// Changes an account in its own transaction, committed if it succeeds.
    pub async fn try_update_account(
        &self,
        account: &Account,
        changes: AccountChanges,
    ) -> Result<Account, ReservationsError> {
        let mut tx = self.tx().await;
        let updated = reservations::update_account(
            &mut tx,
            self.tenant,
            self.user,
            self.property,
            account.id,
            account.version,
            changes,
        )
        .await?;
        tx.commit().await.unwrap();
        Ok(updated)
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

/// Plans of [`Hotel::for_booking`].
pub struct Plans {
    /// USD, any guest, DLX and STD, RO or BB, with a cancellation policy.
    pub bar: RatePlan,
    /// USD, any guest, DLX only, without a cancellation policy.
    pub rack: RatePlan,
    /// USD, non-residents only, DLX only.
    pub fit_f: RatePlan,
}

impl Hotel {
    /// `deluxe` DLX rooms, one STD room, plans BAR, RACK and FIT-F priced at 10000 a night for 40 days, and a
    /// 1500-per-adult breakfast supplement in USD.
    pub async fn for_booking(opts: PgConnectOptions, deluxe: usize) -> (Self, Plans) {
        let hotel = Hotel::new(opts).await;
        let numbers: Vec<String> = (1..=deluxe).map(|n| format!("{}", 100 + n)).collect();
        hotel.rooms(hotel.deluxe.id, &numbers.iter().map(String::as_str).collect::<Vec<_>>()).await;
        hotel.rooms(hotel.standard.id, &["201"]).await;

        let mut tx = hotel.tx().await;
        let policy = NewCancellationPolicy {
            name: "Flexible".into(),
            rules: vec![CancellationRule {
                days_before_arrival: 2,
                penalty: Penalty { kind: PenaltyKind::Nights, value: 1 },
            }],
            no_show: Penalty { kind: PenaltyKind::Percent, value: 10_000 },
        };
        let policy =
            rates::create_cancellation_policy(&mut tx, hotel.tenant, hotel.user, hotel.property, policy).await.unwrap();
        let breakfast = NewMealSupplement {
            meal_plan: MealPlan::Bb,
            currency: "USD".into(),
            adult_amount: 1_500,
            child_amount: 500,
            from: hotel.day(0),
            to: None,
        };
        rates::create_meal_supplement(&mut tx, hotel.tenant, hotel.user, hotel.property, breakfast).await.unwrap();
        tx.commit().await.unwrap();

        let bar = rates::NewRatePlan {
            cancellation_policy_id: Some(policy.id),
            ..hotel.rate_plan("BAR", "USD", &[hotel.deluxe.id, hotel.standard.id])
        };
        let fit_f = rates::NewRatePlan {
            residency: Some(Residency::NonResident),
            ..hotel.rate_plan("FIT-F", "USD", &[hotel.deluxe.id])
        };
        let plans = Plans {
            bar: hotel.priced_plan(bar, 0, 40, 10_000).await,
            rack: hotel.priced_plan(hotel.rate_plan("RACK", "USD", &[hotel.deluxe.id]), 0, 40, 10_000).await,
            fit_f: hotel.priced_plan(fit_f, 0, 40, 10_000).await,
        };
        (hotel, plans)
    }

    /// Two adults in a `room_type` room on `plan`, room only, for `[business date + from, business date + to)`.
    pub fn room(&self, room_type: Uuid, plan: &RatePlan, from: i64, to: i64) -> NewReservationRoom {
        NewReservationRoom {
            room_type_id: room_type,
            rate_plan_id: plan.id,
            meal_plan: MealPlan::Ro,
            check_in: self.day(from),
            check_out: self.day(to),
            adults: 2,
            children: 0,
            primary_guest_id: None,
        }
    }

    /// Books `rooms` for `booker` in its own transaction, committed if it succeeds.
    pub async fn try_book(
        &self,
        booker: &Guest,
        rooms: Vec<NewReservationRoom>,
    ) -> Result<CreatedReservation, ReservationsError> {
        self.try_book_for_account(booker, None, rooms).await
    }

    /// `try_book`, billed to `account` (`None` bills the guest, as `try_book` does).
    pub async fn try_book_for_account(
        &self,
        booker: &Guest,
        account: Option<Uuid>,
        rooms: Vec<NewReservationRoom>,
    ) -> Result<CreatedReservation, ReservationsError> {
        let mut tx = self.tx().await;
        let input = NewReservation {
            booker_guest_id: booker.id,
            source: Source::Phone,
            notes: String::new(),
            account_id: account,
            rooms,
        };
        let created = reservations::create_reservation(&mut tx, self.tenant, self.user, self.property, input).await?;
        tx.commit().await.unwrap();
        Ok(created)
    }

    /// `try_book`, then takes every room out of the room booking auto-assigned it, for tests whose point is an
    /// unassigned stay. The result carries no room and the versions after the unassigns.
    pub async fn try_book_unassigned(
        &self,
        booker: &Guest,
        rooms: Vec<NewReservationRoom>,
    ) -> Result<CreatedReservation, ReservationsError> {
        let mut created = self.try_book(booker, rooms).await?;
        for stay in &mut created.rooms {
            if stay.room_id.is_some() {
                stay.version = self.unassigned(stay).await;
                created.version += 1; // each unassign bumps the reservation too
                stay.room_id = None;
                stay.room_number = None;
            }
        }
        Ok(created)
    }

    /// Unassigns `stay` in its own transaction, committed, and returns its new version.
    pub async fn unassigned(&self, stay: &CreatedRoom) -> i32 {
        let mut tx = self.tx().await;
        let freed = reservations::unassign_room(&mut tx, self.tenant, self.user, self.property, stay.id, stay.version)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        freed.version
    }

    /// `sold` for `room_type` on each day in `[business date + from, business date + to)`.
    pub async fn sold(&self, room_type: Uuid, from: i64, to: i64) -> Vec<i32> {
        let days =
            rooms::list_inventory(&mut self.tx().await, self.property, self.day(from), self.day(to)).await.unwrap();
        days.into_iter().filter(|day| day.room_type_id == room_type).map(|day| day.sold).collect()
    }

    /// Every confirmation number of the property, in order.
    pub async fn confirmation_numbers(&self) -> Vec<String> {
        sqlx::query_scalar("select confirmation_no from reservation where property_id = $1 order by confirmation_no")
            .bind(self.property)
            .fetch_all(&mut *self.tx().await)
            .await
            .unwrap()
    }

    pub async fn drift(&self) -> Vec<rooms::InventoryDrift> {
        rooms::find_drift(&mut self.tx().await, self.property).await.unwrap()
    }

    /// Sets `room_type`'s overbooking allowance, in its own transaction.
    pub async fn set_overbooking(&self, room_type: &RoomType, allowance: i32) -> RoomType {
        let mut tx = self.tx().await;
        let changes = rooms::RoomTypeChanges { overbooking: Some(allowance), ..Default::default() };
        let updated = rooms::update_room_type(
            &mut tx,
            self.tenant,
            self.user,
            self.property,
            room_type.id,
            room_type.version,
            changes,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        updated
    }

    /// Availability's `free` for `room_type` over `[business date + from, business date + to)`, non-resident.
    pub async fn free_for(&self, room_type: Uuid, from: i64, to: i64) -> i32 {
        let request = reservations::AvailabilityRequest {
            check_in: self.day(from),
            check_out: self.day(to),
            adults: 1,
            children: 0,
            residency: Residency::NonResident,
        };
        let found = reservations::availability(&mut self.tx().await, self.property, &request).await.unwrap();
        found.into_iter().find(|found| found.room_type_id == room_type).expect("room type is active").free
    }
}
