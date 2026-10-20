#![allow(dead_code)] // each test binary uses a different subset

use db::testing::app_pool;
use db::{Scope, TenantId, Tx, UserId, begin};
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use time::Date;
use time::macros::date;
use uuid::Uuid;

/// The business date a new hotel starts on.
pub const OPEN_FROM: Date = date!(2026 - 01 - 01);

/// A tenant with one user and one LKR property that accepts LKR, USD, EUR and GBP.
pub struct Hotel {
    pub pool: PgPool,
    pub tenant: TenantId,
    pub user: UserId,
    pub property: Uuid,
}

impl Hotel {
    pub async fn new(opts: PgConnectOptions) -> Self {
        let pool = app_pool(opts, 2).await;
        Self::in_pool(pool).await
    }

    /// Another tenant with its own property in the same database.
    pub async fn in_pool(pool: PgPool) -> Self {
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
        // Before every date the tests use, so none of them is a closed day.
        sqlx::query("update property set business_date = $2 where id = $1")
            .bind(property.id)
            .bind(OPEN_FROM)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        Self { pool, tenant, user, property: property.id }
    }

    /// Moves the property's business date to `date`, as the night audit's roll does.
    pub async fn set_business_date(&self, date: Date) {
        let mut tx = self.tx().await;
        sqlx::query("update property set business_date = $2 where id = $1")
            .bind(self.property)
            .bind(date)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    pub async fn tx(&self) -> Tx {
        begin(&self.pool, Scope::tenant(self.tenant)).await.unwrap()
    }

    pub async fn alert(&self) -> Option<String> {
        let mut tx = self.tx().await;
        sqlx::query_scalar("select message from fx_alert where property_id = $1")
            .bind(self.property)
            .fetch_optional(&mut *tx)
            .await
            .unwrap()
    }

    /// `(date, source_date, rate, source)` of the stored rates of `currency`, oldest first.
    pub async fn stored(&self, currency: &str) -> Vec<(time::Date, time::Date, rust_decimal::Decimal, String)> {
        let mut tx = self.tx().await;
        sqlx::query_as(
            "select date, source_date, rate_to_base, source from fx_rate where property_id = $1 and currency = $2
             order by date",
        )
        .bind(self.property)
        .bind(currency)
        .fetch_all(&mut *tx)
        .await
        .unwrap()
    }
}
