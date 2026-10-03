mod common;

use axum::http::{Method, StatusCode, header};
use common::{TestApp, TestResponse, uuid};
use core_api::events::{LiveEvent, spawn_listener};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::time::Duration as StdDuration;
use time::{Date, Duration, format_description::well_known::Iso8601};
use uuid::Uuid;

async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    let key = Uuid::now_v7().to_string();
    post_with_key(app, cookie, path, &key, body).await
}

async fn post_with_key(app: &TestApp, cookie: &str, path: &str, key: &str, body: Value) -> TestResponse {
    app.send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", key)])
        .await
}

async fn patch(app: &TestApp, cookie: &str, path: &str, version: i64, body: Value) -> TestResponse {
    let if_match = format!("\"{version}\"");
    app.send_with(Method::PATCH, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)])
        .await
}

/// A reservation room command (`…/modify`, `…/check-in`, …) with `If-Match: "<version>"` and an optional body.
async fn command(app: &TestApp, cookie: &str, path: &str, version: i64, body: Option<Value>) -> TestResponse {
    let if_match = format!("\"{version}\"");
    app.send_with(Method::POST, path, Some(cookie), body, &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)]).await
}

/// A `DELETE` (removing an occupant) with `If-Match: "<version>"`.
async fn delete_cmd(app: &TestApp, cookie: &str, path: &str, version: i64) -> TestResponse {
    let if_match = format!("\"{version}\"");
    app.send_with(Method::DELETE, path, Some(cookie), None, &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)]).await
}

/// A property with two deluxe rooms (101, 102), sold on BAR, USD 100 a night for two adults for 30 nights from
/// the business date, set up by its owner.
struct Hotel {
    owner: String,
    superuser: PgPool,
    id: Uuid,
    path: String,
    business_date: Date,
    deluxe: Uuid,
    rooms: Vec<Uuid>,
    bar: Uuid,
}

