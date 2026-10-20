#![allow(dead_code)] // each test binary uses a different subset

use db::testing::app_pool;
use db::{Scope, TenantId, Tx, UserId, begin};
use media::{Completed, DocumentKind, MediaError, NewUpload, Target, Verified, complete_upload, start_upload};
use media_types::{Format, MediaResult, RenditionOut, Size};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::{Date, Duration};
use uuid::Uuid;

/// A tenant with one user, one property, one room type and one room.
pub struct Hotel {
    pub pool: PgPool,
    /// Connects as the test superuser, for what `goodfolk_app` may not touch (the outbox, locks).
    pub owner: PgPool,
    pub tenant: TenantId,
    pub user: UserId,
    pub property: Uuid,
    pub business_date: Date,
    pub room_type: Uuid,
    pub room: Uuid,
}

impl Hotel {
    pub async fn new(opts: PgConnectOptions) -> Self {
        let pool = app_pool(opts.clone(), 4).await;
        let owner = PgPoolOptions::new().max_connections(2).connect_with(opts).await.unwrap();
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
        let mut hotel = Self {
            pool,
            owner,
            tenant,
            user,
            property: property.id,
            business_date: property.business_date,
            room_type: Uuid::nil(),
            room: Uuid::nil(),
        };
        hotel.room_type = hotel.add_room_type(hotel.property, "DLX").await;
        let mut tx = hotel.tx().await;
        let room =
            rooms::NewRoom { room_type_id: hotel.room_type, number: "101".into(), floor: None, section_id: None };
        hotel.room = rooms::create_room(&mut tx, tenant, user, hotel.property, room).await.unwrap().id;
        tx.commit().await.unwrap();
        hotel
    }

    pub async fn tx(&self) -> Tx {
        begin(&self.pool, Scope::tenant(self.tenant)).await.unwrap()
    }

    /// Another property of the tenant.
    pub async fn add_property(&self, code: &str) -> Uuid {
        let mut tx = self.tx().await;
        let hotel = property::NewProperty {
            code: code.into(),
            name: "Other".into(),
            timezone: "Asia/Colombo".into(),
            base_currency: "LKR".into(),
        };
        let property = property::create_property(&mut tx, self.tenant, self.user, hotel).await.unwrap();
        tx.commit().await.unwrap();
        property.id
    }

    pub async fn add_room_type(&self, property: Uuid, code: &str) -> Uuid {
        let mut tx = self.tx().await;
        let room_type = rooms::NewRoomType {
            code: code.into(),
            name: "Room".into(),
            base_occupancy: 2,
            max_adults: 2,
            max_children: 1,
            max_occupancy: 3,
            overbooking: 0,
            bed_config: vec![],
            amenities: vec![],
        };
        let id = rooms::create_room_type(&mut tx, self.tenant, self.user, property, room_type).await.unwrap().id;
        tx.commit().await.unwrap();
        id
    }

    /// An open issue in the hotel's room.
    pub async fn issue(&self) -> Uuid {
        let id = Uuid::now_v7();
        let mut tx = self.tx().await;
        sqlx::query(
            "insert into room_issue (id, tenant_id, property_id, room_id, severity, description, reported_by)
             values ($1, $2, $3, $4, 'low', 'Broken lamp', $5)",
        )
        .bind(id)
        .bind(self.tenant.0)
        .bind(self.property)
        .bind(self.room)
        .bind(self.user.0)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        id
    }

    pub async fn guest(&self) -> Uuid {
        let id = Uuid::now_v7();
        let mut tx = self.tx().await;
        sqlx::query(
            "insert into guest (id, tenant_id, first_name, last_name, residency) values ($1, $2, 'Ada', 'Silva', 'resident')",
        )
        .bind(id)
        .bind(self.tenant.0)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        id
    }

