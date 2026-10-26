#![allow(dead_code)] // each test binary uses a different subset

use audit::{BreakdownRow, DailyStats};
use db::testing::app_pool;
use db::{Scope, TenantId, Tx, UserId, begin};
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use std::collections::BTreeMap;
use time::{Date, Month};
use uuid::Uuid;

/// A tenant with one user, one LKR property and one room type, whose business date is set by the test.
pub struct Hotel {
    pub pool: PgPool,
    pub tenant: TenantId,
    pub user: UserId,
    pub property: Uuid,
    pub room_type: Uuid,
}

pub fn date(year: i32, month: u8, day: u8) -> Date {
    Date::from_calendar_date(year, Month::try_from(month).unwrap(), day).unwrap()
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
        let room_type = rooms::create_room_type(&mut tx, tenant, user, property.id, room_type).await.unwrap().id;
        tx.commit().await.unwrap();
        Self { pool, tenant, user, property: property.id, room_type }
    }

    pub async fn tx(&self) -> Tx {
        begin(&self.pool, Scope::tenant(self.tenant)).await.unwrap()
    }

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

    /// Stores a closed day of `rooms_sold` of `rooms_available` rooms with `room_revenue`, and the breakdown rows.
    pub async fn close_day(
        &self,
        business_date: Date,
        rooms_available: i32,
        rooms_sold: i32,
        room_revenue: i64,
        breakdown: Vec<BreakdownRow>,
    ) {
        let stats = DailyStats {
            business_date,
            rooms_total: rooms_available,
            rooms_out_of_order: 0,
            rooms_available,
            rooms_sold,
            complimentary_rooms: 0,
            occupancy_bp: 0,
            adults: 0,
            children: 0,
            arrivals: 0,
            departures: 0,
            no_shows: 0,
            cancellations: 0,
            walk_ins: 0,
            room_revenue,
            meal_plan_revenue: 0,
            pos_revenue: 0,
            fee_revenue: 0,
            other_revenue: 0,
            adr: 0,
            revpar: 0,
            taxes: BTreeMap::new(),
            payments: BTreeMap::new(),
            breakdown,
        };
        let mut tx = self.tx().await;
        audit::store(&mut tx, self.tenant, self.property, &stats).await.unwrap();
        tx.commit().await.unwrap();
    }

    /// Closes every day from `from` to `to` with the same figures.
    pub async fn close_days(&self, from: Date, to: Date, rooms_sold: i32, room_revenue: i64) {
        for date in from.iter_to(to) {
            self.close_day(date, 10, rooms_sold, room_revenue, vec![]).await;
        }
    }

    /// A stay of `[arrival, departure)` in `status` for `first_name last_name`, in `room` when given.
    pub async fn stay(
        &self,
        status: &str,
        arrival: Date,
        departure: Date,
        room: Option<&str>,
        first_name: &str,
        last_name: &str,
    ) -> Uuid {
        let mut tx = self.tx().await;
        let n: i64 = sqlx::query_scalar("select count(*) + 1 from reservation").fetch_one(&mut *tx).await.unwrap();
        let room_id = match room {
            Some(number) => {
                let id = Uuid::now_v7();
                sqlx::query(
                    "insert into room (id, tenant_id, property_id, room_type_id, number) values ($1, $2, $3, $4, $5)",
                )
                .bind(id)
                .bind(self.tenant.0)
                .bind(self.property)
                .bind(self.room_type)
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
        .bind(first_name)
        .bind(last_name)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "insert into rate_plan (id, tenant_id, property_id, code, name, kind, segment, residency, currency,
                                    allowed_meal_plans)
             values ($1, $2, $3, 'P' || $4, 'Plan', 'standard', 'IBE', null, 'LKR', '{RO,BB}')",
        )
        .bind(plan)
        .bind(self.tenant.0)
        .bind(self.property)
        .bind(n.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "insert into reservation (id, tenant_id, property_id, confirmation_no, source, booker_guest_id,
                                      booked_business_date)
             values ($1, $2, $3, $4, 'front_desk', $5, $6)",
        )
        .bind(reservation)
        .bind(self.tenant.0)
        .bind(self.property)
        .bind(format!("GAL-{n:06}"))
        .bind(guest)
        .bind(arrival)
        .execute(&mut *tx)
        .await
        .unwrap();
        let checked_in = matches!(status, "checked_in" | "checked_out");
        sqlx::query(
            "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, room_id, stay,
                                           adults, children, rate_plan_id, meal_plan, status, primary_guest_id,
                                           currency, checked_in_at, checked_in_business_date, checked_out_at, cancelled_at,
                                           cancellation_penalty, cancelled_business_date)
             values ($1, $2, $3, $4, $5, $6, daterange($7, $8), 2, 1, $9, 'RO', $10, $11, 'LKR',
                     case when $12 then now() end, case when $12 then $7::date end,
                     case when $10 = 'checked_out' then now() end,
                     case when $10 = 'cancelled' then now() end, case when $10 = 'cancelled' then 0 end,
                     case when $10 = 'cancelled' then $7::date end)",
        )
        .bind(id)
        .bind(self.tenant.0)
        .bind(self.property)
        .bind(reservation)
        .bind(self.room_type)
        .bind(room_id)
        .bind(arrival)
        .bind(departure)
        .bind(plan)
        .bind(status)
        .bind(guest)
        .bind(checked_in)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        id
    }

    /// A guest folio on a new checked-out stay.
    pub async fn folio(&self) -> Uuid {
        let stay = self.stay("checked_out", date(2026, 4, 1), date(2026, 4, 2), None, "Ann", "Perera").await;
        let id = Uuid::now_v7();
        let mut tx = self.tx().await;
        sqlx::query(
            "insert into folio (id, tenant_id, property_id, kind, reservation_room_id) values ($1, $2, $3, 'guest', $4)",
        )
        .bind(id)
        .bind(self.tenant.0)
        .bind(self.property)
        .bind(stay)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        id
    }

    /// An LKR line of `net` on `folio`, dated `business_date`, from `source`, reversing `reverses` when given, with
    /// its `(code, kind, presentation, amount)` taxes: the on-bill ones add to its total.
    #[allow(clippy::too_many_arguments)]
    pub async fn line(
        &self,
        folio: Uuid,
        business_date: Date,
        source: &str,
        category: &str,
        net: i64,
        taxes: &[(&str, &str, &str, i64)],
        reverses: Option<Uuid>,
    ) -> Uuid {
        let id = Uuid::now_v7();
        let total = net + taxes.iter().filter(|tax| tax.2 == "on_bill").map(|tax| tax.3).sum::<i64>();
        let mut tx = self.tx().await;
        sqlx::query(
            "insert into folio_line (id, tenant_id, folio_id, source, category, description, net, total, currency,
                                     base_total, rate, rate_date, business_date, reverses)
             values ($1, $2, $3, $4, $5, 'Line', $6, $7, 'LKR', $7, 1, $8, $8, $9)",
        )
        .bind(id)
        .bind(self.tenant.0)
        .bind(folio)
        .bind(source)
        .bind(category)
        .bind(net)
        .bind(total)
        .bind(business_date)
        .bind(reverses)
        .execute(&mut *tx)
        .await
        .unwrap();
        for &(code, kind, presentation, amount) in taxes {
            sqlx::query(
                "insert into folio_line_tax (tenant_id, line_id, code, name, kind, rate_bp, base, amount, presentation)
                 values ($1, $2, $3, $3, $4, 0, $5, $6, $7)",
            )
            .bind(self.tenant.0)
            .bind(id)
            .bind(code)
            .bind(kind)
            .bind(net)
            .bind(amount)
            .bind(presentation)
            .execute(&mut *tx)
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();
        id
    }

    /// An outlet `code` whose sales fall under `group`.
    pub async fn outlet(&self, code: &str, group: folio::ReportGroup) -> Uuid {
        let mut tx = self.tx().await;
        let input =
            folio::NewOutlet { code: code.into(), name: code.into(), service_charge: false, report_group: group };
        let outlet = folio::create_outlet(&mut tx, self.tenant, Some(self.user), self.property, input).await.unwrap();
        tx.commit().await.unwrap();
        outlet.id
    }

    /// A one-item LKR bill of `unit_net` at `outlet`, paid there in cash on the property's business date.
    pub async fn settled_bill(&self, outlet: &str, bill_no: &str, unit_net: i64) {
        let mut tx = self.tx().await;
        let bill = folio::PosBillIn {
            outlet: outlet.into(),
            bill_no: bill_no.into(),
            business_date: None,
            currency: "LKR".into(),
            items: vec![folio::PosItem { description: "Item".into(), quantity: 1, unit_net, category: None }],
            target: folio::PosTarget::Settled { method: folio::Method::Cash },
        };
        folio::post_pos_bill(&mut tx, self.tenant, Some(self.user), self.property, bill).await.unwrap();
        tx.commit().await.unwrap();
    }
}