impl Hotel {
    async fn new(app: &TestApp, opts: PgConnectOptions) -> Self {
        let superuser = PgPool::connect_with(opts).await.unwrap();
        let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
        let property = json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"});
        let property = post(app, &owner, "/api/v1/properties", property).await.body;
        let id = uuid(&property["id"]);
        let path = format!("/api/v1/properties/{id}");
        let business_date = Date::parse(property["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
        let deluxe = json!({"code": "DLX", "name": "Deluxe", "base_occupancy": 2, "max_adults": 2,
                            "max_children": 1, "max_occupancy": 3});
        let deluxe = uuid(&post(app, &owner, &format!("{path}/room-types"), deluxe).await.body["id"]);
        let range = json!({"room_type_id": deluxe, "first": 101, "last": 102});
        let rooms = post(app, &owner, &format!("{path}/rooms/bulk"), range).await.body;
        let rooms = rooms.as_array().unwrap().iter().map(|room| uuid(&room["id"])).collect();
        let bar = json!({"code": "BAR", "name": "Best available", "kind": "standard", "segment": "IBE",
                         "currency": "USD", "room_type_ids": [deluxe]});
        let bar = uuid(&post(app, &owner, &format!("{path}/rate-plans"), bar).await.body["id"]);
        let hotel = Self { owner, superuser, id, path, business_date, deluxe, rooms, bar };
        let prices: Vec<Value> = (0..30)
            .map(|day| json!({"room_type_id": deluxe, "date": hotel.day(day), "occupancy": 2, "amount": 10_000}))
            .collect();
        let priced = app
            .send(
                Method::PUT,
                &format!("{}/prices", hotel.bar_path()),
                Some(&hotel.owner),
                Some(json!({"prices": prices})),
            )
            .await;
        assert_eq!(priced.status, StatusCode::NO_CONTENT, "{:?}", priced.body);
        hotel
    }

    fn day(&self, offset: i64) -> String {
        (self.business_date + Duration::days(offset)).to_string()
    }

    fn bar_path(&self) -> String {
        format!("{}/rate-plans/{}", self.path, self.bar)
    }

    fn guests(&self) -> String {
        format!("{}/guests", self.path)
    }

    fn reservations(&self) -> String {
        format!("{}/reservations", self.path)
    }

    fn accounts(&self) -> String {
        format!("{}/accounts", self.path)
    }

    fn guest(&self, last_name: &str) -> Value {
        json!({"first_name": "Ada", "last_name": last_name, "residency": "non_resident"})
    }

    /// One deluxe room on BAR, room only, for two adults over `[business date + from, business date + to)`.
    fn booking(&self, booker: &Value, from: i64, to: i64) -> Value {
        json!({"booker_guest_id": booker["id"], "source": "front_desk",
               "rooms": [{"room_type_id": self.deluxe, "rate_plan_id": self.bar, "meal_plan": "RO",
                          "check_in": self.day(from), "check_out": self.day(to), "adults": 2}]})
    }

    /// Books `booking` over REST, then takes the booked room out of the room booking auto-assigned it, for tests
    /// that start from an unassigned stay. The result shows no room and the room's version after the unassign.
    async fn book_unassigned_with(&self, app: &TestApp, booking: Value) -> Value {
        let mut created = post(app, &self.owner, &self.reservations(), booking).await.body;
        let version = created["rooms"][0]["version"].as_i64().unwrap();
        let freed = command(app, &self.owner, &format!("{}/unassign", self.stay(&created)), version, None).await;
        assert_eq!(freed.status, StatusCode::OK, "{:?}", freed.body);
        created["rooms"][0]["version"] = json!(version + 1);
        created["rooms"][0]["room_id"] = Value::Null;
        created["rooms"][0]["room_number"] = Value::Null;
        created
    }

    async fn book_unassigned(&self, app: &TestApp, guest: &Value, from: i64, to: i64) -> Value {
        self.book_unassigned_with(app, self.booking(guest, from, to)).await
    }

    /// The path of the first room of a reservation `created` over REST.
    fn stay(&self, created: &Value) -> String {
        format!("{}/reservation-rooms/{}", self.path, created["rooms"][0]["id"].as_str().unwrap())
    }

    /// Deluxe rooms sold on each night of `[business date + from, business date + to)`.
    async fn sold(&self, from: i64, to: i64) -> Vec<i32> {
        sqlx::query_scalar(
            "select sold from inventory_day where room_type_id = $1 and date >= $2 and date < $3 order by date",
        )
        .bind(self.deluxe)
        .bind(self.business_date + Duration::days(from))
        .bind(self.business_date + Duration::days(to))
        .fetch_all(&self.superuser)
        .await
        .unwrap()
    }
}

/// Books a room to an account, extends it, assigns a room, checks in on the arrival date and checks out early,
/// asserting each step's status and `ETag`.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_booking_billed_to_an_account_is_modified_assigned_checked_in_and_checked_out_early(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let app = TestApp::new(opts.clone()).await;
    spawn_listener(PgPool::connect_with(opts.clone()).await.unwrap(), app.state.events.clone()).await.unwrap();
    let hotel = Hotel::new(&app, opts).await;

    let account = post(
        &app,
        &hotel.owner,
        &hotel.accounts(),
        json!({"kind": "company", "name": "Acme Travel", "currency": "USD"}),
    )
    .await;
    assert_eq!(account.status, StatusCode::CREATED, "{:?}", account.body);
    assert_eq!(account.headers[header::ETAG], "\"1\"");
    let guest = post(&app, &hotel.owner, &hotel.guests(), hotel.guest("Silva")).await.body;
    let mut booking = hotel.booking(&guest, 0, 3);
    booking["account_id"] = account.body["id"].clone();

    let created = post(&app, &hotel.owner, &hotel.reservations(), booking).await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    assert_eq!(created.headers[header::ETAG], "\"1\"");
    let stay = hotel.stay(&created.body);
    // Booking auto-assigned a room; take it out again so the assignment below is by hand.
    let freed = command(&app, &hotel.owner, &format!("{stay}/unassign"), 1, None).await;
    assert_eq!(freed.status, StatusCode::OK, "{:?}", freed.body);
    assert_eq!(freed.headers[header::ETAG], "\"2\"");
    let before_extend = hotel.sold(0, 4).await;

    let modified =
        command(&app, &hotel.owner, &format!("{stay}/modify"), 2, Some(json!({"check_out": hotel.day(4)}))).await;
    assert_eq!(modified.status, StatusCode::OK, "{:?}", modified.body);
    assert_eq!(modified.headers[header::ETAG], "\"3\"");
    assert_eq!(modified.body["check_out"], hotel.day(4));
    assert_eq!(modified.body["total"], 40_000, "a fourth night at 100.00 was added");
    let after_extend = hotel.sold(0, 4).await;

    let assigned =
        command(&app, &hotel.owner, &format!("{stay}/assign"), 3, Some(json!({"room_id": hotel.rooms[0]}))).await;
    assert_eq!(assigned.status, StatusCode::OK, "{:?}", assigned.body);
    assert_eq!(assigned.headers[header::ETAG], "\"4\"");

    let checked_in = command(&app, &hotel.owner, &format!("{stay}/check-in"), 4, None).await;
    assert_eq!(checked_in.status, StatusCode::OK, "{:?}", checked_in.body);
    assert_eq!(checked_in.headers[header::ETAG], "\"5\"");
    assert_eq!(checked_in.body["status"], "checked_in");
    assert_eq!(checked_in.body["checked_in_business_date"], hotel.day(0));

    // Subscribed since before this test's first request, so nothing already sent is missed; the check-out's
    // own notification is found by its content (the NOTIFY listener delivers asynchronously, so earlier
    // steps' events may still arrive after this point).
    let mut events = app.state.events.subscribe();
    let checked_out = command(&app, &hotel.owner, &format!("{stay}/check-out"), 5, None).await;
    assert_eq!(checked_out.status, StatusCode::OK, "{:?}", checked_out.body);
    assert_eq!(checked_out.headers[header::ETAG], "\"6\"");
    assert_eq!(checked_out.body["status"], "checked_out");
    assert_eq!(
        checked_out.body["released_nights"],
        json!([hotel.day(1), hotel.day(2), hotel.day(3)]),
        "an early departure releases every night after today"
    );
    let after_checkout = hotel.sold(0, 4).await;

    let released_month = format!("inventory:{}:{}", hotel.id, &hotel.day(1)[..7]);
    let deadline = tokio::time::Instant::now() + StdDuration::from_secs(5);
    let found = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let received = tokio::time::timeout(remaining, events.recv())
            .await
            .unwrap_or_else(|_| panic!("no invalidation named {released_month} arrived in time"))
            .unwrap();
        if let LiveEvent::Invalidate(event) = received
            && event.keys.contains(&released_month)
        {
            break event;
        }
    };
    assert_eq!(found.property_id, Some(hotel.id));

