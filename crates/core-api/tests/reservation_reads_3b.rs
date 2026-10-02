mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, TestResponse, uuid};
use db::{Scope, TenantId};
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

/// A reservation room command (`…/assign`, `…/check-in`, …) with `If-Match: "<version>"` and an optional body.
async fn command(app: &TestApp, cookie: &str, path: &str, version: i64, body: Option<Value>) -> TestResponse {
    let if_match = format!("\"{version}\"");
    app.send_with(Method::POST, path, Some(cookie), body, &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)]).await
}

async fn graphql(app: &TestApp, cookie: &str, query: &str, variables: Value) -> Value {
    let response =
        app.send(Method::POST, "/graphql", Some(cookie), Some(json!({"query": query, "variables": variables}))).await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
    response.body
}

const ACCOUNTS: &str = "query ($p: UUID!, $search: String, $includeInactive: Boolean = false, $first: Int) {
    accounts(propertyId: $p, search: $search, includeInactive: $includeInactive, first: $first) {
        id kind name contact { email phone address contactName } creditLimit currency active version
    }
}";

const DETAIL: &str = "query ($p: UUID!, $id: UUID!) {
    reservation(propertyId: $p, id: $id) {
        account { id name kind }
        rooms {
            id status version
            occupants { id firstName lastName idDocType idDocMasked }
            checkedInAt checkedInBusinessDate checkedOutAt
            canCheckIn canUndoCheckIn canCheckOut
        }
    }
}";

