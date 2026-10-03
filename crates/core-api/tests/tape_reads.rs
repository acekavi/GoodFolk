mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, TestResponse, uuid};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::{Date, Duration, format_description::well_known::Iso8601};
use uuid::Uuid;

async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    let key = Uuid::now_v7().to_string();
    app.send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)])
        .await
}

async fn graphql(app: &TestApp, cookie: &str, query: &str, variables: Value) -> Value {
    let response =
        app.send(Method::POST, "/graphql", Some(cookie), Some(json!({"query": query, "variables": variables}))).await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
    response.body
}

const TAPE: &str = "query ($p: UUID!, $rooms: [UUID!]!, $from: Date!, $to: Date!) {
    tapeWindow(propertyId: $p, roomIds: $rooms, from: $from, to: $to) {
        stays { id reservationId roomId roomTypeId start end status guestName accountName version }
        blocks { id roomId start end reason }
    }
}";

const UNASSIGNED: &str = "query ($p: UUID!, $from: Date!, $to: Date!) {
    unassignedStays(propertyId: $p, from: $from, to: $to) {
        id reservationId roomTypeId start end status guestName reason version
    }
}";

/// A property with two deluxe rooms (101, 102), sold on BAR, USD 100.00 a night for two adults for 30 nights
/// from the business date.
struct Hotel {
    owner: String,
    superuser: PgPool,
    tenant: Uuid,
    id: Uuid,
    path: String,
    business_date: Date,
    rooms: Vec<Value>,
    bar: Uuid,
}

