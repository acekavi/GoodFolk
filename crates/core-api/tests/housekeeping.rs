mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, TestResponse, uuid};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
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

/// A property with two rooms (101, 102), set up by its owner, and one staff member per role.
struct Hotel {
    owner: String,
    path: String,
    id: Uuid,
    rooms: Vec<Uuid>,
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
        let id = uuid(&post(app, &owner, "/api/v1/properties", &key(), property).await.body["id"]);
        let path = format!("/api/v1/properties/{id}");
        let deluxe = json!({"code": "DLX", "name": "Deluxe", "base_occupancy": 2, "max_adults": 2,
                            "max_children": 1, "max_occupancy": 3});
        let deluxe = uuid(&post(app, &owner, &format!("{path}/room-types"), &key(), deluxe).await.body["id"]);
        let range = json!({"room_type_id": deluxe, "first": 101, "last": 102});
        let rooms = post(app, &owner, &format!("{path}/rooms/bulk"), &key(), range).await.body;
        let rooms = rooms.as_array().unwrap().iter().map(|room| uuid(&room["id"])).collect();
        let housekeeping = app.staff(&superuser, &owner, "hk@example.com", "housekeeping").await;
        let supervisor = app.staff(&superuser, &owner, "sup@example.com", "hk_supervisor").await;
        let front_desk = app.staff(&superuser, &owner, "desk@example.com", "front_desk").await;
        let accountant = app.staff(&superuser, &owner, "acc@example.com", "accountant").await;
        Self { owner, path, id, rooms, housekeeping, supervisor, front_desk, accountant, superuser }
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
async fn every_command_is_refused_to_roles_without_its_permission(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let room = hotel.rooms[0];
    let issue = post(
        &app,
        &hotel.owner,
        &format!("{}/room-issues", hotel.path),
        &Uuid::now_v7().to_string(),
        json!({"room_id": room, "severity": "low", "description": "Scuffed wall"}),
    )
    .await
    .body;
    let issue_path = format!("{}/room-issues/{}", hotel.path, issue["id"].as_str().unwrap());
    let report = json!({"room_id": room, "severity": "low", "description": "Stain"});
    let bulk = json!({"room_ids": [room], "condition": "dirty"});
    let assignment = json!({"assignments": [{"room_id": room, "user_id": null}]});
    let set = json!({"condition": "cleaning"});
    let resolve = json!({"status": "resolved"});

    // (role, may set a condition, may supervise, may report)
    let roles = [
        ("housekeeping", &hotel.housekeeping, true, false, true),
        ("front_desk", &hotel.front_desk, false, false, true),
        ("accountant", &hotel.accountant, false, false, false),
        ("hk_supervisor", &hotel.supervisor, true, true, true),
    ];
    for (name, cookie, may_update, may_supervise, may_report) in roles {
        let key = || Uuid::now_v7().to_string();
        let responses = [
            (
                "condition",
                may_update,
                versioned(&app, Method::PUT, cookie, &hotel.condition_path(room), 1, set.clone()).await,
            ),
            (
                "bulk",
                may_supervise,
                post(&app, cookie, &format!("{}/rooms/conditions", hotel.path), &key(), bulk.clone()).await,
            ),
            (
                "assign",
                may_supervise,
                app.send(Method::PUT, &hotel.assignments(), Some(cookie), Some(assignment.clone())).await,
            ),
            (
                "copy",
                may_supervise,
                post(&app, cookie, &format!("{}/copy-yesterday", hotel.assignments()), &key(), json!({})).await,
            ),
            (
                "report",
                may_report,
                post(&app, cookie, &format!("{}/room-issues", hotel.path), &key(), report.clone()).await,
            ),
            ("resolve", may_supervise, versioned(&app, Method::PATCH, cookie, &issue_path, 1, resolve.clone()).await),
        ];
        for (command, allowed, response) in responses {
            // A housekeeper's room that is not theirs is also 403, but the permission check words it differently.
            let by_permission = response.body["detail"] == "you do not have permission for this action";
            assert_eq!(by_permission, !allowed, "{name} {command}: {:?}", response.body);
        }
    }
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
    for query in [MY_ROOMS, HOUSEKEEPERS] {
        let refused = graphql(&app, &hotel.accountant, query, variables.clone()).await;
        assert!(refused["errors"][0]["message"].is_string(), "{refused:?}");
    }
}
