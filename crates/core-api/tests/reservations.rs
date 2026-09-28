mod common;

use axum::http::{Method, StatusCode, Uri, header};
use common::{TestApp, TestResponse, uuid};
use core_api::events::{LiveEvent, spawn_listener};
use core_api::idempotency::request_hash;
use db::UserId;
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;
use time::{Date, Duration, format_description::well_known::Iso8601};
use tracing_subscriber::fmt::MakeWriter;
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

/// A reservation room command (`…/cancel`, `…/assign`, `…/unassign`) with `If-Match: "<version>"`.
async fn command(app: &TestApp, cookie: &str, path: &str, version: i64, body: Option<Value>) -> TestResponse {
    let if_match = format!("\"{version}\"");
    app.send_with(Method::POST, path, Some(cookie), body, &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)]).await
}

/// A property with deluxe rooms 101 and 102 sold on BAR, USD 100 a night for two adults for 30 nights from the
/// business date, set up by its owner.
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

    fn guest(&self, last_name: &str) -> Value {
        json!({"first_name": "Ada", "last_name": last_name, "residency": "non_resident"})
    }

    /// One deluxe room on BAR, room only, for two adults over `[business date + from, business date + to)`.
    fn booking(&self, booker: &Value, from: i64, to: i64) -> Value {
        json!({"booker_guest_id": booker["id"], "source": "front_desk",
               "rooms": [{"room_type_id": self.deluxe, "rate_plan_id": self.bar, "meal_plan": "RO",
                          "check_in": self.day(from), "check_out": self.day(to), "adults": 2}]})
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

    async fn scalar(&self, sql: &'static str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(&self.superuser).await.unwrap()
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_guest_books_a_room_that_is_assigned_unassigned_and_cancelled_over_rest(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;

    let guest = post(&app, &hotel.owner, &hotel.guests(), hotel.guest("Silva")).await;
    let guest_path = format!("{}/{}", hotel.guests(), guest.body["id"].as_str().unwrap());
    let changed = patch(&app, &hotel.owner, &guest_path, 1, json!({"email": "ada@example.com", "country": "LK"})).await;
    let booking = hotel.booking(&guest.body, 1, 3);
    let created = post_with_key(&app, &hotel.owner, &hotel.reservations(), "booking-0001", booking.clone()).await;
    let replayed = post_with_key(&app, &hotel.owner, &hotel.reservations(), "booking-0001", booking).await;
    let sold_when_booked = hotel.sold(1, 3).await;
    let stay = hotel.stay(&created.body);
    let room_version = created.body["rooms"][0]["version"].as_i64().expect("each booked room has its version");
    let assign_101 = Some(json!({"room_id": hotel.rooms[0]}));
    let assigned = command(&app, &hotel.owner, &format!("{stay}/assign"), room_version, assign_101).await;
    let unassigned = command(&app, &hotel.owner, &format!("{stay}/unassign"), 2, None).await;
    let cancelled = command(&app, &hotel.owner, &format!("{stay}/cancel"), 3, None).await;

    assert_eq!(guest.status, StatusCode::CREATED, "{:?}", guest.body);
    assert_eq!(guest.headers[header::ETAG], "\"1\"");
    assert_eq!(
        (guest.body["last_name"].as_str(), guest.body["residency"].as_str()),
        (Some("Silva"), Some("non_resident"))
    );
    assert_eq!(changed.status, StatusCode::OK, "{:?}", changed.body);
    assert_eq!(changed.headers[header::ETAG], "\"2\"");
    assert_eq!(
        (changed.body["email"].as_str(), changed.body["country"].as_str()),
        (Some("ada@example.com"), Some("LK"))
    );
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    assert_eq!(created.headers[header::ETAG], "\"1\"");
    assert_eq!(created.body["confirmation_no"], "GAL-000001");
    assert_eq!(created.body["rooms"][0]["total"], 20_000);
    assert_eq!(room_version, 1, "a booked room starts at version 1");
    assert_eq!(created.body["totals"], json!([{"currency": "USD", "amount": 20_000}]));
    assert_eq!((replayed.status, &replayed.body), (StatusCode::CREATED, &created.body), "a retry replays the booking");
    assert_eq!(replayed.headers[header::ETAG], "\"1\"");
    assert_eq!(hotel.scalar("select count(*) from reservation").await, 1, "the retry booked nothing");
    assert_eq!(hotel.scalar("select value from property_counter").await, 1, "the retry took no second number");
    assert_eq!(sold_when_booked, [1, 1]);
    assert_eq!(assigned.status, StatusCode::OK, "{:?}", assigned.body);
    assert_eq!(assigned.headers[header::ETAG], "\"2\"");
    assert_eq!(
        (assigned.body["room_id"].clone(), assigned.body["room_number"].clone()),
        (json!(hotel.rooms[0]), json!("101"))
    );
    assert_eq!(unassigned.status, StatusCode::OK, "{:?}", unassigned.body);
    assert_eq!(unassigned.headers[header::ETAG], "\"3\"");
    assert_eq!(unassigned.body["room_id"], Value::Null);
    assert_eq!(cancelled.status, StatusCode::OK, "{:?}", cancelled.body);
    assert_eq!(cancelled.headers[header::ETAG], "\"4\"");
    assert_eq!(
        (cancelled.body["status"].as_str(), cancelled.body["penalty"].as_i64(), cancelled.body["currency"].as_str()),
        (Some("cancelled"), Some(0), Some("USD")),
        "BAR has no cancellation policy"
    );
    assert_eq!(hotel.sold(1, 3).await, [0, 0], "cancelling released the nights");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn reservation_rules_are_problems(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let guest = post(&app, &hotel.owner, &hotel.guests(), hotel.guest("Silva")).await.body;
    let guest_path = format!("{}/{}", hotel.guests(), guest["id"].as_str().unwrap());
    let first = post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 1, 3)).await.body;
    let second = post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 2, 4)).await.body;
    let (first, second) = (hotel.stay(&first), hotel.stay(&second));
    let assign_101 = json!({"room_id": hotel.rooms[0]});
    command(&app, &hotel.owner, &format!("{first}/assign"), 1, Some(assign_101.clone())).await;
    let min_stay = json!({"from": hotel.day(10), "to": hotel.day(11), "min_stay": 3});
    let restricted =
        app.send(Method::PUT, &format!("{}/restrictions", hotel.bar_path()), Some(&hotel.owner), Some(min_stay)).await;
    assert_eq!(restricted.status, StatusCode::NO_CONTENT, "{:?}", restricted.body);

    let responses = [
        (
            post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 2, 3)).await,
            StatusCode::CONFLICT,
            Some(format!("no DLX rooms left on {}", hotel.day(2))),
        ),
        (
            command(&app, &hotel.owner, &format!("{second}/assign"), 1, Some(assign_101)).await,
            StatusCode::CONFLICT,
            Some("room 101 is taken by GAL-000001 on those nights".to_owned()),
        ),
        (
            post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 10, 12)).await,
            StatusCode::UNPROCESSABLE_ENTITY,
            Some(format!("stays over {} are at least 3 nights", hotel.day(10))),
        ),
        (command(&app, &hotel.owner, &format!("{first}/cancel"), 7, None).await, StatusCode::PRECONDITION_FAILED, None),
        (
            app.send(Method::POST, &format!("{first}/cancel"), Some(&hotel.owner), None).await,
            StatusCode::PRECONDITION_REQUIRED,
            None,
        ),
        (patch(&app, &hotel.owner, &guest_path, 1, json!({})).await, StatusCode::UNPROCESSABLE_ENTITY, None),
    ];

    for (index, (response, status, detail)) in responses.into_iter().enumerate() {
        assert_eq!(response.status, status, "case {index}: {:?}", response.body);
        assert_eq!(response.headers[header::CONTENT_TYPE], "application/problem+json");
        if let Some(detail) = detail {
            assert_eq!(response.body["detail"], detail, "case {index}");
        }
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn front_desk_manages_reservations_and_housekeeping_and_accountants_cannot(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let front_desk = app.staff(&hotel.superuser, &hotel.owner, "desk@example.com", "front_desk").await;
    let housekeeping = app.staff(&hotel.superuser, &hotel.owner, "rooms@example.com", "housekeeping").await;
    let accountant = app.staff(&hotel.superuser, &hotel.owner, "accounts@example.com", "accountant").await;
    let guest = post(&app, &hotel.owner, &hotel.guests(), hotel.guest("Silva")).await.body;
    let guest_path = format!("{}/{}", hotel.guests(), guest["id"].as_str().unwrap());
    let stay = hotel.stay(&post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 1, 3)).await.body);
    let assign_101 = json!({"room_id": hotel.rooms[0]});

    for staff in [&housekeeping, &accountant] {
        let refused = [
            post(&app, staff, &hotel.guests(), hotel.guest("Perera")).await,
            patch(&app, staff, &guest_path, 1, json!({"notes": "Late arrival"})).await,
            post(&app, staff, &hotel.reservations(), hotel.booking(&guest, 1, 3)).await,
            command(&app, staff, &format!("{stay}/assign"), 1, Some(assign_101.clone())).await,
            command(&app, staff, &format!("{stay}/unassign"), 1, None).await,
            command(&app, staff, &format!("{stay}/cancel"), 1, None).await,
        ];
        for (index, response) in refused.into_iter().enumerate() {
            assert_eq!(response.status, StatusCode::FORBIDDEN, "case {index}: {:?}", response.body);
        }
    }
    let by_front_desk = [
        (post(&app, &front_desk, &hotel.guests(), hotel.guest("Perera")).await, StatusCode::CREATED),
        (patch(&app, &front_desk, &guest_path, 1, json!({"notes": "Late arrival"})).await, StatusCode::OK),
        (post(&app, &front_desk, &hotel.reservations(), hotel.booking(&guest, 1, 3)).await, StatusCode::CREATED),
        (command(&app, &front_desk, &format!("{stay}/assign"), 1, Some(assign_101)).await, StatusCode::OK),
        (command(&app, &front_desk, &format!("{stay}/unassign"), 2, None).await, StatusCode::OK),
        (command(&app, &front_desk, &format!("{stay}/cancel"), 3, None).await, StatusCode::OK),
    ];
    for (index, (response, status)) in by_front_desk.into_iter().enumerate() {
        assert_eq!(response.status, status, "case {index}: {:?}", response.body);
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_tenants_guests_and_reservations_cannot_be_changed(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let guest = post(&app, &hotel.owner, &hotel.guests(), hotel.guest("Silva")).await.body;
    let created = post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 1, 3)).await.body;
    let stay = created["rooms"][0]["id"].as_str().unwrap();
    let intruder = app.signup_owner("intruder@example.com", "Other Hotels").await;
    let own = json!({"code": "KAN", "name": "Kandy", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let own = format!(
        "/api/v1/properties/{}",
        post(&app, &intruder, "/api/v1/properties", own).await.body["id"].as_str().unwrap()
    );
    let own_guest = post(&app, &intruder, &format!("{own}/guests"), hotel.guest("Fernando")).await;
    assert_eq!(own_guest.status, StatusCode::CREATED, "{:?}", own_guest.body);
    // The intruder's own guest, booked into the other tenant's room type on its rate plan.
    let booking = hotel.booking(&own_guest.body, 1, 3);
    let guest_id = guest["id"].as_str().unwrap();

    let into_their_property = post(&app, &intruder, &hotel.guests(), hotel.guest("Fernando")).await;
    assert_eq!(into_their_property.status, StatusCode::NOT_FOUND, "{:?}", into_their_property.body);
    // Through the other tenant's property, and through the intruder's own property with the other tenant's ids.
    for property in [hotel.path.as_str(), own.as_str()] {
        let responses = [
            patch(&app, &intruder, &format!("{property}/guests/{guest_id}"), 1, json!({"notes": "X"})).await,
            post(&app, &intruder, &format!("{property}/reservations"), booking.clone()).await,
            command(
                &app,
                &intruder,
                &format!("{property}/reservation-rooms/{stay}/assign"),
                1,
                Some(json!({"room_id": hotel.rooms[0]})),
            )
            .await,
            command(&app, &intruder, &format!("{property}/reservation-rooms/{stay}/unassign"), 1, None).await,
            command(&app, &intruder, &format!("{property}/reservation-rooms/{stay}/cancel"), 1, None).await,
        ];
        // Through the intruder's own property, the reservation resolves far enough to see the room type and
        // rate plan are not theirs, which is 422 (an unknown id in the body), like an unknown guest; every
        // other case, and every case through the other tenant's property, never gets that far: 404.
        let reservation_status =
            if property == own.as_str() { StatusCode::UNPROCESSABLE_ENTITY } else { StatusCode::NOT_FOUND };
        let expected = [
            StatusCode::NOT_FOUND,
            reservation_status,
            StatusCode::NOT_FOUND,
            StatusCode::NOT_FOUND,
            StatusCode::NOT_FOUND,
        ];
        for (index, (response, status)) in responses.into_iter().zip(expected).enumerate() {
            assert_eq!(response.status, status, "{property}, case {index}: {:?}", response.body);
        }
    }
    let untouched: (i64, i64, i32, i32, i32, String, bool, i64) = sqlx::query_as(
        "select (select count(*) from guest), (select count(*) from reservation),
                (select version from guest where last_name = 'Silva'), (select version from reservation),
                (select version from reservation_room), (select status from reservation_room),
                (select room_id is null from reservation_room), (select sum(sold) from inventory_day)",
    )
    .fetch_one(&hotel.superuser)
    .await
    .unwrap();
    assert_eq!(untouched, (2, 1, 1, 1, 1, "confirmed".to_owned(), true, 2), "nothing of the other tenant changed");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_booking_tells_screens_to_refetch_the_list_the_reservation_and_its_inventory_months(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let app = TestApp::new(opts.clone()).await;
    spawn_listener(PgPool::connect_with(opts.clone()).await.unwrap(), app.state.events.clone()).await.unwrap();
    let hotel = Hotel::new(&app, opts).await;
    let guest = post(&app, &hotel.owner, &hotel.guests(), hotel.guest("Silva")).await.body;
    let mut events = app.state.events.subscribe();

    let created = post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&guest, 1, 3)).await.body;

    let LiveEvent::Invalidate(event) =
        tokio::time::timeout(StdDuration::from_secs(5), events.recv()).await.unwrap().unwrap()
    else {
        panic!("expected an invalidation")
    };
    let months: BTreeSet<String> =
        [hotel.day(1), hotel.day(2)].iter().map(|night| format!("inventory:{}:{}", hotel.id, &night[..7])).collect();
    let mut expected =
        vec![format!("reservations:{}", hotel.id), format!("reservation:{}", created["id"].as_str().unwrap())];
    expected.extend(months);
    assert_eq!(event.property_id, Some(hotel.id));
    assert_eq!(event.keys, expected);
}

