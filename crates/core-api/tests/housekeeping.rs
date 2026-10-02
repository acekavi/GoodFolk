mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, TestResponse, uuid};
use core_api::events::{LiveEvent, spawn_listener};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::time::Duration;
use time::{Date, Duration as Days, format_description::well_known::Iso8601};
use uuid::Uuid;

async fn post(app: &TestApp, cookie: &str, path: &str, key: &str, body: Value) -> TestResponse {
    app.send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", key)])
        .await
}

/// A request with `If-Match: "<version>"`.
async fn versioned(app: &TestApp, method: Method, cookie: &str, path: &str, version: i64, body: Value) -> TestResponse {
    let if_match = format!("\"{version}\"");
    app.send_with(method, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)]).await
}

async fn graphql(app: &TestApp, cookie: &str, query: &str, variables: Value) -> Value {
    let response =
        app.send(Method::POST, "/graphql", Some(cookie), Some(json!({"query": query, "variables": variables}))).await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
    response.body
}

const BOARD: &str = "query ($p: UUID!) {
    housekeepingBoard(propertyId: $p) {
        roomId number condition conditionVersion occupancy openIssues blockKind assignee { userId displayName }
    }
}";
const MY_ROOMS: &str = "query ($p: UUID!) { myRooms(propertyId: $p) { roomId number condition conditionVersion } }";
const ISSUES: &str = "query ($p: UUID!, $status: IssueStatus) {
    roomIssues(propertyId: $p, status: $status) { id roomId severity description status reportedBy version }
}";
const HOUSEKEEPERS: &str = "query ($p: UUID!) { housekeepers(propertyId: $p) { userId displayName } }";

async fn put(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    app.send(Method::PUT, path, Some(cookie), Some(body)).await
}

/// A refusal by permission: 403 as a problem document.
fn assert_forbidden(response: &TestResponse, context: &str) {
    assert_eq!(response.status, StatusCode::FORBIDDEN, "{context}: {:?}", response.body);
    assert_eq!(response.body["type"], "about:blank", "{context}: {:?}", response.body);
    assert_eq!(response.headers["content-type"], "application/problem+json", "{context}");
}

