//! Booking on the site: a confirmed booking-site reservation with new guests, its manage token, its confirmation email
//! queued, and the checks before anything is written: the price, the rooms left, occupancy and residency.

use db::testing::app_pool;
use db::{Scope, TenantId, Tx, UserId, begin};
use ibe::{BookRoom, Booked, Booker, BookingError, BookingRequest, RoomAsk, SearchRequest};
use rates::{
    CancellationRule, DepositDue, DepositKind, DepositRule, MealPlan, NewCancellationPolicy, NewRatePlan, Penalty,
    PenaltyKind, PlanKind, RatePlan, Residency, Segment,
};
use rooms::{NewRoomType, RoomType};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::collections::BTreeMap;
use time::{Date, Duration, OffsetDateTime};
use uuid::Uuid;

/// A tenant with one property `GAL` in LKR with an enabled booking site, a `DLX` room type (2 adults and a child, 3 in
/// all) with `rooms` rooms, and an `IBE` plan `BAR` for anyone at 10 000 a night, room only or with breakfast, with a
/// cancellation policy and a deposit rule.
struct Hotel {
    pool: PgPool,
    /// The superuser, which reads the outbox.
    owner: PgPool,
    tenant: TenantId,
    user: UserId,
    property: Uuid,
    business_date: Date,
    deluxe: RoomType,
    bar: RatePlan,
}

impl Hotel {
    async fn new(opts: PgConnectOptions, rooms: u32) -> Self {
        let owner = PgPool::connect_with(opts.clone()).await.unwrap();
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
        let hotel = property::NewProperty {
            code: "GAL".into(),
            name: "Galle Fort Hotel".into(),
            timezone: "Asia/Colombo".into(),
            base_currency: "LKR".into(),
        };
        let property = property::create_property(&mut tx, tenant, user, hotel).await.unwrap();
        sqlx::query(
            "insert into ibe_site (tenant_id, property_id, enabled, public_code, contact_email)
             values ($1, $2, true, 'GAL', 'desk@gallefort.example')",
        )
        .bind(tenant.0)
        .bind(property.id)
        .execute(&mut *tx)
        .await
        .unwrap();
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
        let deluxe = rooms::create_room_type(&mut tx, tenant, user, property.id, deluxe).await.unwrap();
        for n in 1..=rooms {
            let room =
                rooms::NewRoom { room_type_id: deluxe.id, number: format!("10{n}"), floor: None, section_id: None };
            rooms::create_room(&mut tx, tenant, user, property.id, room).await.unwrap();
        }
        let policy = NewCancellationPolicy {
            name: "Flexible".into(),
            rules: vec![CancellationRule {
                days_before_arrival: 3,
                penalty: Penalty { kind: PenaltyKind::Nights, value: 1 },
            }],
            no_show: Penalty { kind: PenaltyKind::Nights, value: 1 },
        };
        let policy = rates::create_cancellation_policy(&mut tx, tenant, user, property.id, policy).await.unwrap();
        let bar = NewRatePlan {
            cancellation_policy_id: Some(policy.id),
            deposit_rule: Some(DepositRule {
                kind: DepositKind::Percent,
                value: 5_000,
                due: DepositDue::DaysBeforeArrival { days: 1 },
            }),
            ..new_plan("BAR", deluxe.id)
        };
        let bar = create_plan(&mut tx, (tenant, user, property.id), deluxe.id, property.business_date, bar).await;
        tx.commit().await.unwrap();
        Self { pool, owner, tenant, user, property: property.id, business_date: property.business_date, deluxe, bar }
    }

    async fn tx(&self) -> Tx {
        begin(&self.pool, Scope::tenant(self.tenant)).await.unwrap()
    }

    fn day(&self, days: i64) -> Date {
        self.business_date + Duration::days(days)
    }

    /// Creates `input`, priced at 10 000 a night from one adult for 60 days.
    async fn plan(&self, input: NewRatePlan) -> RatePlan {
        let mut tx = self.tx().await;
        let owner = (self.tenant, self.user, self.property);
        let plan = create_plan(&mut tx, owner, self.deluxe.id, self.business_date, input).await;
        tx.commit().await.unwrap();
        plan
    }