const LIST: &str = "query ($p: UUID!) {
    reservations(propertyId: $p, first: 20) {
        nodes { id confirmationNo accountName }
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

    fn reservations(&self) -> String {
        format!("{}/reservations", self.path)
    }

    fn accounts(&self) -> String {
        format!("{}/accounts", self.path)
    }

    async fn guest(&self, app: &TestApp, first: &str, last: &str) -> Value {
        let guest = json!({"first_name": first, "last_name": last, "residency": "non_resident"});
        post(app, &self.owner, &format!("{}/guests", self.path), guest).await.body
    }

    /// A guest with an ID document, so a caller can prove it stays masked once it becomes an occupant.
    async fn guest_with_id_doc(&self, app: &TestApp, first: &str, last: &str, number: &str) -> Value {
        let guest = json!({"first_name": first, "last_name": last, "residency": "non_resident",
                           "id_doc": {"type": "passport", "number": number}});
        post(app, &self.owner, &format!("{}/guests", self.path), guest).await.body
    }

    async fn account(&self, app: &TestApp, name: &str, kind: &str) -> Value {
        let account = json!({"kind": kind, "name": name, "currency": "USD"});
        post(app, &self.owner, &self.accounts(), account).await.body
    }

    /// One deluxe room on BAR, room only, for two adults over `[business date + from, business date + to)`.
    fn booking(&self, guest: &Value, from: i64, to: i64) -> Value {
        json!({"booker_guest_id": guest["id"], "source": "front_desk",
               "rooms": [{"room_type_id": self.rooms[0]["room_type_id"], "rate_plan_id": self.bar,
                          "meal_plan": "RO", "check_in": self.day(from), "check_out": self.day(to), "adults": 2}]})
    }

    async fn book(&self, app: &TestApp, guest: &Value, from: i64, to: i64) -> Value {
        post(app, &self.owner, &self.reservations(), self.booking(guest, from, to)).await.body
    }

    /// `book`, then takes the booked room out of the room booking auto-assigned it, for tests that start from
    /// an unassigned stay. The result shows no room and the room's version after the unassign.
    async fn book_unassigned(&self, app: &TestApp, guest: &Value, from: i64, to: i64) -> Value {
        let mut created = self.book(app, guest, from, to).await;
        let version = created["rooms"][0]["version"].as_i64().unwrap();
        let freed = command(app, &self.owner, &format!("{}/unassign", self.stay(&created)), version, None).await;
        assert_eq!(freed.status, StatusCode::OK, "{:?}", freed.body);
        created["rooms"][0]["version"] = json!(version + 1);
        created["rooms"][0]["room_id"] = Value::Null;
        created["rooms"][0]["room_number"] = Value::Null;
        created
    }

    /// The path of a reservation's first room, created over REST.
    fn stay(&self, created: &Value) -> String {
        format!("{}/reservation-rooms/{}", self.path, created["rooms"][0]["id"].as_str().unwrap())
    }

    /// Moves the business date to `business date + days` and extends the counter window to match, as the
    /// night audit would.
    async fn advance(&self, app: &TestApp, days: i64) {
        sqlx::query("update property set business_date = $2 where id = $1")
            .bind(self.id)
            .bind(self.business_date + Duration::days(days))
            .execute(&self.superuser)
            .await
            .unwrap();
        let mut tx = db::begin(&app.pool, Scope::tenant(TenantId(self.tenant))).await.unwrap();
        rooms::extend_window(&mut tx, self.id).await.unwrap();
        tx.commit().await.unwrap();
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_accounts_query_searches_filters_inactive_rows_and_is_isolated_by_tenant(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    hotel.account(&app, "Acme Travel", "company").await;
    hotel.account(&app, "Beta Corp", "travel_agent").await;
    let gamma = hotel.account(&app, "Gamma Tours", "company").await;
    let deactivated = patch(
        &app,
        &hotel.owner,
        &format!("{}/{}", hotel.accounts(), gamma["id"].as_str().unwrap()),
        1,
        json!({"active": false}),
    )
    .await;
    assert_eq!(deactivated.status, StatusCode::OK, "{:?}", deactivated.body);

    let names = |response: &Value| -> Vec<String> {
        response["data"]["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|account| account["name"].as_str().unwrap().to_owned())
            .collect()
    };

    let default = graphql(&app, &hotel.owner, ACCOUNTS, json!({"p": hotel.id})).await;
    assert_eq!(names(&default), vec!["Acme Travel", "Beta Corp"], "inactive excluded by default, by name");

    let with_inactive = graphql(&app, &hotel.owner, ACCOUNTS, json!({"p": hotel.id, "includeInactive": true})).await;
    assert_eq!(names(&with_inactive), vec!["Acme Travel", "Beta Corp", "Gamma Tours"]);

    let searched = graphql(&app, &hotel.owner, ACCOUNTS, json!({"p": hotel.id, "search": "acme"})).await;
    assert_eq!(names(&searched), vec!["Acme Travel"], "case-insensitive substring");

    let searched_inactive = graphql(&app, &hotel.owner, ACCOUNTS, json!({"p": hotel.id, "search": "gamma"})).await;
    assert_eq!(names(&searched_inactive), Vec::<String>::new(), "inactive stays excluded even when it matches");

    let shape = &default["data"]["accounts"][0];
    assert_eq!(
        (shape["kind"].clone(), shape["contact"].clone(), shape["creditLimit"].clone()),
        (json!("COMPANY"), json!({"email": null, "phone": null, "address": null, "contactName": null}), Value::Null)
    );
    assert_eq!(
        (shape["currency"].clone(), shape["active"].clone(), shape["version"].clone()),
        (json!("USD"), json!(true), json!(1))
    );

    let capped = graphql(&app, &hotel.owner, ACCOUNTS, json!({"p": hotel.id, "first": 1})).await;
    assert_eq!(capped["data"]["accounts"].as_array().map(Vec::len), Some(1));
    let none = graphql(&app, &hotel.owner, ACCOUNTS, json!({"p": hotel.id, "first": 0})).await;
    assert_eq!(none["errors"][0]["message"], "first is 1 to 100");
    let too_many = graphql(&app, &hotel.owner, ACCOUNTS, json!({"p": hotel.id, "first": 101})).await;
    assert_eq!(too_many["errors"][0]["message"], "first is 1 to 100");

    let intruder = app.signup_owner("intruder@example.com", "Other Hotels").await;
    let isolated = graphql(&app, &intruder, ACCOUNTS, json!({"p": hotel.id})).await;
    assert_eq!(isolated["data"]["accounts"], json!([]), "another tenant sees nothing, not even an error");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_detail_s_occupants_and_check_in_flags_track_a_room_through_a_stay(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let booker = hotel.guest(&app, "Ada", "Silva").await;
    let occupant = hotel.guest_with_id_doc(&app, "Ben", "Perera", "N9988776").await;
    let query = |id: &str| json!({"p": hotel.id, "id": id});

    // Booked today, but not yet assigned: none of the three actions are offered.
    let created = hotel.book_unassigned(&app, &booker, 0, 2).await;
    let stay = hotel.stay(&created);
    let reservation_id = created["id"].as_str().unwrap();

    let unassigned = graphql(&app, &hotel.owner, DETAIL, query(reservation_id)).await;
    let room = &unassigned["data"]["reservation"]["rooms"][0];
    assert_eq!(
        (room["canCheckIn"].clone(), room["canUndoCheckIn"].clone(), room["canCheckOut"].clone()),
        (json!(false), json!(false), json!(false)),
        "unassigned: {room:?}"
    );
    assert_eq!(room["occupants"], json!([]));

    // A second room, arriving later: assigned but not arriving today, so check-in still isn't offered.
    let future = hotel.book_unassigned(&app, &booker, 5, 7).await;
    command(
        &app,
        &hotel.owner,
        &format!("{}/assign", hotel.stay(&future)),
        2,
        Some(json!({"room_id": hotel.rooms[1]["id"]})),
    )
    .await;
    let future_detail = graphql(&app, &hotel.owner, DETAIL, query(future["id"].as_str().unwrap())).await;
    assert_eq!(future_detail["data"]["reservation"]["rooms"][0]["canCheckIn"], false, "arrives in five days");

    // Assigned and an occupant added: check-in becomes possible; the occupant shows up masked.
    let assigned =
        command(&app, &hotel.owner, &format!("{stay}/assign"), 2, Some(json!({"room_id": hotel.rooms[0]["id"]}))).await;
    assert_eq!(assigned.status, StatusCode::OK, "{:?}", assigned.body);
    let added =
        command(&app, &hotel.owner, &format!("{stay}/guests"), 3, Some(json!({"guest_id": occupant["id"]}))).await;
    assert_eq!(added.status, StatusCode::OK, "{:?}", added.body);

    let ready = graphql(&app, &hotel.owner, DETAIL, query(reservation_id)).await;
    let room = &ready["data"]["reservation"]["rooms"][0];
    assert_eq!(
        (room["canCheckIn"].clone(), room["canUndoCheckIn"].clone(), room["canCheckOut"].clone()),
        (json!(true), json!(false), json!(false)),
        "assigned and arriving today: {room:?}"
    );
    let raw = ready.to_string();
    assert!(!raw.contains("9988776"), "the occupant's ID number never leaves the database: {raw}");
    assert_eq!(
        (
            room["occupants"][0]["lastName"].clone(),
            room["occupants"][0]["idDocType"].clone(),
            room["occupants"][0]["idDocMasked"].clone()
        ),
        (json!("Perera"), json!("PASSPORT"), json!("•••• 8776"))
    );

    // Checked in today: undo is offered, and so is check-out (always true once checked in).
    let checked_in = command(&app, &hotel.owner, &format!("{stay}/check-in"), 4, None).await;
    assert_eq!(checked_in.status, StatusCode::OK, "{:?}", checked_in.body);
    let in_house = graphql(&app, &hotel.owner, DETAIL, query(reservation_id)).await;
    let room = &in_house["data"]["reservation"]["rooms"][0];
    assert_eq!(room["status"], "CHECKED_IN");
    assert!(room["checkedInAt"].is_string(), "{room:?}");
    assert_eq!(room["checkedInBusinessDate"], hotel.day(0));
    assert_eq!(room["checkedOutAt"], Value::Null);
    assert_eq!(
        (room["canCheckIn"].clone(), room["canUndoCheckIn"].clone(), room["canCheckOut"].clone()),
        (json!(false), json!(true), json!(true)),
        "checked in today: {room:?}"
    );

    // The business date moves on: still checked in, but undo is no longer offered.
    hotel.advance(&app, 1).await;
    let moved_on = graphql(&app, &hotel.owner, DETAIL, query(reservation_id)).await;
    let room = &moved_on["data"]["reservation"]["rooms"][0];
    assert_eq!(
        (room["canCheckIn"].clone(), room["canUndoCheckIn"].clone(), room["canCheckOut"].clone()),
        (json!(false), json!(false), json!(true)),
        "a day later: {room:?}"
    );

    // Checked out (early, since the business date is still before the booked departure): nothing more to do.
    let checked_out = command(&app, &hotel.owner, &format!("{stay}/check-out"), 5, None).await;
    assert_eq!(checked_out.status, StatusCode::OK, "{:?}", checked_out.body);
    let departed = graphql(&app, &hotel.owner, DETAIL, query(reservation_id)).await;
    let room = &departed["data"]["reservation"]["rooms"][0];
    assert_eq!(room["status"], "CHECKED_OUT");
    assert!(room["checkedOutAt"].is_string(), "{room:?}");
    assert_eq!(
        (room["canCheckIn"].clone(), room["canUndoCheckIn"].clone(), room["canCheckOut"].clone()),
        (json!(false), json!(false), json!(false)),
        "checked out: {room:?}"
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_reservation_s_account_and_the_list_s_account_name_agree(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let account = hotel.account(&app, "Acme Travel", "company").await;
    let guest = hotel.guest(&app, "Ada", "Silva").await;

    let mut billed = hotel.booking(&guest, 0, 2);
    billed["account_id"] = account["id"].clone();
    let billed = post(&app, &hotel.owner, &hotel.reservations(), billed).await.body;
    let unbilled = hotel.book(&app, &guest, 3, 5).await;

    let detail = graphql(&app, &hotel.owner, DETAIL, json!({"p": hotel.id, "id": billed["id"]})).await;
    assert_eq!(
        detail["data"]["reservation"]["account"],
        json!({"id": account["id"], "name": "Acme Travel", "kind": "COMPANY"})
    );
    let unbilled_detail = graphql(&app, &hotel.owner, DETAIL, json!({"p": hotel.id, "id": unbilled["id"]})).await;
    assert_eq!(unbilled_detail["data"]["reservation"]["account"], Value::Null);

    let list = graphql(&app, &hotel.owner, LIST, json!({"p": hotel.id})).await;
    let nodes = list["data"]["reservations"]["nodes"].as_array().unwrap();
    let by_room = |room_id: &str| nodes.iter().find(|node| node["id"] == room_id).unwrap();
    assert_eq!(by_room(billed["rooms"][0]["id"].as_str().unwrap())["accountName"], "Acme Travel");
    assert_eq!(by_room(unbilled["rooms"][0]["id"].as_str().unwrap())["accountName"], Value::Null);
}