impl Hotel {
    async fn new(app: &TestApp, opts: PgConnectOptions) -> Self {
        let superuser = PgPool::connect_with(opts).await.unwrap();
        let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
        let tenant = uuid(&app.send(Method::GET, "/api/v1/me", Some(&owner), None).await.body["current_tenant"]);
        let property = json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"});
        let property = post(app, &owner, "/api/v1/properties", property).await.body;
        let id = uuid(&property["id"]);
        let path = format!("/api/v1/properties/{id}");
        let business_date = Date::parse(property["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
        let deluxe = json!({"code": "DLX", "name": "Deluxe", "base_occupancy": 2, "max_adults": 2,
                            "max_children": 1, "max_occupancy": 3});
        let deluxe = uuid(&post(app, &owner, &format!("{path}/room-types"), deluxe).await.body["id"]);
        let range = json!({"room_type_id": deluxe, "first": 101, "last": 102});
        let rooms = post(app, &owner, &format!("{path}/rooms/bulk"), range).await.body.as_array().unwrap().clone();
        let bar = json!({"code": "BAR", "name": "Best available", "kind": "standard", "segment": "IBE",
                         "currency": "USD", "room_type_ids": [deluxe]});
        let bar = uuid(&post(app, &owner, &format!("{path}/rate-plans"), bar).await.body["id"]);
        let hotel = Self { owner, superuser, tenant, id, path, business_date, rooms, bar };
        let prices: Vec<Value> = (0..30)
            .map(|day| json!({"room_type_id": deluxe, "date": hotel.day(day), "occupancy": 2, "amount": 10_000}))
            .collect();
        let priced = app
            .send(
                Method::PUT,
                &format!("{}/rate-plans/{}/prices", hotel.path, hotel.bar),
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

    fn room_ids(&self) -> Vec<Value> {
        self.rooms.iter().map(|room| room["id"].clone()).collect()
    }

    async fn guest(&self, app: &TestApp, first: &str, last: &str) -> Value {
        let guest = json!({"first_name": first, "last_name": last, "residency": "non_resident"});
        post(app, &self.owner, &format!("{}/guests", self.path), guest).await.body
    }

    /// One deluxe room on BAR for two adults over `[business date + from, business date + to)`; booking
    /// auto-assigns a room.
    async fn book(&self, app: &TestApp, guest: &Value, from: i64, to: i64) -> Value {
        let booking = json!({"booker_guest_id": guest["id"], "source": "front_desk",
            "rooms": [{"room_type_id": self.rooms[0]["room_type_id"], "rate_plan_id": self.bar,
                       "meal_plan": "RO", "check_in": self.day(from), "check_out": self.day(to), "adults": 2}]});
        post(app, &self.owner, &format!("{}/reservations", self.path), booking).await.body
    }

    /// `book`, then takes the booked room out of the room booking auto-assigned it.
    async fn book_unassigned(&self, app: &TestApp, guest: &Value, from: i64, to: i64) -> Value {
        let mut created = self.book(app, guest, from, to).await;
        let version = created["rooms"][0]["version"].as_i64().unwrap();
        let path = format!("{}/reservation-rooms/{}/unassign", self.path, created["rooms"][0]["id"].as_str().unwrap());
        let if_match = format!("\"{version}\"");
        let freed = app
            .send_with(
                Method::POST,
                &path,
                Some(&self.owner),
                None,
                &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)],
            )
            .await;
        assert_eq!(freed.status, StatusCode::OK, "{:?}", freed.body);
        created["rooms"][0]["version"] = json!(version + 1);
        created
    }

    async fn block(&self, app: &TestApp, room: usize, from: i64, to: i64) -> Value {
        let reason: Uuid = sqlx::query_scalar("select id from block_reason where code = 'MAINTENANCE'")
            .fetch_one(&self.superuser)
            .await
            .unwrap();
        let body = json!({"from": self.day(from), "to": self.day(to), "kind": "out_of_order",
                          "reason_id": reason, "note": "Leaking pipe"});
        let blocked = post(
            app,
            &self.owner,
            &format!("{}/rooms/{}/blocks", self.path, self.rooms[room]["id"].as_str().unwrap()),
            body,
        )
        .await;
        assert_eq!(blocked.status, StatusCode::CREATED, "{:?}", blocked.body);
        blocked.body
    }

    fn tape(&self, rooms: &[Value], from: i64, to: i64) -> Value {
        json!({"p": self.id, "rooms": rooms, "from": self.day(from), "to": self.day(to)})
    }

    fn needing_rooms(&self, from: i64, to: i64) -> Value {
        json!({"p": self.id, "from": self.day(from), "to": self.day(to)})
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_owner_reads_the_tape_window_and_the_stays_that_need_a_room(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let silva = hotel.guest(&app, "Anula", "Silva").await;
    let seated = hotel.book(&app, &silva, 1, 4).await;
    let waiting = hotel.book_unassigned(&app, &silva, 2, 5).await;
    let block = hotel.block(&app, 1, 6, 8).await;

    let tape = graphql(&app, &hotel.owner, TAPE, hotel.tape(&hotel.room_ids(), 0, 14)).await;
    let needing = graphql(&app, &hotel.owner, UNASSIGNED, hotel.needing_rooms(0, 14)).await;

    assert_eq!(tape["errors"], Value::Null, "{tape:?}");
    let stays = tape["data"]["tapeWindow"]["stays"].as_array().unwrap();
    assert_eq!(stays.len(), 1, "{tape:?}");
    assert_eq!(stays[0]["id"], seated["rooms"][0]["id"]);
    assert_eq!(stays[0]["reservationId"], seated["id"]);
    assert_eq!(stays[0]["roomId"], seated["rooms"][0]["room_id"]);
    assert_eq!(stays[0]["roomTypeId"], hotel.rooms[0]["room_type_id"]);
    assert_eq!(stays[0]["start"], hotel.day(1));
    assert_eq!(stays[0]["end"], hotel.day(4));
    assert_eq!(stays[0]["status"], "CONFIRMED");
    assert_eq!(stays[0]["guestName"], "Silva, A.");
    assert_eq!(stays[0]["accountName"], Value::Null);
    assert_eq!(stays[0]["version"], seated["rooms"][0]["version"]);
    assert_eq!(
        tape["data"]["tapeWindow"]["blocks"],
        json!([{"id": block["id"], "roomId": hotel.rooms[1]["id"], "start": hotel.day(6), "end": hotel.day(8),
                "reason": "Maintenance"}])
    );

    let needing = needing["data"]["unassignedStays"].as_array().unwrap();
    assert_eq!(needing.len(), 1, "{needing:?}");
    assert_eq!(needing[0]["id"], waiting["rooms"][0]["id"]);
    assert_eq!(needing[0]["reservationId"], waiting["id"]);
    assert_eq!(needing[0]["start"], hotel.day(2));
    assert_eq!(needing[0]["end"], hotel.day(5));
    assert_eq!(needing[0]["status"], "CONFIRMED");
    assert_eq!(needing[0]["guestName"], "Silva, A.");
    assert_eq!(needing[0]["reason"], "NO_SINGLE_ROOM");
    assert_eq!(needing[0]["version"], waiting["rooms"][0]["version"]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_tape_window_over_the_limits_is_an_error(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;

    let too_long = graphql(&app, &hotel.owner, TAPE, hotel.tape(&hotel.room_ids(), 0, 43)).await;
    let no_rooms = graphql(&app, &hotel.owner, TAPE, hotel.tape(&[], 0, 7)).await;
    let unknown = graphql(&app, &hotel.owner, TAPE, hotel.tape(&[json!(Uuid::now_v7())], 0, 7)).await;
    let unassigned_too_long = graphql(&app, &hotel.owner, UNASSIGNED, hotel.needing_rooms(0, 43)).await;

    for refused in [&too_long, &no_rooms, &unknown, &unassigned_too_long] {
        assert!(refused["errors"][0]["message"].is_string(), "{refused:?}");
        assert_eq!(refused["data"], Value::Null, "{refused:?}");
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn housekeeping_reads_the_tape_and_a_user_without_a_grant_or_another_tenant_cannot(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let silva = hotel.guest(&app, "Anula", "Silva").await;
    hotel.book(&app, &silva, 1, 4).await;
    hotel.book_unassigned(&app, &silva, 2, 5).await;
    let housekeeping = app.staff(&hotel.superuser, &hotel.owner, "hk@example.com", "housekeeping").await;
    let intruder = app.signup_owner("intruder@example.com", "Other Hotels").await;
    // A member of the tenant who holds no role.
    let ungranted = app.signup_owner("nobody@example.com", "Own tenant").await;
    let user = uuid(&app.send(Method::GET, "/api/v1/me", Some(&ungranted), None).await.body["user_id"]);
    sqlx::query("insert into membership (tenant_id, user_id) values ($1, $2)")
        .bind(hotel.tenant)
        .bind(user)
        .execute(&hotel.superuser)
        .await
        .unwrap();
    let switched = app
        .send(Method::PUT, "/api/v1/session/tenant", Some(&ungranted), Some(json!({"tenant_id": hotel.tenant})))
        .await;
    assert_eq!(switched.status, StatusCode::OK, "{:?}", switched.body);

    let tape = graphql(&app, &housekeeping, TAPE, hotel.tape(&hotel.room_ids(), 0, 14)).await;
    let needing = graphql(&app, &housekeeping, UNASSIGNED, hotel.needing_rooms(0, 14)).await;

    assert_eq!(tape["data"]["tapeWindow"]["stays"].as_array().map(Vec::len), Some(1), "{tape:?}");
    assert_eq!(needing["data"]["unassignedStays"].as_array().map(Vec::len), Some(1), "{needing:?}");
    let tape = graphql(&app, &ungranted, TAPE, hotel.tape(&hotel.room_ids(), 0, 14)).await;
    let needing = graphql(&app, &ungranted, UNASSIGNED, hotel.needing_rooms(0, 14)).await;
    for refused in [tape, needing] {
        assert_eq!(refused["errors"][0]["message"], "you do not have permission for this property", "{refused:?}");
        assert_eq!(refused["data"], Value::Null, "{refused:?}");
    }
    // Another tenant's session reads nothing of this property: its rooms are not rooms it can see.
    let tape = graphql(&app, &intruder, TAPE, hotel.tape(&hotel.room_ids(), 0, 14)).await;
    let needing = graphql(&app, &intruder, UNASSIGNED, hotel.needing_rooms(0, 14)).await;
    assert_eq!(tape["errors"][0]["message"], "every room must belong to the property", "{tape:?}");
    assert_eq!(tape["data"], Value::Null, "{tape:?}");
    assert_eq!(needing["data"]["unassignedStays"], json!([]), "{needing:?}");
}