    assert_eq!(before_extend, [1, 1, 1, 0], "the booking sold only its first three nights");
    assert_eq!(after_extend, [1, 1, 1, 1], "the modify sold the added fourth night");
    assert_eq!(after_checkout, [1, 0, 0, 0], "checking out early released every night but the one kept");

    let billed_account: Option<Uuid> = sqlx::query_scalar("select account_id from reservation where id = $1")
        .bind(uuid(&created.body["id"]))
        .fetch_one(&hotel.superuser)
        .await
        .unwrap();
    assert_eq!(billed_account, Some(uuid(&account.body["id"])), "the reservation is billed to the account");
}

/// Every rule the new commands enforce shows up as the right problem.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_new_commands_rules_are_problems(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let guest = post(&app, &hotel.owner, &hotel.guests(), hotel.guest("Silva")).await.body;

    // Check-in on the wrong date: arrives in five days, assigned, but today is not its arrival date.
    let future = hotel.book_unassigned(&app, &guest, 5, 7).await;
    let future_stay = hotel.stay(&future);
    let future_assigned =
        command(&app, &hotel.owner, &format!("{future_stay}/assign"), 2, Some(json!({"room_id": hotel.rooms[0]})))
            .await;
    assert_eq!(future_assigned.status, StatusCode::OK, "{:?}", future_assigned.body);
    let wrong_date = command(&app, &hotel.owner, &format!("{future_stay}/check-in"), 3, None).await;
    assert_eq!(wrong_date.status, StatusCode::CONFLICT, "{:?}", wrong_date.body);
    assert_eq!(wrong_date.body["detail"], format!("check-in is only on the arrival date ({})", hotel.day(5)));

    // No room: arrives today, confirmed, but never assigned.
    let unassigned = hotel.book_unassigned(&app, &guest, 0, 2).await;
    let unassigned_stay = hotel.stay(&unassigned);
    let no_room = command(&app, &hotel.owner, &format!("{unassigned_stay}/check-in"), 2, None).await;
    assert_eq!(no_room.status, StatusCode::CONFLICT, "{:?}", no_room.body);
    assert_eq!(no_room.body["detail"], "assign a room first");

    // Stale If-Match on modify.
    let stale_target = post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 8, 10)).await.body;
    let stale_stay = hotel.stay(&stale_target);
    let stale =
        command(&app, &hotel.owner, &format!("{stale_stay}/modify"), 99, Some(json!({"check_out": hotel.day(11)})))
            .await;
    assert_eq!(stale.status, StatusCode::PRECONDITION_FAILED, "{:?}", stale.body);

    // Missing If-Match on modify.
    let missing = app
        .send(
            Method::POST,
            &format!("{stale_stay}/modify"),
            Some(&hotel.owner),
            Some(json!({"check_out": hotel.day(11)})),
        )
        .await;
    assert_eq!(missing.status, StatusCode::PRECONDITION_REQUIRED, "{:?}", missing.body);

    // Empty modify: nothing to change.
    let empty = command(&app, &hotel.owner, &format!("{stale_stay}/modify"), 1, Some(json!({}))).await;
    assert_eq!(empty.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", empty.body);
    assert_eq!(empty.body["detail"], "nothing to change");

    // Modify onto a sold-out night: both physical rooms are taken on day 10, by two other bookings.
    let sellout_a = post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 10, 11)).await;
    let sellout_b = post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 10, 11)).await;
    assert_eq!(sellout_a.status, StatusCode::CREATED, "{:?}", sellout_a.body);
    assert_eq!(sellout_b.status, StatusCode::CREATED, "{:?}", sellout_b.body);
    let short = post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 5, 6)).await.body;
    let short_stay = hotel.stay(&short);
    let sold_out =
        command(&app, &hotel.owner, &format!("{short_stay}/modify"), 1, Some(json!({"check_out": hotel.day(11)})))
            .await;
    assert_eq!(sold_out.status, StatusCode::CONFLICT, "{:?}", sold_out.body);
    assert_eq!(sold_out.body["detail"], format!("no DLX rooms left on {}", hotel.day(10)));
}