/// Everything the `fmt` subscriber writes, for asserting on log output.
#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl CapturedLogs {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl std::io::Write for CapturedLogs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for CapturedLogs {
    type Writer = CapturedLogs;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// The spec: "Encrypted ID numbers never appear in API responses or logs, only a masked form."
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn id_numbers_never_appear_in_responses_stored_replays_or_logs(_: PgPoolOptions, opts: PgConnectOptions) {
    const DIGITS: &str = "1234567";
    // How `DIGITS` reads inside a bytea column cast to text.
    const DIGITS_HEX: &str = "31323334353637";
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let user = uuid(&app.send(Method::GET, "/api/v1/me", Some(&hotel.owner), None).await.body["user_id"]);
    let logs = CapturedLogs::default();
    let subscriber = tracing_subscriber::fmt().with_max_level(tracing::Level::TRACE).with_writer(logs.clone()).finish();
    let guard = tracing::subscriber::set_default(subscriber);
    let mut new_guest = hotel.guest("Silva");
    new_guest["id_doc"] = json!({"type": "passport", "number": "N1234567X"});

    let created = post_with_key(&app, &hotel.owner, &hotel.guests(), "guest-0001", new_guest.clone()).await;
    let replayed = post_with_key(&app, &hotel.owner, &hotel.guests(), "guest-0001", new_guest.clone()).await;
    let guest_path = format!("{}/{}", hotel.guests(), created.body["id"].as_str().unwrap());
    let renewed =
        patch(&app, &hotel.owner, &guest_path, 1, json!({"id_doc": {"type": "nic", "number": "991234567V"}})).await;
    // Scan the database while the guest still has a document. The guest holds NIC "991234567V" which contains DIGITS.
    // Verify the encrypted number is stored but the plaintext is not.
    let guest_id = uuid(&created.body["id"]);
    let (doc_type, doc_is_null): (Option<String>, bool) =
        sqlx::query_as("select id_doc_type, id_doc_number_enc is null from guest where id = $1")
            .bind(guest_id)
            .fetch_one(&hotel.superuser)
            .await
            .unwrap();
    assert_eq!(doc_type, Some("nic".to_owned()), "the guest's document type is updated");
    assert!(!doc_is_null, "the guest's id_doc_number_enc is not null while a document is present");
    // Now scan guest and audit_log rows to verify plaintext digits don't leak at rest.
    for rows in ["select row::text from guest row", "select row::text from audit_log row"] {
        for row in sqlx::query_scalar::<_, String>(rows).fetch_all(&hotel.superuser).await.unwrap() {
            assert!(
                !row.contains(DIGITS) && !row.contains(DIGITS_HEX),
                "{rows} holds the NIC number at {}: {row}",
                DIGITS
            );
        }
    }
    let removed = patch(&app, &hotel.owner, &guest_path, 2, json!({"id_doc": null})).await;
    let booked = post(&app, &hotel.owner, &hotel.reservations(), hotel.booking(&created.body, 1, 3)).await;
    drop(guard);

    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    assert_eq!(
        (created.body["id_doc_type"].as_str(), created.body["id_doc_masked"].as_str()),
        (Some("passport"), Some("•••• 567X"))
    );
    assert_eq!((replayed.status, &replayed.body), (StatusCode::CREATED, &created.body));
    assert_eq!(renewed.status, StatusCode::OK, "{:?}", renewed.body);
    assert_eq!(
        (renewed.body["id_doc_type"].as_str(), renewed.body["id_doc_masked"].as_str()),
        (Some("nic"), Some("•••• 567V"))
    );
    assert_eq!(removed.status, StatusCode::OK, "{:?}", removed.body);
    assert_eq!(
        (removed.body["id_doc_type"].clone(), removed.body["id_doc_masked"].clone()),
        (Value::Null, Value::Null)
    );
    assert_eq!(booked.status, StatusCode::CREATED, "{:?}", booked.body);
    for (name, response) in
        [("create", &created), ("replay", &replayed), ("renew", &renewed), ("remove", &removed), ("book", &booked)]
    {
        let body = response.body.to_string();
        assert!(!body.contains(DIGITS), "the {name} response shows the ID number: {body}");
    }

    // The replay is served from the stored response; the request itself is kept only as a hash.
    let (stored_hash, stored_body): (Vec<u8>, Vec<u8>) =
        sqlx::query_as("select request_hash, response_body from idempotency_key where key = 'guest-0001'")
            .fetch_one(&hotel.superuser)
            .await
            .unwrap();
    let uri: Uri = hotel.guests().parse().unwrap();
    let hash = request_hash(UserId(user), &Method::POST, &uri, new_guest.to_string().as_bytes());
    assert_eq!(stored_hash, hash, "the request body is stored only as its hash");
    let stored_body = String::from_utf8(stored_body).unwrap();
    assert!(stored_body.contains("•••• 567X") && !stored_body.contains(DIGITS), "stored: {stored_body}");
    for rows in [
        "select row::text from idempotency_key row",
        "select row::text from guest row",
        "select row::text from audit_log row",
    ] {
        for row in sqlx::query_scalar::<_, String>(rows).fetch_all(&hotel.superuser).await.unwrap() {
            assert!(!row.contains(DIGITS) && !row.contains(DIGITS_HEX), "{rows} holds the ID number: {row}");
        }
    }

    let logged = logs.text();
    assert!(logged.contains(&hotel.guests()), "the capture saw the guest requests: {logged}");
    assert!(!logged.contains(DIGITS), "the logs show the ID number: {logged}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn malformed_guest_bodies_do_not_echo_id_numbers(_: PgPoolOptions, opts: PgConnectOptions) {
    const PASSPORT_NUM: &str = "N1234567X";
    const NIC_NUM: i32 = 1234567;
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;

    // POST with invalid id_doc type (string instead of object), tracking the idempotency key
    let key1 = Uuid::now_v7().to_string();
    let response = post_with_key(
        &app,
        &hotel.owner,
        &hotel.guests(),
        &key1,
        json!({
            "first_name": "Test",
            "last_name": "Guest",
            "residency": "non_resident",
            "id_doc": PASSPORT_NUM
        }),
    )
    .await;
    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", response.body);
    let body_text = response.body.to_string();
    assert!(!body_text.contains(PASSPORT_NUM), "error includes id_doc value: {body_text}");

    // POST with invalid number type (int instead of string), tracking the idempotency key
    let key2 = Uuid::now_v7().to_string();
    let response = post_with_key(
        &app,
        &hotel.owner,
        &hotel.guests(),
        &key2,
        json!({
            "first_name": "Test",
            "last_name": "Guest",
            "residency": "non_resident",
            "id_doc": {
                "type": "nic",
                "number": NIC_NUM
            }
        }),
    )
    .await;
    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", response.body);
    let body_text = response.body.to_string();
    assert!(!body_text.contains(&NIC_NUM.to_string()), "error includes number value: {body_text}");

    // Verify error messages are fixed, not from serde
    assert_eq!(
        response.body["detail"].as_str(),
        Some("the request body does not have the expected shape"),
        "error message is fixed"
    );

    // Verify the malformed requests are stored safely through the superuser (not filtered by RLS).
    // Use convert_from to properly decode the bytea, not ::text which would be hex.
    let stored_bodies: Vec<String> = sqlx::query_scalar(
        "select convert_from(response_body, 'UTF8') from idempotency_key where key in ($1, $2) and response_body is not null",
    )
    .bind(&key1)
    .bind(&key2)
    .fetch_all(&hotel.superuser)
    .await
    .unwrap();
    assert_eq!(stored_bodies.len(), 2, "both 422 responses must be stored; got {}", stored_bodies.len());
    for (i, stored) in stored_bodies.iter().enumerate() {
        // Verify the response has the fixed detail message
        assert!(
            stored.contains("the request body does not have the expected shape"),
            "response {i} missing fixed detail: {stored}"
        );
        // Verify plaintext numbers don't leak
        assert!(!stored.contains(PASSPORT_NUM), "response {i} contains passport number: {stored}");
        assert!(!stored.contains(&NIC_NUM.to_string()), "response {i} contains NIC number: {stored}");
    }
}
