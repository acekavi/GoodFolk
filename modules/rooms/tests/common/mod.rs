#![allow(dead_code)] // each test binary uses a different subset

use db::testing::app_pool;
use db::{Scope, TenantId, Tx, UserId, begin};
use rooms::{BlockReason, InventoryDay, NewRoom, NewRoomType, Room, RoomType, WINDOW_DAYS};
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use time::{Date, Duration};
use uuid::Uuid;

/// A tenant with one user and one property, as the API's sign-up and create-property commands leave it.
pub struct Hotel {
    pub pool: PgPool,
    pub tenant: TenantId,
    pub user: UserId,
    pub property: Uuid,
    pub business_date: Date,
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
        rooms::seed_block_reasons(&mut tx, tenant, property.id).await.unwrap();
        tx.commit().await.unwrap();
        Self { pool, tenant, user, property: property.id, business_date: property.business_date }
    }

    pub async fn tx(&self) -> Tx {
        begin(&self.pool, Scope::tenant(self.tenant)).await.unwrap()
    }

    /// The business date plus `days`.
    pub fn day(&self, days: i64) -> Date {
        self.business_date + Duration::days(days)
    }

    pub async fn room(&self, room_type: Uuid, number: &str) -> Room {
        let input = NewRoom { room_type_id: room_type, number: number.into(), floor: None, section_id: None };
        let mut tx = self.tx().await;
        let room = rooms::create_room(&mut tx, self.tenant, self.user, self.property, input).await.unwrap();
        tx.commit().await.unwrap();
        room
    }

    /// The seeded block reason with `code`.
    pub async fn reason(&self, code: &str) -> BlockReason {
        let mut tx = self.tx().await;
        let reasons = rooms::list_block_reasons(&mut tx, self.property).await.unwrap();
        reasons.into_iter().find(|reason| reason.code == code).expect("a seeded reason")
    }

    pub async fn room_type(&self, code: &str) -> RoomType {
        let mut tx = self.tx().await;
        let created = rooms::create_room_type(&mut tx, self.tenant, self.user, self.property, room_type(code)).await;
        tx.commit().await.unwrap();
        created.unwrap()
    }

    /// `room_type`'s counters over the whole window, by date.
    pub async fn counters(&self, room_type: Uuid) -> Vec<InventoryDay> {
        let mut tx = self.tx().await;
        let days = rooms::list_inventory(&mut tx, self.property, self.day(0), self.day(WINDOW_DAYS)).await.unwrap();
        days.into_iter().filter(|day| day.room_type_id == room_type).collect()
    }

    /// A stay of `status` in `room` for `[business date + from, business date + to)`, written straight into the
    /// reservation tables as the reservations module leaves an assigned stay (this crate cannot depend on it),
    /// with its own guest, rate plan and reservation. The counters are not touched. Returns the confirmation
    /// number, `GAL-000001` for the first stay.
    pub async fn assigned_stay(&self, room: &Room, from: i64, to: i64, status: &str) -> String {
        let mut tx = self.tx().await;
        let taken: i64 = sqlx::query_scalar("select count(*) from reservation").fetch_one(&mut *tx).await.unwrap();
        let (guest, plan, reservation) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let confirmation = format!("GAL-{:06}", taken + 1);
        let statements = [
            "insert into guest (id, tenant_id, first_name, last_name, residency)
             values ($3, $1, 'Ada', 'Silva', 'resident')",
            "insert into rate_plan (id, tenant_id, property_id, code, name, kind, segment, currency)
             values ($4, $1, $2, 'BAR' || $6, 'Best available', 'standard', 'IBE', 'USD')",
            "insert into reservation (id, tenant_id, property_id, confirmation_no, source, booker_guest_id)
             values ($5, $1, $2, $7, 'front_desk', $3)",
            "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, room_id, stay,
                                           adults, children, rate_plan_id, meal_plan, status, primary_guest_id,
                                           currency, cancelled_at, cancellation_penalty, checked_in_at,
                                           checked_in_business_date, checked_out_at)
             select gen_random_uuid(), $1, $2, $5, r.room_type_id, r.id, daterange($9, $10), 2, 0, $4, 'RO', $11,
                    $3, 'USD', case when $11 = 'cancelled' then now() end, case when $11 = 'cancelled' then 0 end,
                    case when $11 in ('checked_in', 'checked_out') then now() end,
                    case when $11 in ('checked_in', 'checked_out') then $9 end,
                    case when $11 = 'checked_out' then now() end
             from room r where r.id = $8",
        ];
        for statement in statements {
            sqlx::query(statement)
                .bind(self.tenant.0)
                .bind(self.property)
                .bind(guest)
                .bind(plan)
                .bind(reservation)
                .bind(taken + 1)
                .bind(&confirmation)
                .bind(room.id)
                .bind(self.day(from))
                .bind(self.day(to))
                .bind(status)
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        tx.commit().await.unwrap();
        confirmation
    }

    /// Counter rows that disagree with rooms, blocks and reservations; empty when the counters are right.
    pub async fn drift(&self) -> Vec<rooms::InventoryDrift> {
        let mut tx = self.tx().await;
        rooms::find_drift(&mut tx, self.property).await.unwrap()
    }
}

/// A double room for two adults and one child.
pub fn room_type(code: &str) -> NewRoomType {
    NewRoomType {
        code: code.into(),
        name: format!("Room type {code}"),
        base_occupancy: 2,
        max_adults: 2,
        max_children: 1,
        max_occupancy: 3,
        overbooking: 0,
        bed_config: vec![rooms::Bed { kind: "queen".into(), count: 1 }],
        amenities: vec!["Air conditioning".into()],
    }
}
