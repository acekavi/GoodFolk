#![allow(dead_code)] // each test binary uses a different subset

use db::testing::app_pool;
use db::{Scope, TenantId, Tx, UserId, begin};
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use uuid::Uuid;

/// A tenant with one user, one property and one room type.
pub struct Hotel {
    pub pool: PgPool,
    pub tenant: TenantId,
    pub user: UserId,
    pub property: Uuid,
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
        let property = property::create_property(
            &mut tx,
            tenant,
            user,
            property::NewProperty {
                code: "GAL".into(),
                name: "Galle".into(),
                timezone: "Asia/Colombo".into(),
                base_currency: "LKR".into(),
            },
        )
        .await
        .unwrap();
        let room_type = Self::room_type_in(&mut tx, tenant, user, property.id, "DLX").await;
        tx.commit().await.unwrap();
        Self { pool, tenant, user, property: property.id, room_type }
    }

    async fn room_type_in(tx: &mut Tx, tenant: TenantId, user: UserId, property: Uuid, code: &str) -> Uuid {
        let room_type = rooms::NewRoomType {
            code: code.into(),
            name: "Deluxe".into(),
            base_occupancy: 2,
            max_adults: 2,
            max_children: 1,
            max_occupancy: 3,
            overbooking: 0,
            bed_config: vec![],
            amenities: vec![],
        };
        rooms::create_room_type(tx, tenant, user, property, room_type).await.unwrap().id
    }

    pub async fn tx(&self) -> Tx {
        begin(&self.pool, Scope::tenant(self.tenant)).await.unwrap()
    }

    pub async fn section(&self, name: &str) -> Uuid {
        let mut tx = self.tx().await;
        let section = rooms::create_section(&mut tx, self.tenant, self.user, self.property, name).await.unwrap();
        tx.commit().await.unwrap();
        section.id
    }

    /// A second tenant-wide property with its own room type, for cross-property checks.
    pub async fn other_property(&self) -> (Uuid, Uuid) {
        let mut tx = self.tx().await;
        let property = property::create_property(
            &mut tx,
            self.tenant,
            self.user,
            property::NewProperty {
                code: "KAN".into(),
                name: "Kandy".into(),
                timezone: "Asia/Colombo".into(),
                base_currency: "LKR".into(),
            },
        )
        .await
        .unwrap();
        let room_type = Self::room_type_in(&mut tx, self.tenant, self.user, property.id, "STD").await;
        tx.commit().await.unwrap();
        (property.id, room_type)
    }

    pub async fn audited(&self, action: &str) -> i64 {
        sqlx::query_scalar("select count(*) from audit_log where action = $1")
            .bind(action)
            .fetch_one(&mut *self.tx().await)
            .await
            .unwrap()
    }

    /// The data of every `action` audit entry, oldest first.
    pub async fn audit_data(&self, action: &str) -> Vec<serde_json::Value> {
        sqlx::query_scalar("select data from audit_log where action = $1 order by at, id")
            .bind(action)
            .fetch_all(&mut *self.tx().await)
            .await
            .unwrap()
    }
}
