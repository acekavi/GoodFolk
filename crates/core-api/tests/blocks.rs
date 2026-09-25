mod common;

use axum::http::{Method, StatusCode, header};
use common::{TestApp, TestResponse};
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

async fn patch(app: &TestApp, cookie: &str, path: &str, version: i64, body: Value) -> TestResponse {
    let if_match = format!("\"{version}\"");
    app.send_with(Method::PATCH, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)])
        .await
}

/// A property with one room type and rooms 101 and 102, set up by its owner.
struct Hotel {
    owner: String,
    superuser: PgPool,
    path: String,
    business_date: Date,
    room_type: Uuid,
    rooms: Vec<Uuid>,
}

impl Hotel {
    async fn new(app: &TestApp, opts: PgConnectOptions) -> Self {
        let superuser = PgPool::connect_with(opts).await.unwrap();
        let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
        let property = json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"});
        let property = post(app, &owner, "/api/v1/properties", property).await.body;
        let path = format!("/api/v1/properties/{}", property["id"].as_str().unwrap());
        let business_date = Date::parse(property["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
        let room_type = json!({"code": "DLX", "name": "Deluxe", "base_occupancy": 2, "max_adults": 2,
                               "max_children": 0, "max_occupancy": 2});
        let room_type = post(app, &owner, &format!("{path}/room-types"), room_type).await.body["id"].clone();
        let range = json!({"room_type_id": room_type, "first": 101, "last": 102});
        let rooms = post(app, &owner, &format!("{path}/rooms/bulk"), range).await.body;
        let rooms = rooms.as_array().unwrap().iter().map(|room| common::uuid(&room["id"])).collect();
        Self { owner, superuser, path, business_date, room_type: common::uuid(&room_type), rooms }
    }

    fn day(&self, offset: i64) -> String {
        (self.business_date + Duration::days(offset)).to_string()
    }

    async fn reason(&self, code: &str) -> Uuid {
        sqlx::query_scalar("select id from block_reason where code = $1")
            .bind(code)
            .fetch_one(&self.superuser)
            .await
            .unwrap()
    }

    async fn block(&self, app: &TestApp, cookie: &str, room: usize, from: i64, to: i64) -> TestResponse {
        let body = json!({"from": self.day(from), "to": self.day(to), "kind": "out_of_order",
                          "reason_id": self.reason("MAINTENANCE").await, "note": "Leaking pipe"});
        post(app, cookie, &format!("{}/rooms/{}/blocks", self.path, self.rooms[room]), body).await
    }

    /// Rooms out of order on the business date plus `offset`.
    async fn out_of_order(&self, offset: i64) -> i32 {
        sqlx::query_scalar("select out_of_order from inventory_day where room_type_id = $1 and date = $2")
            .bind(self.room_type)
            .bind(self.business_date + Duration::days(offset))
            .fetch_one(&self.superuser)
            .await
            .unwrap()
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn front_desk_blocks_a_room_and_an_overlap_is_a_409_naming_the_block(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let front_desk = app.staff(&hotel.superuser, &hotel.owner, "desk@example.com", "front_desk").await;

    let created = hotel.block(&app, &front_desk, 0, 1, 4).await;
    let overlapping = hotel.block(&app, &front_desk, 0, 3, 5).await;
    let other_room = hotel.block(&app, &front_desk, 1, 3, 5).await;

    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    assert_eq!(created.headers[header::ETAG], "\"1\"");
    assert_eq!(
        (created.body["from"].as_str(), created.body["to"].as_str()),
        (Some(hotel.day(1).as_str()), Some(hotel.day(4).as_str()))
    );
    assert_eq!(overlapping.status, StatusCode::CONFLICT, "{:?}", overlapping.body);
    assert_eq!(overlapping.headers[header::CONTENT_TYPE], "application/problem+json");
    assert_eq!(overlapping.body["conflicts"][0]["id"], created.body["id"]);
    assert_eq!(overlapping.body["conflicts"].as_array().unwrap().len(), 1);
    assert_eq!(other_room.status, StatusCode::CREATED, "{:?}", other_room.body);
    assert_eq!((hotel.out_of_order(0).await, hotel.out_of_order(3).await, hotel.out_of_order(4).await), (0, 2, 1));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_block_cannot_start_before_the_business_date(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;

    let past = hotel.block(&app, &hotel.owner, 0, -1, 2).await;
    let backwards = hotel.block(&app, &hotel.owner, 0, 3, 1).await;

    assert_eq!(past.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", past.body);
    assert_eq!(backwards.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", backwards.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_block_ending_after_the_counter_window_is_a_422(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;

    let too_long = hotel.block(&app, &hotel.owner, 0, 0, 731).await;

    assert_eq!(too_long.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", too_long.body);
    assert_eq!(too_long.headers[header::CONTENT_TYPE], "application/problem+json");
    assert_eq!(too_long.body["detail"], "a block can end at most 730 days after the business date");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn releasing_a_block_early_restores_availability(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let block = hotel.block(&app, &hotel.owner, 0, 0, 5).await;
    let path = format!("{}/blocks/{}", hotel.path, block.body["id"].as_str().unwrap());

    let released = patch(&app, &hotel.owner, &path, 1, json!({"to": hotel.day(2)})).await;
    let stale = patch(&app, &hotel.owner, &path, 1, json!({"to": hotel.day(1)})).await;
    let in_the_past = patch(&app, &hotel.owner, &path, 2, json!({"to": hotel.day(-1)})).await;

    assert_eq!(released.status, StatusCode::OK, "{:?}", released.body);
    assert_eq!(released.headers[header::ETAG], "\"2\"");
    assert_eq!(released.body["to"].as_str(), Some(hotel.day(2).as_str()));
    assert_eq!((hotel.out_of_order(1).await, hotel.out_of_order(2).await), (1, 0));
    assert_eq!(stale.status, StatusCode::PRECONDITION_FAILED, "{:?}", stale.body);
    assert_eq!(in_the_past.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", in_the_past.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn blocking_and_reasons_follow_the_role_permissions(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let manager = app.staff(&hotel.superuser, &hotel.owner, "manager@example.com", "manager").await;
    let front_desk = app.staff(&hotel.superuser, &hotel.owner, "desk@example.com", "front_desk").await;
    let housekeeping = app.staff(&hotel.superuser, &hotel.owner, "hk@example.com", "housekeeping").await;
    let pest = json!({"code": "PEST", "label": "Pest control", "default_kind": "out_of_order"});

    let by_housekeeping = hotel.block(&app, &housekeeping, 0, 0, 1).await;
    let reason_by_desk = post(&app, &front_desk, &format!("{}/block-reasons", hotel.path), pest.clone()).await;
    let reason_by_manager = post(&app, &manager, &format!("{}/block-reasons", hotel.path), pest).await;
    let reason_path = format!("{}/block-reasons/{}", hotel.path, reason_by_manager.body["id"].as_str().unwrap());
    let retired = patch(&app, &manager, &reason_path, 1, json!({"active": false})).await;

    assert_eq!(by_housekeeping.status, StatusCode::FORBIDDEN);
    assert_eq!(reason_by_desk.status, StatusCode::FORBIDDEN);
    assert_eq!(reason_by_manager.status, StatusCode::CREATED, "{:?}", reason_by_manager.body);
    assert_eq!(retired.status, StatusCode::OK, "{:?}", retired.body);
    assert_eq!(retired.body["active"], false);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_tenants_rooms_types_sections_and_blocks_cannot_be_changed(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let section = post(&app, &hotel.owner, &format!("{}/sections", hotel.path), json!({"name": "East"})).await;
    let block = hotel.block(&app, &hotel.owner, 0, 1, 3).await;
    let (room, section, block) =
        (hotel.rooms[0], section.body["id"].as_str().unwrap(), block.body["id"].as_str().unwrap());
    let intruder = app.signup_owner("intruder@example.com", "Other Hotels").await;
    let own = json!({"code": "KAN", "name": "Kandy", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let own = format!(
        "/api/v1/properties/{}",
        post(&app, &intruder, "/api/v1/properties", own).await.body["id"].as_str().unwrap()
    );
    let new_block = json!({"from": hotel.day(5), "to": hotel.day(6), "kind": "out_of_order",
                           "reason_id": hotel.reason("MAINTENANCE").await});

    // Through the other tenant's property, and through the intruder's own property with the other tenant's ids.
    for property in [hotel.path.as_str(), own.as_str()] {
        let responses = [
            patch(&app, &intruder, &format!("{property}/rooms/{room}"), 1, json!({"floor": "9"})).await,
            patch(&app, &intruder, &format!("{property}/room-types/{}", hotel.room_type), 1, json!({"name": "X"}))
                .await,
            patch(&app, &intruder, &format!("{property}/sections/{section}"), 1, json!({"name": "X"})).await,
            patch(&app, &intruder, &format!("{property}/blocks/{block}"), 1, json!({"to": hotel.day(2)})).await,
            post(&app, &intruder, &format!("{property}/rooms/{room}/blocks"), new_block.clone()).await,
        ];
        for response in responses {
            assert_eq!(response.status, StatusCode::NOT_FOUND, "{property}: {:?}", response.body);
        }
    }
    let versions: (i32, i32, i32, i32, i64) = sqlx::query_as(
        "select (select version from room where id = $1), (select version from room_type where id = $2),
                (select version from housekeeping_section where id = $3::uuid),
                (select version from room_block where id = $4::uuid), (select count(*) from room_block)",
    )
    .bind(room)
    .bind(hotel.room_type)
    .bind(section)
    .bind(block)
    .fetch_one(&hotel.superuser)
    .await
    .unwrap();
    assert_eq!(versions, (1, 1, 1, 1, 1), "nothing of the other tenant changed");
}
