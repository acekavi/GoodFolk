#![allow(dead_code)] // each test binary uses a different subset

use db::testing::app_pool;
use db::{Scope, TenantId, Tx, UserId, begin};
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use time::Date;
use uuid::Uuid;

/// A tenant with one user (the supervisor of its tests), one property and one room type.
pub struct Hotel {
    pub pool: PgPool,
    pub tenant: TenantId,
    pub user: UserId,
    pub property: Uuid,
    pub business_date: Date,
    pub room_type: Uuid,
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
        let room_type = rooms::NewRoomType {
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
        let room_type = rooms::create_room_type(&mut tx, tenant, user, property.id, room_type).await.unwrap();
        tx.commit().await.unwrap();
        Self {
            pool,
            tenant,
            user,
            property: property.id,
            business_date: property.business_date,
            room_type: room_type.id,
        }
    }

    pub async fn tx(&self) -> Tx {
        begin(&self.pool, Scope::tenant(self.tenant)).await.unwrap()
    }

    /// Rooms numbered `numbers`, each created with its (clean) condition.
    pub async fn rooms(&self, numbers: &[&str]) -> Vec<Uuid> {
        let mut tx = self.tx().await;
        let mut ids = Vec::new();
        for number in numbers {
            let room = rooms::NewRoom {
                room_type_id: self.room_type,
                number: (*number).into(),
                floor: None,
                section_id: None,
            };
            ids.push(rooms::create_room(&mut tx, self.tenant, self.user, self.property, room).await.unwrap().id);
        }
        tx.commit().await.unwrap();
        ids
    }

    /// A new member of the tenant holding `role` (tenant-wide when `property` is `None`), or no role at all.
    pub async fn member(&self, role: Option<&str>, property: Option<Uuid>) -> UserId {
        let user = UserId(Uuid::now_v7());
        let mut tx = self.tx().await;
        sqlx::query("insert into app_user (id, email, password_hash, display_name) values ($1, $2, 'x', 'U')")
            .bind(user.0)
            .bind(format!("{}@example.com", user.0))
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("insert into membership (tenant_id, user_id) values ($1, $2)")
            .bind(self.tenant.0)
            .bind(user.0)
            .execute(&mut *tx)
            .await
            .unwrap();
        if let Some(role) = role {
            sqlx::query(
                "insert into role_grant (id, tenant_id, user_id, property_id, role) values ($1, $2, $3, $4, $5)",
            )
            .bind(Uuid::now_v7())
            .bind(self.tenant.0)
            .bind(user.0)
            .bind(property)
            .bind(role)
            .execute(&mut *tx)
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();
        user
    }

    /// Assigns `room` to `user` for the business date.
    pub async fn assign(&self, room: Uuid, user: UserId) {
        let mut tx = self.tx().await;
        sqlx::query("insert into hk_assignment (tenant_id, property_id, business_date, room_id, user_id) values ($1, $2, $3, $4, $5)")
            .bind(self.tenant.0)
            .bind(self.property)
            .bind(self.business_date)
            .bind(room)
            .bind(user.0)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    /// The room's stored condition and version.
    pub async fn condition(&self, room: Uuid) -> (String, i32) {
        sqlx::query_as("select condition, version from room_condition where room_id = $1")
            .bind(room)
            .fetch_one(&mut *self.tx().await)
            .await
            .unwrap()
    }

    /// How many `room.condition_changed` audit entries exist.
    pub async fn audit_count(&self) -> i64 {
        sqlx::query_scalar("select count(*) from audit_log where action = 'room.condition_changed'")
            .fetch_one(&mut *self.tx().await)
            .await
            .unwrap()
    }
}