/// Housekeeping and accountants are refused every new mutation; front desk can check a room in.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn housekeeping_and_accountants_are_refused_the_new_mutations_front_desk_checks_in(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let front_desk = app.staff(&hotel.superuser, &hotel.owner, "desk@example.com", "front_desk").await;
    let housekeeping = app.staff(&hotel.superuser, &hotel.owner, "rooms@example.com", "housekeeping").await;
    let accountant = app.staff(&hotel.superuser, &hotel.owner, "accounts@example.com", "accountant").await;
    let fake = Uuid::now_v7();
    let account_body = json!({"kind": "company", "name": "Acme", "currency": "USD"});

    for staff in [&housekeeping, &accountant] {
        let refused = [
            post(&app, staff, &hotel.accounts(), account_body.clone()).await,
            patch(&app, staff, &format!("{}/{fake}", hotel.accounts()), 1, json!({"name": "X"})).await,
            patch(&app, staff, &format!("{}/{fake}", hotel.reservations()), 1, json!({"notes": "X"})).await,
            command(
                &app,
                staff,
                &format!("{}/reservation-rooms/{fake}/modify", hotel.path),
                1,
                Some(json!({"check_out": hotel.day(5)})),
            )
            .await,
            command(&app, staff, &format!("{}/reservation-rooms/{fake}/check-in", hotel.path), 1, None).await,
            command(&app, staff, &format!("{}/reservation-rooms/{fake}/undo-check-in", hotel.path), 1, None).await,
            command(&app, staff, &format!("{}/reservation-rooms/{fake}/check-out", hotel.path), 1, None).await,
            command(
                &app,
                staff,
                &format!("{}/reservation-rooms/{fake}/guests", hotel.path),
                1,
                Some(json!({"guest_id": fake})),
            )
            .await,
            delete_cmd(&app, staff, &format!("{}/reservation-rooms/{fake}/guests/{fake}", hotel.path), 1).await,
        ];
        for (index, response) in refused.into_iter().enumerate() {
            assert_eq!(response.status, StatusCode::FORBIDDEN, "case {index}: {:?}", response.body);
        }
    }

    let guest = post(&app, &hotel.owner, &hotel.guests(), hotel.guest("Perera")).await.body;
    let created = hotel.book_unassigned(&app, &guest, 0, 2).await;
    let stay = hotel.stay(&created);
    let assigned =
        command(&app, &hotel.owner, &format!("{stay}/assign"), 2, Some(json!({"room_id": hotel.rooms[0]}))).await;
    assert_eq!(assigned.status, StatusCode::OK, "{:?}", assigned.body);
    let checked_in = command(&app, &front_desk, &format!("{stay}/check-in"), 3, None).await;
    assert_eq!(checked_in.status, StatusCode::OK, "{:?}", checked_in.body);
    assert_eq!(checked_in.body["status"], "checked_in");
}

