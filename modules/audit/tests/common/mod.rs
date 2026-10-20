#![allow(dead_code)] // each test binary uses a different subset

use db::testing::app_pool;
use db::{Scope, TenantId, Tx, UserId, begin};
use folio::{LineRow, Method, NewCharge, PaymentRow, PosBillIn, PosBillRow, PosItem, PosOutcome, PosTarget, Source};
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
    /// The property's business date D.
    pub business_date: Date,
}

/// A stay to write straight into the reservation tables, as `folio`'s tests seed theirs: one reservation of one room
/// on its own rate plan, with a night priced for every date of the stay, its deposit folio and its guest folio.
pub struct Stay<'a> {
    pub property: Uuid,
    pub status: &'a str,
    /// The stay is `[D + from, D + to)`.
    pub from: i64,
    pub to: i64,
    pub room: Option<&'a str>,
    pub currency: &'a str,
    /// The rate plan's segment; `FIT_F` plans are sold to non-residents.
    pub segment: &'a str,
    pub room_amount: i64,
    pub meal_amount: i64,
    /// The booked cancellation terms.
    pub terms: Option<serde_json::Value>,
    pub first_name: &'a str,
    pub last_name: &'a str,
}

impl<'a> Stay<'a> {
    /// A stay in `property` of `[D + from, D + to)` in `status`, unassigned, in LKR on an `IBE` plan at 10 000 a night
    /// with no meals and no terms, for Ada Silva.
    pub fn new(property: Uuid, status: &'a str, from: i64, to: i64) -> Self {
        Stay {
            property,
            status,
            from,
            to,
            room: None,
            currency: "LKR",
            segment: "IBE",
            room_amount: 10_000,
            meal_amount: 0,
            terms: None,
            first_name: "Ada",
            last_name: "Silva",
        }
    }
}

/// What [`Hotel::stay`] wrote.
pub struct Booked {
    /// The reservation room.
    pub id: Uuid,
    pub reservation: Uuid,
    pub confirmation: String,
    pub guest_folio: Uuid,
    pub deposit: Uuid,
}

impl Hotel {
    pub async fn new(opts: PgConnectOptions) -> Self {
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

    /// `D + days`.
    pub fn day(&self, days: i64) -> Date {
        self.business_date + Duration::days(days)
    }

    /// Another LKR property of this tenant, with a room type `STD`.
    pub async fn add_property(&self, code: &str) -> Uuid {
        let mut tx = self.tx().await;
        let input = property::NewProperty {
            code: code.into(),
            name: format!("Hotel {code}"),
            timezone: "Asia/Colombo".into(),
            base_currency: "LKR".into(),
        };
        let property = property::create_property(&mut tx, self.tenant, self.user, input).await.unwrap();
        sqlx::query(
            "insert into room_type (id, tenant_id, property_id, code, name, base_occupancy, max_adults, max_children,
                                    max_occupancy)
             values ($1, $2, $3, 'STD', 'Standard', 2, 2, 0, 2)",
        )
        .bind(Uuid::now_v7())
        .bind(self.tenant.0)
        .bind(property.id)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        property.id
    }

    /// A manual rate for `currency` in `property` on D.
    pub async fn rate(&self, property: Uuid, currency: &str, rate: Decimal) {
        let mut tx = self.tx().await;
        fx::set_manual(&mut tx, self.tenant, self.user, property, self.business_date, currency, rate).await.unwrap();
        tx.commit().await.unwrap();
    }