    /// A booking of `rooms` (adults, children) on `plan`, room only, for `[D + 1, D + 3)`, by Kamala Perera at
    /// `email`, for `residency`, at the totals the search shows for it.
    async fn request(
        &self,
        plan: &RatePlan,
        residency: Residency,
        rooms: &[(i32, i32)],
        email: &str,
    ) -> BookingRequest {
        let rooms: Vec<BookRoom> = rooms
            .iter()
            .map(|&(adults, children)| BookRoom {
                room_type_id: self.deluxe.id,
                rate_plan_id: plan.id,
                meal_plan: MealPlan::Ro,
                adults,
                children,
                occupant_name: None,
            })
            .collect();
        let mut request = BookingRequest {
            residency,
            booker: Booker {
                first_name: "Kamala".into(),
                last_name: "Perera".into(),
                email: email.into(),
                phone: "+94 77 123 4567".into(),
                country: "LK".into(),
                arrival_time: None,
            },
            rooms,
            check_in: self.day(1),
            check_out: self.day(3),
            special_requests: "A quiet room, please".into(),
            expected_totals: BTreeMap::new(),
            agreed: true,
        };
        request.expected_totals = self.totals(&request).await;
        request
    }

    /// The totals per currency the search shows for `request`'s rooms, or none when it offers one of them no more.
    async fn totals(&self, request: &BookingRequest) -> BTreeMap<String, i64> {
        let search = SearchRequest {
            check_in: request.check_in,
            check_out: request.check_out,
            residency: request.residency,
            rooms: request.rooms.iter().map(|room| RoomAsk { adults: room.adults, children: room.children }).collect(),
        };
        let result = ibe::search(&mut self.tx().await, self.property, &search).await.unwrap();
        let mut totals = BTreeMap::new();
        for (index, room) in request.rooms.iter().enumerate() {
            let offer = result
                .room_types
                .iter()
                .flat_map(|offered| offered.plans.iter())
                .filter(|plan| plan.rate_plan_id == room.rate_plan_id)
                .flat_map(|plan| plan.meal_plans.iter().map(move |offer| (plan, offer)))
                .find(|(_, offer)| offer.meal_plan == room.meal_plan && offer.rooms.contains(&(index as u8)));
            if let Some((plan, offer)) = offer {
                *totals.entry(plan.currency.clone()).or_default() += offer.total;
            }
        }
        totals
    }

    async fn book(&self, request: &BookingRequest) -> Result<Booked, BookingError> {
        let mut tx = self.tx().await;
        let booked = ibe::book(&mut tx, self.tenant, self.property, request).await?;
        tx.commit().await.unwrap();
        Ok(booked)
    }

    async fn reservations(&self) -> i64 {
        sqlx::query_scalar("select count(*) from reservation").fetch_one(&mut *self.tx().await).await.unwrap()
    }
}

/// A standard `IBE` plan `code` for anyone in LKR selling `room_type` room only or with breakfast.
fn new_plan(code: &str, room_type: Uuid) -> NewRatePlan {
    NewRatePlan {
        code: code.into(),
        name: format!("Plan {code}"),
        kind: PlanKind::Standard,
        segment: Segment::Ibe,
        residency: None,
        currency: "LKR".into(),
        parent_id: None,
        derive_mode: None,
        derive_value: None,
        rounding_step: 1,
        extra_adult_amount: 1_000,
        inherit_restrictions: false,
        allowed_meal_plans: vec![MealPlan::Ro, MealPlan::Bb],
        cancellation_policy_id: None,
        deposit_rule: None,
        room_type_ids: vec![room_type],
    }
}

/// Creates `input` in the property of `(tenant, user, property)`, priced for `room_type` at 10 000 a night from one
/// adult for 60 days from `from`.
async fn create_plan(
    tx: &mut Tx,
    (tenant, user, property): (TenantId, UserId, Uuid),
    room_type: Uuid,
    from: Date,
    input: NewRatePlan,
) -> RatePlan {
    let plan = rates::create_rate_plan(tx, tenant, user, property, input).await.unwrap();
    let prices: Vec<rates::Price> = (0..60)
        .map(|day| rates::Price {
            room_type_id: room_type,
            date: from + Duration::days(day),
            occupancy: 1,
            amount: 10_000,
        })
        .collect();
    rates::set_prices(tx, tenant, user, property, plan.id, &prices).await.unwrap();
    plan
}