/// Another tenant's account, reservation and room ids resolve to nothing through either property, and change
/// nothing.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_tenants_accounts_reservations_and_rooms_cannot_be_changed(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let account =
        post(&app, &hotel.owner, &hotel.accounts(), json!({"kind": "company", "name": "Acme", "currency": "USD"}))
            .await
            .body;
    let guest = post(&app, &hotel.owner, &hotel.guests(), hotel.guest("Silva")).await.body;
    let mut booking = hotel.booking(&guest, 0, 2);
    booking["account_id"] = account["id"].clone();
    let created = hotel.book_unassigned_with(&app, booking).await;
    let reservation_id = created["id"].as_str().unwrap();
    let room_id = created["rooms"][0]["id"].as_str().unwrap();
    let stay = hotel.stay(&created);
    let assigned =
        command(&app, &hotel.owner, &format!("{stay}/assign"), 2, Some(json!({"room_id": hotel.rooms[0]}))).await;
    assert_eq!(assigned.status, StatusCode::OK, "{:?}", assigned.body);

    let intruder = app.signup_owner("intruder@example.com", "Other Hotels").await;
    let own = json!({"code": "KAN", "name": "Kandy", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let own = format!(
        "/api/v1/properties/{}",
        post(&app, &intruder, "/api/v1/properties", own).await.body["id"].as_str().unwrap()
    );
    let account_id = account["id"].as_str().unwrap();

    for property in [hotel.path.as_str(), own.as_str()] {
        let responses = [
            patch(&app, &intruder, &format!("{property}/accounts/{account_id}"), 1, json!({"name": "X"})).await,
            patch(&app, &intruder, &format!("{property}/reservations/{reservation_id}"), 1, json!({"notes": "X"}))
                .await,
            command(
                &app,
                &intruder,
                &format!("{property}/reservation-rooms/{room_id}/modify"),
                2,
                Some(json!({"check_out": hotel.day(5)})),
            )
            .await,
            command(&app, &intruder, &format!("{property}/reservation-rooms/{room_id}/check-in"), 2, None).await,
            command(&app, &intruder, &format!("{property}/reservation-rooms/{room_id}/undo-check-in"), 2, None).await,
            command(&app, &intruder, &format!("{property}/reservation-rooms/{room_id}/check-out"), 2, None).await,
            command(
                &app,
                &intruder,
                &format!("{property}/reservation-rooms/{room_id}/guests"),
                2,
                Some(json!({"guest_id": guest["id"]})),
            )
            .await,
            delete_cmd(
                &app,
                &intruder,
                &format!("{property}/reservation-rooms/{room_id}/guests/{}", guest["id"].as_str().unwrap()),
                2,
            )
            .await,
        ];
        for (index, response) in responses.into_iter().enumerate() {
            assert_eq!(response.status, StatusCode::NOT_FOUND, "{property}, case {index}: {:?}", response.body);
        }
    }

    let untouched: (i32, i32, i32, String) = sqlx::query_as(
        "select (select version from account where id = $1), (select version from reservation where id = $2),
                (select version from reservation_room where id = $3), (select status from reservation_room where id = $3)",
    )
    .bind(uuid(&account["id"]))
    .bind(uuid(&created["id"]))
    .bind(uuid(&created["rooms"][0]["id"]))
    .fetch_one(&hotel.superuser)
    .await
    .unwrap();
    // The reservation and its room each moved by the unassign and the assignment that set the stay up.
    assert_eq!(untouched, (1, 3, 3, "confirmed".to_owned()), "nothing of the other tenant changed");
}