    /// A stay of `status` for `[business date + from, business date + to)` with `guest` as `role`: `booker`,
    /// `primary` or `occupant` (the other roles go to a fresh guest).
    pub async fn stay(&self, guest: Uuid, role: &str, from: i64, to: i64, status: &str) {
        let other = self.guest().await;
        let pick = |wanted: &str| if role == wanted { guest } else { other };
        let mut tx = self.tx().await;
        let taken: i64 = sqlx::query_scalar("select count(*) from reservation").fetch_one(&mut *tx).await.unwrap();
        let (plan, reservation, room) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        sqlx::query(
            "insert into rate_plan (id, tenant_id, property_id, code, name, kind, segment, currency)
             values ($1, $2, $3, 'BAR' || $4, 'Best available', 'standard', 'IBE', 'USD')",
        )
        .bind(plan)
        .bind(self.tenant.0)
        .bind(self.property)
        .bind(taken + 1)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "insert into reservation (id, tenant_id, property_id, confirmation_no, source, booker_guest_id,
                                      booked_business_date)
             values ($1, $2, $3, $4, 'front_desk', $5, current_date)",
        )
        .bind(reservation)
        .bind(self.tenant.0)
        .bind(self.property)
        .bind(format!("GAL-{:06}", taken + 1))
        .bind(pick("booker"))
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "insert into reservation_room (id, tenant_id, property_id, reservation_id, room_type_id, stay, adults,
                                           children, rate_plan_id, meal_plan, status, primary_guest_id, currency,
                                           cancelled_at, cancellation_penalty, checked_in_at, checked_in_business_date,
                                           checked_out_at, cancelled_business_date)
             values ($1, $2, $3, $4, $5, daterange($6, $7), 2, 0, $8, 'RO', $9, $10, 'USD',
                     case when $9 = 'cancelled' then now() end, case when $9 = 'cancelled' then 0 end,
                     case when $9 in ('checked_in', 'checked_out') then now() end,
                     case when $9 in ('checked_in', 'checked_out') then $6 end,
                     case when $9 = 'checked_out' then now() end,
                     case when $9 in ('cancelled', 'no_show') then $6 end)",
        )
        .bind(room)
        .bind(self.tenant.0)
        .bind(self.property)
        .bind(reservation)
        .bind(self.room_type)
        .bind(self.business_date + Duration::days(from))
        .bind(self.business_date + Duration::days(to))
        .bind(plan)
        .bind(status)
        .bind(pick("primary"))
        .execute(&mut *tx)
        .await
        .unwrap();
        if role == "occupant" {
            sqlx::query(
                "insert into reservation_guest (tenant_id, property_id, reservation_room_id, guest_id)
                 values ($1, $2, $3, $4)",
            )
            .bind(self.tenant.0)
            .bind(self.property)
            .bind(room)
            .bind(guest)
            .execute(&mut *tx)
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();
    }

    pub async fn start(&self, target: Target, bytes: u64) -> Result<NewUpload, MediaError> {
        self.start_in(self.property, target, bytes).await
    }

    pub async fn start_in(&self, property: Uuid, target: Target, bytes: u64) -> Result<NewUpload, MediaError> {
        let mut tx = self.tx().await;
        let upload =
            start_upload(&mut tx, self.tenant, self.user, property, target, "photo.jpg", bytes, [7; 32]).await?;
        tx.commit().await.unwrap();
        Ok(upload)
    }

    pub async fn complete(&self, asset: Uuid, file: u8) -> Result<Completed, MediaError> {
        self.complete_in(self.property, asset, file).await
    }

    pub async fn complete_in(&self, property: Uuid, asset: Uuid, file: u8) -> Result<Completed, MediaError> {
        let mut tx = self.tx().await;
        let completed = complete_upload(&mut tx, self.tenant, self.user, property, asset, verified(file)).await?;
        tx.commit().await.unwrap();
        Ok(completed)
    }

    /// Starts and completes an upload of `file` for `target`.
    pub async fn upload(&self, target: Target, file: u8) -> Completed {
        let upload = self.start(target, 1000).await.unwrap();
        self.complete(upload.asset_id, file).await.unwrap()
    }

    pub async fn apply(&self, result: &MediaResult) {
        let mut tx = self.tx().await;
        media::apply_result(&mut tx, result).await.unwrap();
        tx.commit().await.unwrap();
    }

    /// Reports `asset` ready with a thumb and an md rendition in both formats.
    pub async fn ready(&self, asset: Uuid) {
        self.apply(&ready(self.tenant, asset)).await;
    }

    pub async fn audited(&self, action: &str) -> i64 {
        sqlx::query_scalar("select count(*) from audit_log where action = $1")
            .bind(action)
            .fetch_one(&mut *self.tx().await)
            .await
            .unwrap()
    }

    pub async fn assets(&self) -> i64 {
        sqlx::query_scalar("select count(*) from media_asset").fetch_one(&mut *self.tx().await).await.unwrap()
    }

    pub async fn status(&self, asset: Uuid) -> Option<String> {
        sqlx::query_scalar("select status from media_asset where id = $1")
            .bind(asset)
            .fetch_optional(&mut *self.tx().await)
            .await
            .unwrap()
    }

    /// The payloads queued on `topic`.
    pub async fn outbox(&self, topic: &str) -> Vec<serde_json::Value> {
        sqlx::query_scalar("select payload from outbox where topic = $1 order by id")
            .bind(topic)
            .fetch_all(&self.owner)
            .await
            .unwrap()
    }

    /// Runs `sql` with `$1` bound to `id` in the tenant, to age rows.
    pub async fn exec(&self, sql: &'static str, id: Uuid) {
        let mut tx = self.tx().await;
        sqlx::query(sql).bind(id).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    }
}

pub fn guest_target(guest: Uuid) -> Target {
    Target::Guest { guest, kind: DocumentKind::Passport }
}

/// What Finalize reports for test file number `file`: files with the same number have the same BLAKE3.
pub fn verified(file: u8) -> Verified {
    Verified { blake3: [file; 32], mime: "image/jpeg".into(), bytes: 1000, width: Some(4000), height: Some(3000) }
}

pub fn asset_of(completed: &Completed) -> Uuid {
    match completed {
        Completed::Processing(asset) | Completed::Duplicate { asset, .. } => asset.id,
    }
}

pub fn ready(tenant: TenantId, asset: Uuid) -> MediaResult {
    let rendition = |size: Size, format: Format| RenditionOut {
        size,
        format,
        width: 320,
        height: 320,
        bytes: 100,
        key: format!("t/{}/a/{asset}/{}.{}", tenant.0, size.as_str(), format.as_str()),
    };
    MediaResult::Ready {
        tenant_id: tenant.0,
        asset_id: asset,
        width: 4000,
        height: 3000,
        archive_key: format!("t/{}/a/{asset}/archive.jxl", tenant.0),
        renditions: vec![
            rendition(Size::Thumb, Format::Avif),
            rendition(Size::Thumb, Format::Webp),
            rendition(Size::Md, Format::Avif),
            rendition(Size::Md, Format::Webp),
        ],
    }
}
