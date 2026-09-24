#![allow(dead_code)] // each test binary uses a different subset

use db::testing::app_pool;
use db::{Scope, TenantId, Tx, UserId, begin};
use rooms::{InventoryDay, NewRoomType, RoomType, WINDOW_DAYS};
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

    /// Counter rows that disagree with rooms and blocks; empty when the counters are right.
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
        bed_config: vec![rooms::Bed { kind: "queen".into(), count: 1 }],
        amenities: vec!["Air conditioning".into()],
    }
}
