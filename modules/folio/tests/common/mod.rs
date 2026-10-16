#![allow(dead_code)] // each test binary uses a different subset

use db::testing::app_pool;
use db::{Scope, TenantId, Tx, UserId, begin};
use folio::{FolioError, LineRow, NewCharge, Source};
use rust_decimal::Decimal;
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use time::{Date, Duration};
use uuid::Uuid;

/// A tenant with one user and one LKR property (`GAL`) with the Sri Lanka preset, accepting LKR, USD, EUR and GBP.
pub struct Hotel {
    pub pool: PgPool,
    pub tenant: TenantId,
    pub user: UserId,
    pub property: Uuid,
    pub business_date: Date,
}

/// A reservation written straight into the reservation tables (this crate cannot use the reservations module), with
/// its folios opened as `reservations::create_reservation` opens them.
pub struct Booking {
    pub id: Uuid,
    pub rooms: Vec<Uuid>,
    pub deposit: Uuid,
    /// The guest folio of each of `rooms`, in order.
    pub guests: Vec<Uuid>,
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
        tx.commit().await.unwrap();
        let mut hotel = Self { pool, tenant, user, property: Uuid::nil(), business_date: Date::MIN };
        hotel.property = hotel.add_property("GAL").await;
        hotel.business_date = sqlx::query_scalar("select business_date from property where id = $1")
            .bind(hotel.property)
            .fetch_one(&mut *hotel.tx().await)
            .await
            .unwrap();
        hotel
    }

    pub async fn tx(&self) -> Tx {
        begin(&self.pool, Scope::tenant(self.tenant)).await.unwrap()
    }

    /// Another LKR property of this tenant.
    pub async fn add_property(&self, code: &str) -> Uuid {
        let mut tx = self.tx().await;
        let input = property::NewProperty {
            code: code.into(),
            name: format!("Hotel {code}"),
            timezone: "Asia/Colombo".into(),
            base_currency: "LKR".into(),
        };
        let property = property::create_property(&mut tx, self.tenant, self.user, input).await.unwrap();
        tx.commit().await.unwrap();
        property.id
    }

    pub async fn set_vat_registered(&self, registered: bool) {
        let mut tx = self.tx().await;
        sqlx::query("update property set vat_registered = $2 where id = $1")
            .bind(self.property)
            .bind(registered)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    /// A company account of the tenant.
    pub async fn account(&self) -> Uuid {
        let id = Uuid::now_v7();
        let mut tx = self.tx().await;
        sqlx::query(
            "insert into account (id, tenant_id, kind, name, currency) values ($1, $2, 'company', 'Acme', 'LKR')",
        )
        .bind(id)
        .bind(self.tenant.0)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        id
    }

    /// A manual rate for `currency` on the business date.
    pub async fn rate(&self, currency: &str, rate: Decimal) {
        let mut tx = self.tx().await;
        fx::set_manual(&mut tx, self.tenant, self.user, self.property, self.business_date, currency, rate)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    /// A confirmed reservation in `property` of `rooms` stays on a plan sold to `residency` (anyone when `None`),
    /// billed to `account`, with its deposit folio and each stay's guest folio.
    pub async fn book_in(
        &self,
        property: Uuid,
        rooms: usize,
        account: Option<Uuid>,
        residency: Option<&str>,
    ) -> Booking {
        let mut tx = self.tx().await;
        let taken: i64 = sqlx::query_scalar("select count(*) from reservation").fetch_one(&mut *tx).await.unwrap();
        let n = taken + 1;
        let (guest, room_type, plan, reservation) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        sqlx::query("insert into guest (id, tenant_id, last_name, residency) values ($1, $2, 'Silva', 'resident')")
            .bind(guest)
            .bind(self.tenant.0)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query(
            "insert into room_type (id, tenant_id, property_id, code, name, base_occupancy, max_adults, max_children,
                                    max_occupancy)
             values ($1, $2, $3, 'T' || $4, 'Type', 2, 2, 0, 2)",
        )
        .bind(room_type)
        .bind(self.tenant.0)
        .bind(property)
        .bind(n.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "insert into rate_plan (id, tenant_id, property_id, code, name, kind, segment, residency, currency)
             values ($1, $2, $3, 'P' || $4, 'Plan', 'standard', 'IBE', $5, 'LKR')",
        )
        .bind(plan)
        .bind(self.tenant.0)
        .bind(property)
        .bind(n.to_string())
        .bind(residency)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "insert into reservation (id, tenant_id, property_id, confirmation_no, source, booker_guest_id, account_id)
             values ($1, $2, $3, $4, 'front_desk', $5, $6)",
        )
        .bind(reservation)
        .bind(self.tenant.0)
        .bind(property)
        .bind(format!("RES-{n:06}"))
        .bind(guest)
        .bind(account)
        .execute(&mut *tx)
        .await
        .unwrap();
        let deposit = folio::ensure_deposit_folio(&mut tx, self.tenant, property, reservation).await.unwrap();
        let (mut room_ids, mut guests) = (Vec::new(), Vec::new());
        for _ in 0..rooms {
            let room = Uuid::now_v7();
            sqlx::query(
                "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, stay, adults,
                                               children, rate_plan_id, meal_plan, status, primary_guest_id, currency)
                 values ($1, $2, $3, $4, $5, daterange($6, $7), 2, 0, $8, 'RO', 'confirmed', $9, 'LKR')",
            )
            .bind(room)
            .bind(self.tenant.0)
            .bind(property)
            .bind(reservation)
            .bind(room_type)
            .bind(self.business_date)
            .bind(self.business_date + Duration::days(2))
            .bind(plan)
            .bind(guest)
            .execute(&mut *tx)
            .await
            .unwrap();
            guests.push(folio::ensure_guest_folio(&mut tx, self.tenant, property, room).await.unwrap());
            room_ids.push(room);
        }
        tx.commit().await.unwrap();
        Booking { id: reservation, rooms: room_ids, deposit, guests }
    }

    /// A one-stay reservation in the hotel's property on a plan sold to anyone.
    pub async fn book(&self, account: Option<Uuid>) -> Booking {
        self.book_in(self.property, 1, account, None).await
    }

    /// A charge of one `unit_net` in `currency`, in `category`, on the business date.
    pub fn charge(&self, source: Source, category: &str, unit_net: i64, currency: &str) -> NewCharge {
        NewCharge {
            source,
            category: category.into(),
            description: "Charge".into(),
            quantity: 1,
            unit_net,
            currency: currency.into(),
            business_date: self.business_date,
            posting_key: None,
            inclusive: false,
        }
    }

    /// `post_charge` in its own transaction, committed if it succeeds.
    pub async fn post(&self, folio: Uuid, charge: NewCharge) -> Result<LineRow, FolioError> {
        let mut tx = self.tx().await;
        let line = folio::post_charge(&mut tx, self.tenant, Some(self.user), folio, charge).await?;
        tx.commit().await.unwrap();
        Ok(line)
    }

    /// `post_for_stay` in its own transaction, committed if it succeeds.
    pub async fn post_for_stay(&self, stay: Uuid, charge: NewCharge) -> Result<LineRow, FolioError> {
        let mut tx = self.tx().await;
        let line = folio::post_for_stay(&mut tx, self.tenant, None, stay, charge).await?;
        tx.commit().await.unwrap();
        Ok(line)
    }

    /// `set_routing` in its own transaction, committed if it succeeds.
    pub async fn route(&self, reservation: Uuid, categories: &[&str]) -> Result<(), FolioError> {
        let categories: Vec<String> = categories.iter().map(|category| (*category).to_owned()).collect();
        let mut tx = self.tx().await;
        folio::set_routing(&mut tx, self.tenant, Some(self.user), reservation, &categories).await?;
        tx.commit().await.unwrap();
        Ok(())
    }

    /// `(folio, total, posting key)` of every line of the folios `ids`, oldest first.
    pub async fn lines_on(&self, ids: &[Uuid]) -> Vec<(Uuid, i64, Option<String>)> {
        sqlx::query_as("select folio_id, total, posting_key from folio_line where folio_id = any($1) order by id")
            .bind(ids)
            .fetch_all(&mut *self.tx().await)
            .await
            .unwrap()
    }

    /// `(entity_id, data)` of the audit entries named `action`, oldest first.
    pub async fn audited(&self, action: &str) -> Vec<(Uuid, serde_json::Value)> {
        sqlx::query_as("select entity_id, data from audit_log where action = $1 order by id")
            .bind(action)
            .fetch_all(&mut *self.tx().await)
            .await
            .unwrap()
    }
}