#[derive(sqlx::FromRow)]
struct ReservationRow {
    source: String,
    guarantee: String,
    created_by: Option<Uuid>,
    booker_guest_id: Uuid,
    notes: String,
}

#[derive(sqlx::FromRow)]
struct RoomRow {
    status: String,
    cancellation_terms: Option<serde_json::Value>,
    deposit_terms: Option<serde_json::Value>,
    first_name: String,
    last_name: String,
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn booking_creates_confirmed_ibe_reservation(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts, 3).await;
    let mut request = hotel.request(&hotel.bar, Residency::Resident, &[(2, 0), (2, 1)], "Kamala@Example.com").await;
    request.rooms[1].occupant_name = Some("  Nimal  de Silva ".into());
    request.booker.arrival_time = Some(time::macros::time!(14:30));

    let booked = hotel.book(&request).await.unwrap();
    let again =
        hotel.book(&hotel.request(&hotel.bar, Residency::Resident, &[(1, 0)], "kamala@example.com").await).await;

    let mut tx = hotel.tx().await;
    let row: ReservationRow =
        sqlx::query_as("select source, guarantee, created_by, booker_guest_id, notes from reservation where id = $1")
            .bind(booked.reservation_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!((row.source.as_str(), row.guarantee.as_str(), row.created_by), ("ibe", "none", None));
    assert_eq!(row.notes, "Arrival time: 14:30\nA quiet room, please");
    assert!(booked.confirmation_no.starts_with("GAL-"), "{}", booked.confirmation_no);
    let rooms: Vec<RoomRow> = sqlx::query_as(
        "select rr.status, rr.cancellation_terms, rr.deposit_terms, g.first_name, g.last_name
         from reservation_room rr join guest g on g.id = rr.primary_guest_id
         where rr.reservation_id = $1 order by rr.adults desc, rr.children",
    )
    .bind(booked.reservation_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(rooms.len(), 2);
    assert!(rooms.iter().all(|room| room.status == "confirmed"));
    assert_eq!(
        rooms[0].cancellation_terms,
        Some(serde_json::json!({
            "rules": [{"days_before_arrival": 3, "penalty": {"kind": "nights", "value": 1}}],
            "no_show": {"kind": "nights", "value": 1},
        })),
        "the terms are snapshotted"
    );
    assert!(rooms[0].deposit_terms.is_some(), "the deposit rule is kept");
    let name = |room: &RoomRow| (room.first_name.clone(), room.last_name.clone());
    assert_eq!(name(&rooms[0]), ("Kamala".into(), "Perera".into()), "the booker stays in the first");
    assert_eq!(name(&rooms[1]), ("Nimal  de".into(), "Silva".into()), "the occupant has a profile");

    let booker: (String, String, String, String, String) =
        sqlx::query_as("select email, phone, country::text, residency, first_name from guest where id = $1")
            .bind(row.booker_guest_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(
        booker,
        ("kamala@example.com".into(), "+94 77 123 4567".into(), "LK".into(), "resident".into(), "Kamala".into())
    );
    let again = again.unwrap();
    let bookers: Vec<Uuid> = sqlx::query_scalar("select id from guest where email = 'kamala@example.com' order by id")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(bookers.len(), 2, "a second booking with the same email is a second guest");
    assert_ne!(again.reservation_id, booked.reservation_id);

    let (hash, expires_at): (Vec<u8>, OffsetDateTime) =
        sqlx::query_as("select sha256, expires_at from reservation_access_token where reservation_id = $1")
            .bind(booked.reservation_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(hash, ibe::token_hash(&booked.token));
    assert_eq!(booked.token.len(), 43, "32 bytes, base64url");
    let colombo = time::UtcOffset::from_hms(5, 30, 0).unwrap();
    assert_eq!(expires_at, hotel.day(3 + 31).midnight().assume_offset(colombo), "the end of check-out + 30 days");

    let jobs: Vec<serde_json::Value> =
        sqlx::query_scalar("select payload from outbox where topic = 'mail-send' order by id")
            .fetch_all(&hotel.owner)
            .await
            .unwrap();
    let job: mail::MailJob = serde_json::from_value(jobs[0].clone()).unwrap();
    assert_eq!(
        job,
        mail::MailJob::IbeConfirmation {
            tenant_id: hotel.tenant.0,
            reservation_id: booked.reservation_id,
            token: booked.token.clone()
        }
    );

    let audits: Vec<(Option<Uuid>, String, serde_json::Value)> = sqlx::query_as(
        "select actor_user_id, action, data from audit_log
         where entity_id = $1 and action in ('ibe.booking_created', 'reservation.created') order by action",
    )
    .bind(booked.reservation_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(audits[0].1, "ibe.booking_created");
    assert_eq!(audits[0].2, serde_json::json!({"actor": "ibe", "rooms": 2}), "no email or phone");
    assert_eq!(
        (audits[1].0, audits[1].1.as_str(), &audits[1].2["actor"]),
        (None, "reservation.created", &"ibe".into())
    );

    let confirmation = ibe::confirmation(&mut tx, booked.reservation_id, &booked.token).await.unwrap().unwrap();
    assert_eq!(confirmation.totals, request.expected_totals);
    assert_eq!((confirmation.code.as_str(), confirmation.booker_email.as_str()), ("GAL", "kamala@example.com"));
    assert_eq!(confirmation.contact_email.as_deref(), Some("desk@gallefort.example"));
    assert_eq!(confirmation.deposits.len(), 1, "the advisory deposit");
    assert!(ibe::confirmation(&mut tx, booked.reservation_id, "not-the-token").await.unwrap().is_none());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn price_changed_needs_reconfirm(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts, 3).await;
    let mut request = hotel.request(&hotel.bar, Residency::Resident, &[(2, 0)], "kamala@example.com").await;
    let quoted = request.expected_totals.clone();
    request.expected_totals.insert("LKR".into(), quoted["LKR"] - 1_000);

    let changed = hotel.book(&request).await;

    match changed {
        Err(BookingError::PriceChanged { totals }) => assert_eq!(totals, quoted),
        other => panic!("{other:?}"),
    }
    assert_eq!(hotel.reservations().await, 0);
    request.expected_totals = quoted;
    hotel.book(&request).await.expect("booked once the guest confirms the new totals");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn sold_out_race(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts, 1).await;
    let first = hotel.request(&hotel.bar, Residency::Resident, &[(2, 0)], "kamala@example.com").await;
    let second = hotel.request(&hotel.bar, Residency::Resident, &[(2, 0)], "nimal@example.com").await;

    let (a, b) = tokio::join!(hotel.book(&first), hotel.book(&second));

    let results = [a, b];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1, "{results:?}");
    assert_eq!(results.iter().filter(|result| matches!(result, Err(BookingError::SoldOut))).count(), 1, "{results:?}");
    assert_eq!(hotel.reservations().await, 1);
    let late = hotel.book(&second).await;
    assert!(matches!(late, Err(BookingError::SoldOut)), "a search that no longer offers it: {late:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn occupancy_over_max_is_invalid(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts, 3).await;
    let mut request = hotel.request(&hotel.bar, Residency::Resident, &[(2, 0)], "kamala@example.com").await;
    request.rooms[0].adults = 3;

    let over = hotel.book(&request).await;
    request.rooms[0].adults = 2;
    request.agreed = false;
    let not_agreed = hotel.book(&request).await;

    assert!(matches!(over, Err(BookingError::Invalid(ref message)) if message.contains("holds 2 adults")), "{over:?}");
    assert!(matches!(not_agreed, Err(BookingError::NotAgreed)), "{not_agreed:?}");
    assert_eq!(hotel.reservations().await, 0);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn residency_mismatch_is_invalid(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts, 3).await;
    let residents =
        hotel.plan(NewRatePlan { residency: Some(Residency::Resident), ..new_plan("FITL", hotel.deluxe.id) }).await;
    let mut request = hotel.request(&residents, Residency::Resident, &[(2, 0)], "kamala@example.com").await;
    request.residency = Residency::NonResident;

    let mismatch = hotel.book(&request).await;

    assert!(
        matches!(mismatch, Err(BookingError::Invalid(ref message)) if message.contains("only for residents")),
        "{mismatch:?}"
    );
    assert_eq!(hotel.reservations().await, 0);
}