/// A property with one room, set up by `cookie`; returns its id and its room.
async fn property_with_room(app: &TestApp, cookie: &str, code: &str) -> (Uuid, Uuid) {
    let key = || Uuid::now_v7().to_string();
    let body = json!({"code": code, "name": code, "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let id = uuid(&post(app, cookie, "/api/v1/properties", &key(), body).await.body["id"]);
    let path = format!("/api/v1/properties/{id}");
    let deluxe = json!({"code": "DLX", "name": "Deluxe", "base_occupancy": 2, "max_adults": 2,
                        "max_children": 1, "max_occupancy": 3});
    let deluxe = uuid(&post(app, cookie, &format!("{path}/room-types"), &key(), deluxe).await.body["id"]);
    let range = json!({"room_type_id": deluxe, "first": 201, "last": 201});
    let rooms = post(app, cookie, &format!("{path}/rooms/bulk"), &key(), range).await.body;
    (id, uuid(&rooms[0]["id"]))
}

/// Waits up to 5 seconds for an invalidation naming `key`.
async fn expect_event(events: &mut tokio::sync::broadcast::Receiver<LiveEvent>, key: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let received = tokio::time::timeout(remaining, events.recv())
            .await
            .unwrap_or_else(|_| panic!("no invalidation named {key} arrived in time"))
            .unwrap();
        if let LiveEvent::Invalidate(event) = received
            && event.keys.iter().any(|k| k == key)
        {
            return;
        }
    }
}

/// A property with two rooms (101, 102), set up by its owner, and one staff member per role.
struct Hotel {
    owner: String,
    path: String,
    id: Uuid,
    rooms: Vec<Uuid>,
    deluxe: Uuid,
    business_date: Date,
    manager: String,
    housekeeping: String,
    supervisor: String,
    front_desk: String,
    accountant: String,
    superuser: PgPool,
}

impl Hotel {
    async fn new(app: &TestApp, opts: PgConnectOptions) -> Self {
        let superuser = PgPool::connect_with(opts).await.unwrap();
        let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
        let key = || Uuid::now_v7().to_string();
        let property = json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"});
        let property = post(app, &owner, "/api/v1/properties", &key(), property).await.body;
        let id = uuid(&property["id"]);
        let business_date = Date::parse(property["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
        let path = format!("/api/v1/properties/{id}");
        let deluxe = json!({"code": "DLX", "name": "Deluxe", "base_occupancy": 2, "max_adults": 2,
                            "max_children": 1, "max_occupancy": 3});
        let deluxe = uuid(&post(app, &owner, &format!("{path}/room-types"), &key(), deluxe).await.body["id"]);
        let range = json!({"room_type_id": deluxe, "first": 101, "last": 102});
        let rooms = post(app, &owner, &format!("{path}/rooms/bulk"), &key(), range).await.body;
        let rooms = rooms.as_array().unwrap().iter().map(|room| uuid(&room["id"])).collect();
        let manager = app.staff(&superuser, &owner, "mgr@example.com", "manager").await;
        let housekeeping = app.staff(&superuser, &owner, "hk@example.com", "housekeeping").await;
        let supervisor = app.staff(&superuser, &owner, "sup@example.com", "hk_supervisor").await;
        let front_desk = app.staff(&superuser, &owner, "desk@example.com", "front_desk").await;
        let accountant = app.staff(&superuser, &owner, "acc@example.com", "accountant").await;
        Self {
            owner,
            path,
            id,
            rooms,
            deluxe,
            business_date,
            manager,
            housekeeping,
            supervisor,
            front_desk,
            accountant,
            superuser,
        }
    }

    async fn count(&self, sql: &'static str, id: Uuid) -> i64 {
        sqlx::query_scalar(sql).bind(id).fetch_one(&self.superuser).await.unwrap()
    }

    /// Reports an issue in the first room as the owner.
    async fn issue(&self, app: &TestApp) -> Value {
        let body = json!({"room_id": self.rooms[0], "severity": "low", "description": "Scuffed wall"});
        let reported =
            post(app, &self.owner, &format!("{}/room-issues", self.path), &Uuid::now_v7().to_string(), body).await;
        assert_eq!(reported.status, StatusCode::CREATED, "{:?}", reported.body);
        reported.body
    }

    async fn user_id(&self, app: &TestApp, cookie: &str) -> Uuid {
        uuid(&app.send(Method::GET, "/api/v1/me", Some(cookie), None).await.body["user_id"])
    }

    fn condition_path(&self, room: Uuid) -> String {
        format!("{}/rooms/{room}/condition", self.path)
    }

    fn assignments(&self) -> String {
        format!("{}/hk-assignments", self.path)
    }

    async fn board(&self, app: &TestApp, cookie: &str) -> Value {
        let board = graphql(app, cookie, BOARD, json!({"p": self.id})).await;
        assert_eq!(board["errors"], Value::Null, "{board:?}");
        board["data"]["housekeepingBoard"].clone()
    }

    async fn condition_version(&self, app: &TestApp, room: Uuid) -> i64 {
        let board = self.board(app, &self.owner).await;
        let room = board.as_array().unwrap().iter().find(|r| r["roomId"] == json!(room)).unwrap().clone();
        room["conditionVersion"].as_i64().unwrap()
    }

    async fn assign(&self, app: &TestApp, room: Uuid, user: Uuid) {
        let body = json!({"assignments": [{"room_id": room, "user_id": user}]});
        let assigned = app.send(Method::PUT, &self.assignments(), Some(&self.owner), Some(body)).await;
        assert_eq!(assigned.status, StatusCode::NO_CONTENT, "{:?}", assigned.body);
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_supervisor_commands_each_succeed(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let (first, second) = (hotel.rooms[0], hotel.rooms[1]);
    let housekeeper = hotel.user_id(&app, &hotel.housekeeping).await;

    let version = hotel.condition_version(&app, first).await;
    let set = versioned(
        &app,
        Method::PUT,
        &hotel.supervisor,
        &hotel.condition_path(first),
        version,
        json!({"condition": "inspected"}),
    )
    .await;
    assert_eq!(set.status, StatusCode::OK, "{:?}", set.body);
    assert_eq!(set.body["condition"], "inspected");
    assert_eq!(set.body["version"], version + 1);
    assert_eq!(set.headers["etag"], format!("\"{}\"", version + 1));

    let bulk = post(
        &app,
        &hotel.supervisor,
        &format!("{}/rooms/conditions", hotel.path),
        &Uuid::now_v7().to_string(),
        json!({"room_ids": [first, second], "condition": "dirty"}),
    )
    .await;
    assert_eq!(bulk.status, StatusCode::OK, "{:?}", bulk.body);
    assert_eq!(bulk.body.as_array().unwrap().len(), 2);
    assert!(bulk.body.as_array().unwrap().iter().all(|row| row["condition"] == "dirty"));

    hotel.assign(&app, first, housekeeper).await;
    let board = hotel.board(&app, &hotel.owner).await;
    assert_eq!(board[0]["assignee"]["userId"], json!(housekeeper));
    assert_eq!(board[1]["assignee"], Value::Null);

    let unassigned = json!({"assignments": [{"room_id": first, "user_id": null}]});
    let cleared = app.send(Method::PUT, &hotel.assignments(), Some(&hotel.supervisor), Some(unassigned)).await;
    assert_eq!(cleared.status, StatusCode::NO_CONTENT, "{:?}", cleared.body);

    let copied = post(
        &app,
        &hotel.supervisor,
        &format!("{}/copy-yesterday", hotel.assignments()),
        &Uuid::now_v7().to_string(),
        json!({}),
    )
    .await;
    assert_eq!(copied.status, StatusCode::OK, "{:?}", copied.body);
    assert_eq!(copied.body, json!({"copied": 0}));

    let issues = format!("{}/room-issues", hotel.path);
    let reported = post(
        &app,
        &hotel.housekeeping,
        &issues,
        &Uuid::now_v7().to_string(),
        json!({"room_id": first, "severity": "high", "description": "Leaking tap"}),
    )
    .await;
    assert_eq!(reported.status, StatusCode::CREATED, "{:?}", reported.body);
    assert_eq!(reported.body["status"], "open");
    let issue = reported.body["id"].as_str().unwrap();
    let resolved = versioned(
        &app,
        Method::PATCH,
        &hotel.supervisor,
        &format!("{issues}/{issue}"),
        reported.body["version"].as_i64().unwrap(),
        json!({"status": "resolved"}),
    )
    .await;
    assert_eq!(resolved.status, StatusCode::OK, "{:?}", resolved.body);
    assert_eq!(resolved.body["status"], "resolved");
    let again = versioned(
        &app,
        Method::PATCH,
        &hotel.supervisor,
        &format!("{issues}/{issue}"),
        resolved.body["version"].as_i64().unwrap(),
        json!({"status": "resolved"}),
    )
    .await;
    assert_eq!(again.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", again.body);
    let reopened = versioned(
        &app,
        Method::PATCH,
        &hotel.supervisor,
        &format!("{issues}/{issue}"),
        resolved.body["version"].as_i64().unwrap(),
        json!({"status": "open"}),
    )
    .await;
    assert_eq!(reopened.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", reopened.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_housekeeper_takes_their_own_room_one_step_at_a_time(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let (mine, other) = (hotel.rooms[0], hotel.rooms[1]);
    let me = hotel.user_id(&app, &hotel.housekeeping).await;
    hotel.assign(&app, mine, me).await;
    let bulk = format!("{}/rooms/conditions", hotel.path);
    post(
        &app,
        &hotel.owner,
        &bulk,
        &Uuid::now_v7().to_string(),
        json!({"room_ids": [mine, other], "condition": "dirty"}),
    )
    .await;
    let version = hotel.condition_version(&app, mine).await;
    let path = hotel.condition_path(mine);

    let skipped =
        versioned(&app, Method::PUT, &hotel.housekeeping, &path, version, json!({"condition": "clean"})).await;
    let inspected =
        versioned(&app, Method::PUT, &hotel.housekeeping, &path, version, json!({"condition": "inspected"})).await;
    let not_theirs = versioned(
        &app,
        Method::PUT,
        &hotel.housekeeping,
        &hotel.condition_path(other),
        hotel.condition_version(&app, other).await,
        json!({"condition": "cleaning"}),
    )
    .await;
    let stale =
        versioned(&app, Method::PUT, &hotel.housekeeping, &path, version - 1, json!({"condition": "cleaning"})).await;
    let started =
        versioned(&app, Method::PUT, &hotel.housekeeping, &path, version, json!({"condition": "cleaning"})).await;

    assert_eq!(skipped.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", skipped.body);
    assert_eq!(inspected.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", inspected.body);
    assert_eq!(not_theirs.status, StatusCode::FORBIDDEN, "{:?}", not_theirs.body);
    assert_eq!(stale.status, StatusCode::PRECONDITION_FAILED, "{:?}", stale.body);
    assert_eq!(started.status, StatusCode::OK, "{:?}", started.body);
    assert_eq!(started.body["condition"], "cleaning");
    let missing = app
        .send_with(
            Method::PUT,
            &path,
            Some(&hotel.housekeeping),
            Some(json!({"condition": "clean"})),
            &[("x-goodfolk-csrf", "1")],
        )
        .await;
    assert_eq!(missing.status, StatusCode::PRECONDITION_REQUIRED);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_retried_bulk_change_applies_once(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let path = format!("{}/rooms/conditions", hotel.path);
    let body = json!({"room_ids": hotel.rooms, "condition": "dirty"});
    let key = Uuid::now_v7().to_string();

    let first = post(&app, &hotel.supervisor, &path, &key, body.clone()).await;
    let retry = post(&app, &hotel.supervisor, &path, &key, body).await;

    assert_eq!(first.status, StatusCode::OK, "{:?}", first.body);
    assert_eq!(retry.status, StatusCode::OK, "{:?}", retry.body);
    assert_eq!(retry.body, first.body);
    let changes: i64 =
        sqlx::query_scalar("select count(*) from audit_log where action = 'room.condition_changed' and entity_id = $1")
            .bind(hotel.rooms[0])
            .fetch_one(&hotel.superuser)
            .await
            .unwrap();
    assert_eq!(changes, 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_reads_follow_each_role_and_my_rooms_are_the_callers_own(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let (mine, other) = (hotel.rooms[0], hotel.rooms[1]);
    let me = hotel.user_id(&app, &hotel.housekeeping).await;
    hotel.assign(&app, mine, me).await;
    let reported = post(
        &app,
        &hotel.front_desk,
        &format!("{}/room-issues", hotel.path),
        &Uuid::now_v7().to_string(),
        json!({"room_id": other, "severity": "medium", "description": "Light is out"}),
    )
    .await;
    assert_eq!(reported.status, StatusCode::CREATED, "{:?}", reported.body);
    let variables = json!({"p": hotel.id});

    for cookie in [&hotel.owner, &hotel.housekeeping, &hotel.supervisor, &hotel.front_desk] {
        let board = hotel.board(&app, cookie).await;
        assert_eq!(board.as_array().unwrap().len(), 2);
        assert_eq!(board[0]["number"], "101");
        assert_eq!(board[0]["condition"], "CLEAN");
        assert_eq!(board[0]["occupancy"], "VACANT");
        assert_eq!(board[0]["assignee"]["userId"], json!(me));
        assert_eq!(board[1]["openIssues"], 1);
        let issues = graphql(&app, cookie, ISSUES, json!({"p": hotel.id, "status": "OPEN"})).await;
        assert_eq!(issues["errors"], Value::Null, "{issues:?}");
        assert_eq!(issues["data"]["roomIssues"][0]["roomId"], json!(other));
        assert_eq!(issues["data"]["roomIssues"][0]["severity"], "MEDIUM");
        let housekeepers = graphql(&app, cookie, HOUSEKEEPERS, variables.clone()).await;
        assert_eq!(housekeepers["errors"], Value::Null, "{housekeepers:?}");
        let users: Vec<&Value> =
            housekeepers["data"]["housekeepers"].as_array().unwrap().iter().map(|h| &h["userId"]).collect();
        assert!(users.contains(&&json!(me)), "{housekeepers:?}");
    }
    let resolved = graphql(&app, &hotel.owner, ISSUES, json!({"p": hotel.id, "status": "RESOLVED"})).await;
    assert_eq!(resolved["data"]["roomIssues"], json!([]));

    let mine_only = graphql(&app, &hotel.housekeeping, MY_ROOMS, variables.clone()).await;
    assert_eq!(mine_only["errors"], Value::Null, "{mine_only:?}");
    assert_eq!(mine_only["data"]["myRooms"].as_array().unwrap().len(), 1);
    assert_eq!(mine_only["data"]["myRooms"][0]["roomId"], json!(mine));
    let none = graphql(&app, &hotel.supervisor, MY_ROOMS, variables.clone()).await;
    assert_eq!(none["data"]["myRooms"], json!([]));

    let refused = graphql(&app, &hotel.accountant, BOARD, variables.clone()).await;
    assert!(refused["errors"][0]["message"].is_string(), "{refused:?}");
    assert_eq!(refused["data"], Value::Null);
    for query in [MY_ROOMS, HOUSEKEEPERS, ISSUES] {
        let refused = graphql(&app, &hotel.accountant, query, variables.clone()).await;
        assert!(refused["errors"][0]["message"].is_string(), "{refused:?}");
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn every_role_gets_exactly_what_its_permissions_allow(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let (room, spare) = (hotel.rooms[0], hotel.rooms[1]);
    let housekeeper = hotel.user_id(&app, &hotel.housekeeping).await;
    hotel.assign(&app, room, housekeeper).await;
    let bulk_path = format!("{}/rooms/conditions", hotel.path);
    let issues = format!("{}/room-issues", hotel.path);

    // (role, may set a condition, may supervise, may report)
    let roles = [
        ("owner", &hotel.owner, true, true, true),
        ("manager", &hotel.manager, true, true, true),
        ("hk_supervisor", &hotel.supervisor, true, true, true),
        ("housekeeping", &hotel.housekeeping, true, false, true),
        ("front_desk", &hotel.front_desk, false, false, true),
        ("accountant", &hotel.accountant, false, false, false),
    ];
    for (name, cookie, may_update, may_supervise, may_report) in roles {
        let key = || Uuid::now_v7().to_string();
        let reset =
            post(&app, &hotel.owner, &bulk_path, &key(), json!({"room_ids": [room], "condition": "dirty"})).await;
        assert_eq!(reset.status, StatusCode::OK, "{name}: {:?}", reset.body);
        let version = hotel.condition_version(&app, room).await;
        let open_issue = hotel.issue(&app).await;
        let issue_path = format!("{issues}/{}", open_issue["id"].as_str().unwrap());

        let responses = [
            (
                "condition",
                may_update.then_some(StatusCode::OK),
                versioned(
                    &app,
                    Method::PUT,
                    cookie,
                    &hotel.condition_path(room),
                    version,
                    json!({"condition": "cleaning"}),
                )
                .await,
            ),
            (
                "bulk",
                may_supervise.then_some(StatusCode::OK),
                post(&app, cookie, &bulk_path, &key(), json!({"room_ids": [room], "condition": "dirty"})).await,
            ),
            (
                "assign",
                may_supervise.then_some(StatusCode::NO_CONTENT),
                put(&app, cookie, &hotel.assignments(), json!({"assignments": [{"room_id": spare, "user_id": null}]}))
                    .await,
            ),
            (
                "copy",
                may_supervise.then_some(StatusCode::OK),
                post(&app, cookie, &format!("{}/copy-yesterday", hotel.assignments()), &key(), json!({})).await,
            ),
            (
                "report",
                may_report.then_some(StatusCode::CREATED),
                post(
                    &app,
                    cookie,
                    &issues,
                    &key(),
                    json!({"room_id": room, "severity": "low", "description": "Stain"}),
                )
                .await,
            ),
            (
                "resolve",
                may_supervise.then_some(StatusCode::OK),
                versioned(&app, Method::PATCH, cookie, &issue_path, 1, json!({"status": "resolved"})).await,
            ),
        ];
        for (command, success, response) in responses {
            match success {
                Some(status) => assert_eq!(response.status, status, "{name} {command}: {:?}", response.body),
                None => assert_forbidden(&response, &format!("{name} {command}")),
            }
        }
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn commands_name_only_this_tenants_property_and_rooms(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let (_, other_room) = property_with_room(&app, &hotel.owner, "KAN").await;
    let stranger = app.signup_owner("stranger@example.com", "Other Co").await;
    let (foreign_property, foreign_room) = property_with_room(&app, &stranger, "COL").await;
    let room = hotel.rooms[0];
    let key = || Uuid::now_v7().to_string();
    let issue = hotel.issue(&app).await["id"].clone();

    // Every command against a property that does not exist, or belongs to another tenant, is 404.
    for property in [Uuid::now_v7(), foreign_property] {
        let path = format!("/api/v1/properties/{property}");
        let room = if property == foreign_property { foreign_room } else { room };
        let responses = [
            versioned(
                &app,
                Method::PUT,
                &hotel.owner,
                &format!("{path}/rooms/{room}/condition"),
                1,
                json!({"condition": "dirty"}),
            )
            .await,
            post(
                &app,
                &hotel.owner,
                &format!("{path}/rooms/conditions"),
                &key(),
                json!({"room_ids": [room], "condition": "dirty"}),
            )
            .await,
            put(
                &app,
                &hotel.owner,
                &format!("{path}/hk-assignments"),
                json!({"assignments": [{"room_id": room, "user_id": null}]}),
            )
            .await,
            post(&app, &hotel.owner, &format!("{path}/hk-assignments/copy-yesterday"), &key(), json!({})).await,
            post(
                &app,
                &hotel.owner,
                &format!("{path}/room-issues"),
                &key(),
                json!({"room_id": room, "severity": "low", "description": "x"}),
            )
            .await,
            versioned(
                &app,
                Method::PATCH,
                &hotel.owner,
                &format!("{path}/room-issues/{}", issue.as_str().unwrap()),
                1,
                json!({"status": "resolved"}),
            )
            .await,
        ];
        for response in responses {
            assert_eq!(response.status, StatusCode::NOT_FOUND, "{property}: {:?}", response.body);
        }
    }

    // A room of another property, in this tenant or another, is 404 and nothing is written.
    let before = hotel.board(&app, &hotel.owner).await;
    for foreign in [other_room, foreign_room] {
        let single = versioned(
            &app,
            Method::PUT,
            &hotel.owner,
            &hotel.condition_path(foreign),
            1,
            json!({"condition": "dirty"}),
        )
        .await;
        let bulk = post(
            &app,
            &hotel.owner,
            &format!("{}/rooms/conditions", hotel.path),
            &key(),
            json!({"room_ids": [hotel.rooms[0], foreign, hotel.rooms[1]], "condition": "dirty"}),
        )
        .await;
        let assigned = put(
            &app,
            &hotel.owner,
            &hotel.assignments(),
            json!({"assignments": [{"room_id": hotel.rooms[0], "user_id": null}, {"room_id": foreign, "user_id": null}]}),
        )
        .await;
        let reported = post(
            &app,
            &hotel.owner,
            &format!("{}/room-issues", hotel.path),
            &key(),
            json!({"room_id": foreign, "severity": "low", "description": "Not here"}),
        )
        .await;
        for response in [single, bulk, assigned, reported] {
            assert_eq!(response.status, StatusCode::NOT_FOUND, "{foreign}: {:?}", response.body);
        }
    }
    assert_eq!(hotel.board(&app, &hotel.owner).await, before, "a refused command changed nothing");
    let changes = "select count(*) from audit_log where entity_id = $1 and action in ('room.condition_changed', 'room.unassigned')";
    assert_eq!(hotel.count(changes, hotel.rooms[0]).await, 0);
    assert_eq!(hotel.count("select count(*) from room_issue where room_id = $1", other_room).await, 0);
    assert_eq!(hotel.count("select count(*) from room_issue where room_id = $1", foreign_room).await, 0);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn reads_and_commands_need_a_grant_for_the_property(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let (other_property, _) = property_with_room(&app, &hotel.owner, "KAN").await;
    let stranger = app.signup_owner("stranger@example.com", "Other Co").await;
    // A housekeeping user whose only grant is for the other property.
    let narrow = app.staff(&hotel.superuser, &hotel.owner, "narrow@example.com", "housekeeping").await;
    let user = hotel.user_id(&app, &narrow).await;
    sqlx::query("update role_grant set property_id = $1 where user_id = $2")
        .bind(other_property)
        .bind(user)
        .execute(&hotel.superuser)
        .await
        .unwrap();

    for query in [BOARD, MY_ROOMS, HOUSEKEEPERS, ISSUES] {
        // No grant for the property is an error.
        let refused = graphql(&app, &narrow, query, json!({"p": hotel.id})).await;
        assert!(refused["errors"][0]["message"].is_string(), "{refused:?}");
        assert_eq!(refused["data"], Value::Null, "{refused:?}");
        let granted = graphql(&app, &narrow, query, json!({"p": other_property})).await;
        assert_eq!(granted["errors"], Value::Null, "{granted:?}");
        // Another tenant's owner holds a grant in their own tenant, but row-level security shows them nothing.
        let hidden = graphql(&app, &stranger, query, json!({"p": hotel.id})).await;
        assert_eq!(hidden["errors"], Value::Null, "{hidden:?}");
        let rows = hidden["data"].as_object().unwrap().values().next().unwrap();
        assert_eq!(rows, &json!([]), "{hidden:?}");
    }
    let refused = put(
        &app,
        &narrow,
        &hotel.assignments(),
        json!({"assignments": [{"room_id": hotel.rooms[0], "user_id": null}]}),
    )
    .await;
    assert_forbidden(&refused, "narrow assignment");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_retried_command_replays_and_a_key_names_one_request(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let issues = format!("{}/room-issues", hotel.path);
    let body = json!({"room_id": hotel.rooms[0], "severity": "high", "description": "Leaking tap"});
    let key = Uuid::now_v7().to_string();

    let first = post(&app, &hotel.housekeeping, &issues, &key, body.clone()).await;
    let retry = post(&app, &hotel.housekeeping, &issues, &key, body.clone()).await;
    let other_body = json!({"room_id": hotel.rooms[0], "severity": "low", "description": "Another"});
    let reused = post(&app, &hotel.housekeeping, &issues, &key, other_body).await;
    let keyless = app.send(Method::POST, &issues, Some(&hotel.housekeeping), Some(body)).await;

    assert_eq!(first.status, StatusCode::CREATED, "{:?}", first.body);
    assert_eq!(retry.status, StatusCode::CREATED, "{:?}", retry.body);
    assert_eq!(retry.body, first.body);
    assert_eq!(retry.headers["etag"], first.headers["etag"]);
    assert_eq!(reused.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", reused.body);
    assert_eq!(keyless.status, StatusCode::BAD_REQUEST, "{:?}", keyless.body);
    assert_eq!(hotel.count("select count(*) from room_issue where room_id = $1", hotel.rooms[0]).await, 1);

    let copy = format!("{}/copy-yesterday", hotel.assignments());
    let key = Uuid::now_v7().to_string();
    let copied = post(&app, &hotel.supervisor, &copy, &key, json!({})).await;
    let replayed = post(&app, &hotel.supervisor, &copy, &key, json!({})).await;
    assert_eq!(replayed.status, copied.status);
    assert_eq!(replayed.body, copied.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_rules_of_each_command_are_problems(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let room = hotel.rooms[0];
    let key = || Uuid::now_v7().to_string();
    let issues = format!("{}/room-issues", hotel.path);
    let bulk_path = format!("{}/rooms/conditions", hotel.path);

    for description in [String::new(), "   ".into(), "x".repeat(2001)] {
        let body = json!({"room_id": room, "severity": "low", "description": description});
        let refused = post(&app, &hotel.owner, &issues, &key(), body).await;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", refused.body);
    }
    let long_enough = json!({"room_id": room, "severity": "low", "description": "x".repeat(2000)});
    assert_eq!(post(&app, &hotel.owner, &issues, &key(), long_enough).await.status, StatusCode::CREATED);
    let bad_severity = json!({"room_id": room, "severity": "urgent", "description": "x"});
    assert_eq!(post(&app, &hotel.owner, &issues, &key(), bad_severity).await.status, StatusCode::UNPROCESSABLE_ENTITY);

    let none: Vec<Uuid> = vec![];
    let too_many: Vec<Uuid> = (0..501).map(|_| Uuid::now_v7()).collect();
    for rooms in [none, too_many] {
        let refused =
            post(&app, &hotel.owner, &bulk_path, &key(), json!({"room_ids": rooms, "condition": "dirty"})).await;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", refused.body);
    }
    let bad_condition =
        post(&app, &hotel.owner, &bulk_path, &key(), json!({"room_ids": [room], "condition": "foo"})).await;
    assert_eq!(bad_condition.status, StatusCode::UNPROCESSABLE_ENTITY);
    let single =
        versioned(&app, Method::PUT, &hotel.owner, &hotel.condition_path(room), 1, json!({"condition": "foo"})).await;
    assert_eq!(single.status, StatusCode::UNPROCESSABLE_ENTITY);

    let accountant = hotel.user_id(&app, &hotel.accountant).await;
    let not_a_housekeeper = put(
        &app,
        &hotel.owner,
        &hotel.assignments(),
        json!({"assignments": [{"room_id": room, "user_id": accountant}]}),
    )
    .await;
    assert_eq!(not_a_housekeeper.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", not_a_housekeeper.body);
    let twice = put(
        &app,
        &hotel.owner,
        &hotel.assignments(),
        json!({"assignments": [{"room_id": room, "user_id": null}, {"room_id": room, "user_id": null}]}),
    )
    .await;
    assert_eq!(twice.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", twice.body);

    let issue = hotel.issue(&app).await;
    let issue_path = format!("{issues}/{}", issue["id"].as_str().unwrap());
    let resolve = json!({"status": "resolved"});
    let unversioned = app.send(Method::PATCH, &issue_path, Some(&hotel.owner), Some(resolve.clone())).await;
    let stale = versioned(&app, Method::PATCH, &hotel.owner, &issue_path, 5, resolve.clone()).await;
    let unknown =
        versioned(&app, Method::PATCH, &hotel.owner, &format!("{issues}/{}", Uuid::now_v7()), 1, resolve.clone()).await;
    let resolved = versioned(&app, Method::PATCH, &hotel.owner, &issue_path, 1, resolve).await;
    assert_eq!(unversioned.status, StatusCode::PRECONDITION_REQUIRED, "{:?}", unversioned.body);
    assert_eq!(stale.status, StatusCode::PRECONDITION_FAILED, "{:?}", stale.body);
    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{:?}", unknown.body);
    assert_eq!(resolved.status, StatusCode::OK, "{:?}", resolved.body);
    assert_eq!(issue["version"], 1);
    assert_eq!(resolved.headers["etag"], "\"2\"");
    let reported =
        post(&app, &hotel.owner, &issues, &key(), json!({"room_id": room, "severity": "low", "description": "y"}))
            .await;
    assert_eq!(reported.headers["etag"], "\"1\"");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_board_reads_occupancy_and_blocks(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let key = || Uuid::now_v7().to_string();
    let day = |offset: i64| (hotel.business_date + Days::days(offset)).to_string();
    let bar = json!({"code": "BAR", "name": "Best available", "kind": "standard", "segment": "IBE",
                     "currency": "USD", "room_type_ids": [hotel.deluxe]});
    let bar = uuid(&post(&app, &hotel.owner, &format!("{}/rate-plans", hotel.path), &key(), bar).await.body["id"]);
    let prices: Vec<Value> = (0..3)
        .map(|d| json!({"room_type_id": hotel.deluxe, "date": day(d), "occupancy": 2, "amount": 10_000}))
        .collect();
    let priced =
        put(&app, &hotel.owner, &format!("{}/rate-plans/{bar}/prices", hotel.path), json!({"prices": prices})).await;
    assert_eq!(priced.status, StatusCode::NO_CONTENT, "{:?}", priced.body);
    let guest = json!({"first_name": "Ada", "last_name": "Silva", "residency": "non_resident"});
    let guest = post(&app, &hotel.owner, &format!("{}/guests", hotel.path), &key(), guest).await.body;
    let booking = json!({"booker_guest_id": guest["id"], "source": "front_desk",
        "rooms": [{"room_type_id": hotel.deluxe, "rate_plan_id": bar, "meal_plan": "RO",
                   "check_in": day(0), "check_out": day(2), "adults": 2}]});
    let booked = post(&app, &hotel.owner, &format!("{}/reservations", hotel.path), &key(), booking).await;
    assert_eq!(booked.status, StatusCode::CREATED, "{:?}", booked.body);
    let board = hotel.board(&app, &hotel.owner).await;
    let arriving =
        board.as_array().unwrap().iter().find(|r| r["occupancy"] == "DUE_IN").expect("the booked room").clone();
    let vacant = board.as_array().unwrap().iter().find(|r| r["occupancy"] == "VACANT").expect("the free room").clone();

    let reason: Uuid = sqlx::query_scalar("select id from block_reason where code = 'MAINTENANCE'")
        .fetch_one(&hotel.superuser)
        .await
        .unwrap();
    let block = json!({"from": day(0), "to": day(1), "kind": "out_of_order", "reason_id": reason, "note": "Leak"});
    let blocked = post(
        &app,
        &hotel.owner,
        &format!("{}/rooms/{}/blocks", hotel.path, vacant["roomId"].as_str().unwrap()),
        &key(),
        block,
    )
    .await;
    assert_eq!(blocked.status, StatusCode::CREATED, "{:?}", blocked.body);

    let board = hotel.board(&app, &hotel.owner).await;
    let find = |id: &Value| board.as_array().unwrap().iter().find(|r| r["roomId"] == *id).unwrap().clone();
    assert_eq!(find(&arriving["roomId"])["occupancy"], "DUE_IN");
    assert_eq!(find(&arriving["roomId"])["blockKind"], Value::Null);
    assert_eq!(find(&vacant["roomId"])["occupancy"], "VACANT");
    assert_eq!(find(&vacant["roomId"])["blockKind"], "OUT_OF_ORDER");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn commands_tell_the_board_and_the_issues_list(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    spawn_listener(PgPool::connect_with(opts.clone()).await.unwrap(), app.state.events.clone()).await.unwrap();
    let hotel = Hotel::new(&app, opts).await;
    let mut events = app.state.events.subscribe();

    let bulk = post(
        &app,
        &hotel.supervisor,
        &format!("{}/rooms/conditions", hotel.path),
        &Uuid::now_v7().to_string(),
        json!({"room_ids": [hotel.rooms[0]], "condition": "dirty"}),
    )
    .await;
    assert_eq!(bulk.status, StatusCode::OK, "{:?}", bulk.body);
    expect_event(&mut events, &format!("hk:{}", hotel.id)).await;

    hotel.issue(&app).await;
    expect_event(&mut events, &format!("issues:{}", hotel.id)).await;
}