    /// Writes `stay` (see [`Stay`]), creating its room when it names one.
    pub async fn stay(&self, stay: Stay<'_>) -> Booked {
        let mut tx = self.tx().await;
        let n: i64 = sqlx::query_scalar("select count(*) + 1 from reservation").fetch_one(&mut *tx).await.unwrap();
        let room_type: Uuid = sqlx::query_scalar("select id from room_type where property_id = $1 and code = 'STD'")
            .bind(stay.property)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        let room = match stay.room {
            Some(number) => {
                let id = Uuid::now_v7();
                sqlx::query(
                    "insert into room (id, tenant_id, property_id, room_type_id, number) values ($1, $2, $3, $4, $5)",
                )
                .bind(id)
                .bind(self.tenant.0)
                .bind(stay.property)
                .bind(room_type)
                .bind(number)
                .execute(&mut *tx)
                .await
                .unwrap();
                Some(id)
            }
            None => None,
        };
        let (guest, plan, reservation, id) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        sqlx::query(
            "insert into guest (id, tenant_id, first_name, last_name, residency) values ($1, $2, $3, $4, 'resident')",
        )
        .bind(guest)
        .bind(self.tenant.0)
        .bind(stay.first_name)
        .bind(stay.last_name)
        .execute(&mut *tx)
        .await
        .unwrap();
        let residency = (stay.segment == "FIT_F").then_some("non_resident");
        sqlx::query(
            "insert into rate_plan (id, tenant_id, property_id, code, name, kind, segment, residency, currency,
                                    allowed_meal_plans)
             values ($1, $2, $3, 'P' || $4, 'Plan', 'standard', $5, $6, $7, '{RO,BB}')",
        )
        .bind(plan)
        .bind(self.tenant.0)
        .bind(stay.property)
        .bind(n.to_string())
        .bind(stay.segment)
        .bind(residency)
        .bind(stay.currency)
        .execute(&mut *tx)
        .await
        .unwrap();
        let confirmation = format!("GAL-{n:06}");
        sqlx::query(
            "insert into reservation (id, tenant_id, property_id, confirmation_no, source, booker_guest_id,
                                      booked_business_date)
             values ($1, $2, $3, $4, 'front_desk', $5, $6)",
        )
        .bind(reservation)
        .bind(self.tenant.0)
        .bind(stay.property)
        .bind(&confirmation)
        .bind(guest)
        .bind(self.business_date)
        .execute(&mut *tx)
        .await
        .unwrap();
        let (arrival, departure) = (self.day(stay.from), self.day(stay.to));
        let checked_in = matches!(stay.status, "checked_in" | "checked_out");
        sqlx::query(
            "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, room_id, stay,
                                           adults, children, rate_plan_id, meal_plan, status, primary_guest_id,
                                           currency, cancellation_terms, checked_in_at, checked_in_business_date,
                                           checked_out_at)
             values ($1, $2, $3, $4, $5, $6, daterange($7, $8), 2, 0, $9, $10, $11, $12, $13, $14,
                     case when $15 then now() end, case when $15 then $7::date end,
                     case when $11 = 'checked_out' then now() end)",
        )
        .bind(id)
        .bind(self.tenant.0)
        .bind(stay.property)
        .bind(reservation)
        .bind(room_type)
        .bind(room)
        .bind(arrival)
        .bind(departure)
        .bind(plan)
        .bind(if stay.meal_amount > 0 { "BB" } else { "RO" })
        .bind(stay.status)
        .bind(guest)
        .bind(stay.currency)
        .bind(stay.terms)
        .bind(checked_in)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "insert into reservation_night (tenant_id, property_id, reservation_room_id, date, room_amount,
                                            meal_amount, currency)
             select $1, $2, $3, d::date, $6, $7, $8 from generate_series($4::date, $5::date - 1, interval '1 day') d",
        )
        .bind(self.tenant.0)
        .bind(stay.property)
        .bind(id)
        .bind(arrival)
        .bind(departure)
        .bind(stay.room_amount)
        .bind(stay.meal_amount)
        .bind(stay.currency)
        .execute(&mut *tx)
        .await
        .unwrap();
        let deposit = folio::ensure_deposit_folio(&mut tx, self.tenant, stay.property, reservation).await.unwrap();
        let guest_folio = folio::ensure_guest_folio(&mut tx, self.tenant, stay.property, id).await.unwrap();
        tx.commit().await.unwrap();
        Booked { id, reservation, confirmation, guest_folio, deposit }
    }

    /// Marks `stay` checked out.
    pub async fn check_out(&self, stay: Uuid) {
        let mut tx = self.tx().await;
        sqlx::query("update reservation_room set status = 'checked_out', checked_out_at = now() where id = $1")
            .bind(stay)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    /// An active outlet of `property` with its service charge on.
    pub async fn outlet(&self, property: Uuid, code: &str, name: &str) {
        let mut tx = self.tx().await;
        let input = folio::NewOutlet { code: code.into(), name: name.into(), service_charge: true };
        folio::create_outlet(&mut tx, self.tenant, Some(self.user), property, input).await.unwrap();
        tx.commit().await.unwrap();
    }

    /// Posts a one-item LKR bill of `unit_net` at `outlet`, to `target`, on D.
    pub async fn pos_bill(
        &self,
        property: Uuid,
        outlet: &str,
        bill_no: &str,
        unit_net: i64,
        target: PosTarget,
    ) -> PosBillRow {
        let mut tx = self.tx().await;
        let bill = PosBillIn {
            outlet: outlet.into(),
            bill_no: bill_no.into(),
            business_date: None,
            currency: "LKR".into(),
            items: vec![PosItem { description: "Item".into(), quantity: 1, unit_net, category: None }],
            target,
        };
        let outcome = folio::post_pos_bill(&mut tx, self.tenant, Some(self.user), property, bill).await.unwrap();
        tx.commit().await.unwrap();
        match outcome {
            PosOutcome::Posted(row) => row,
            PosOutcome::Replayed(_) => panic!("bill {bill_no} was already posted"),
        }
    }

    /// `(total, base total)` of the folio lines of a bill posted to a stay.
    pub async fn bill_lines(&self, bill: Uuid) -> (i64, i64) {
        sqlx::query_as(
            "select sum(total)::bigint, sum(base_total)::bigint from folio_line where starts_with(posting_key, $1)",
        )
        .bind(format!("pos:{bill}:"))
        .fetch_one(&mut *self.tx().await)
        .await
        .unwrap()
    }

    /// Takes a payment on `folio` of `property`, committed.
    pub async fn pay(
        &self,
        property: Uuid,
        folio: Uuid,
        method: Method,
        amount: i64,
        currency: &str,
        evidence: Option<&str>,
    ) -> PaymentRow {
        let mut tx = self.tx().await;
        let payment = folio::take_payment(
            &mut tx,
            self.tenant,
            Some(self.user),
            property,
            folio,
            method,
            amount,
            currency,
            evidence,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        payment
    }

    /// Refunds `amount` of `payment` from its own folio, committed.
    pub async fn refund(&self, property: Uuid, payment: Uuid, amount: i64) -> PaymentRow {
        let mut tx = self.tx().await;
        let refund = folio::refund(&mut tx, self.tenant, Some(self.user), property, payment, amount, "Overpaid", None)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        refund
    }

    /// A manual LKR charge of `unit_net` in `category` on `folio` of `property`, on D, committed.
    pub async fn charge(&self, property: Uuid, folio: Uuid, category: &str, unit_net: i64) -> LineRow {
        let charge = NewCharge {
            source: Source::Manual,
            category: category.into(),
            description: "Charge".into(),
            quantity: 1,
            unit_net,
            currency: "LKR".into(),
            business_date: self.business_date,
            posting_key: None,
            inclusive: false,
        };
        let mut tx = self.tx().await;
        let line = folio::post_charge(&mut tx, self.tenant, Some(self.user), property, folio, charge).await.unwrap();
        tx.commit().await.unwrap();
        line
    }

    /// `(posting key, net)` of every line of `property` with a posting key, by key.
    pub async fn keyed_lines(&self, property: Uuid) -> Vec<(String, i64)> {
        sqlx::query_as(
            "select l.posting_key, l.net from folio_line l join folio f on f.id = l.folio_id
             where f.property_id = $1 and l.posting_key is not null
             order by l.posting_key",
        )
        .bind(property)
        .fetch_all(&mut *self.tx().await)
        .await
        .unwrap()
    }

    /// Moves `property`'s business date to `D + days`.
    pub async fn move_business_date(&self, property: Uuid, days: i64) {
        let mut tx = self.tx().await;
        sqlx::query("update property set business_date = $2 where id = $1")
            .bind(property)
            .bind(self.day(days))
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    /// `audit::night_audit` of `property`.
    pub async fn night_audit(&self, property: Uuid) -> audit::NightAudit {
        audit::night_audit(&mut self.tx().await, property).await.unwrap()
    }
}
