mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, TestResponse};
use core_api::graphql::build_schema;
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::{Date, Duration, format_description::well_known::Iso8601};
use uuid::Uuid;

async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    let key = Uuid::now_v7().to_string();
    let response = app
        .send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)])
        .await;
    assert!(response.status.is_success(), "{path}: {:?}", response.body);
    response
}

/// A reservation room command (`…/cancel`, `…/assign`) with `If-Match: "<version>"`.
async fn command(app: &TestApp, cookie: &str, path: &str, version: i64, body: Option<Value>) {
    let if_match = format!("\"{version}\"");
    let response = app
        .send_with(Method::POST, path, Some(cookie), body, &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)])
        .await;
    assert_eq!(response.status, StatusCode::OK, "{path}: {:?}", response.body);
}

async fn graphql(app: &TestApp, cookie: &str, query: &str, variables: Value) -> Value {
    let response =
        app.send(Method::POST, "/graphql", Some(cookie), Some(json!({"query": query, "variables": variables}))).await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
    response.body
}

/// The reservations table's query, as the SPA sends it.
const LIST: &str = "query ReservationList($p: UUID!, $filter: ReservationFilter, $sort: ReservationSort, $first: Int,
                                          $after: String, $withCount: Boolean!) {
    reservations(propertyId: $p, filter: $filter, sort: $sort, first: $first, after: $after) {
        nodes {
            id reservationId confirmationNo guestName arrival departure nights roomTypeCode roomNumber status source
            total currency version accountName
        }
        pageInfo { endCursor hasNextPage }
        totalCount @include(if: $withCount)
    }
}";

/// The reservation modal's query, as the SPA sends it.
const DETAIL: &str = "query Reservation($p: UUID!, $id: UUID!) {
    reservation(propertyId: $p, id: $id) {
        id confirmationNo status source notes createdAt version
        booker { id firstName lastName email phone country residency idDocType idDocMasked notes version }
        account { id name kind }
        totals { currency amount }
        rooms {
            id version status checkIn checkOut adults children mealPlan total currency
            roomType { id code name }
            room { id number }
            ratePlan { id code }
            primaryGuest { id firstName lastName residency idDocType idDocMasked }
            occupants { id firstName lastName residency idDocType idDocMasked }
            nights { date room meal }
            cancellationTerms { rules { daysBeforeArrival penalty { kind value } } noShow { kind value } }
            cancellationPenalty cancelledAt recordedPenalty
            checkedInAt checkedInBusinessDate checkedOutAt
            canCheckIn canUndoCheckIn canCheckOut
        }
        history { action at actorName data }
    }
}";

const AVAILABILITY: &str = "query ($p: UUID!, $in: Date!, $out: Date!) {
    availability(propertyId: $p, checkIn: $in, checkOut: $out, adults: 2, children: 0, residency: NON_RESIDENT) {
        roomTypeId code name free
        offers { ratePlanId ratePlanCode mealPlan total currency restrictionsOk violations { kind message }
                 nights { date room meal } }
    }
}";

const FREE_ROOMS: &str = "query ($p: UUID!, $type: UUID!, $in: Date!, $out: Date!) {
    freeRooms(propertyId: $p, roomTypeId: $type, checkIn: $in, checkOut: $out) { id number section }
}";

const GUESTS: &str = "query ($p: UUID!, $search: String) {
    guests(propertyId: $p, search: $search, first: 10) { id firstName lastName residency idDocType idDocMasked }
}";

/// A property with deluxe rooms 101 to 112 and standard rooms 201 and 202, sold on BAR (USD, room only or
/// breakfast) at 100.00 a night for two adults for 60 nights from the business date, with breakfast at 15.00
/// per adult, under a policy that charges the first night for cancelling 30 days or fewer before arrival.
struct Hotel {
    owner: String,
    superuser: PgPool,
    id: Uuid,
    path: String,
    business_date: Date,
    deluxe: Uuid,
    standard: Uuid,
    rooms: Vec<Value>,
    bar: Uuid,
}

impl Hotel {
    async fn new(app: &TestApp, opts: PgConnectOptions) -> Self {
        let superuser = PgPool::connect_with(opts).await.unwrap();
        let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
        let property = json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"});
        let property = post(app, &owner, "/api/v1/properties", property).await.body;
        let id = common::uuid(&property["id"]);
        let path = format!("/api/v1/properties/{id}");
        let business_date = Date::parse(property["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
        let mut types = Vec::new();
        let mut rooms = Vec::new();
        for (code, name, first, last) in [("DLX", "Deluxe", 101, 112), ("STD", "Standard", 201, 202)] {
            let room_type = json!({"code": code, "name": name, "base_occupancy": 2, "max_adults": 2,
                                   "max_children": 1, "max_occupancy": 3});
            let room_type = common::uuid(&post(app, &owner, &format!("{path}/room-types"), room_type).await.body["id"]);
            let range = json!({"room_type_id": room_type, "first": first, "last": last});
            rooms
                .extend(post(app, &owner, &format!("{path}/rooms/bulk"), range).await.body.as_array().unwrap().clone());
            types.push(room_type);
        }
        let policy = json!({"name": "Strict", "rules": [{"days_before_arrival": 30,
                                                         "penalty": {"kind": "nights", "value": 1}}],
                            "no_show": {"kind": "percent", "value": 10000}});
        let policy = post(app, &owner, &format!("{path}/cancellation-policies"), policy).await.body;
        let bar = json!({"code": "BAR", "name": "Best available", "kind": "standard", "segment": "IBE",
                         "currency": "USD", "allowed_meal_plans": ["RO", "BB"], "room_type_ids": types,
                         "cancellation_policy_id": policy["id"]});
        let bar = common::uuid(&post(app, &owner, &format!("{path}/rate-plans"), bar).await.body["id"]);
        let hotel =
            Self { owner, superuser, id, path, business_date, deluxe: types[0], standard: types[1], rooms, bar };
        let days: Vec<String> = (0..60).map(|day| hotel.day(day)).collect();
        let prices: Vec<Value> = types
            .iter()
            .flat_map(|room_type| {
                days.iter().map(move |day| {
                    json!({"room_type_id": room_type, "date": day, "occupancy": 2,
                                                  "amount": 10_000})
                })
            })
            .collect();
        let bar_path = format!("{}/rate-plans/{}", hotel.path, hotel.bar);
        let priced = app
            .send(Method::PUT, &format!("{bar_path}/prices"), Some(&hotel.owner), Some(json!({"prices": prices})))
            .await;
        assert_eq!(priced.status, StatusCode::NO_CONTENT, "{:?}", priced.body);
        let breakfast = json!({"meal_plan": "BB", "currency": "USD", "adult_amount": 1500, "child_amount": 750,
                               "from": hotel.day(0)});
        post(app, &hotel.owner, &format!("{}/meal-supplements", hotel.path), breakfast).await;
        hotel
    }

    fn day(&self, offset: i64) -> String {
        (self.business_date + Duration::days(offset)).to_string()
    }

    fn room(&self, number: &str) -> &Value {
        self.rooms.iter().find(|room| room["number"] == number).unwrap()
    }

    async fn guest(&self, app: &TestApp, first: &str, last: &str) -> Value {
        let guest = json!({"first_name": first, "last_name": last, "residency": "non_resident"});
        post(app, &self.owner, &format!("{}/guests", self.path), guest).await.body
    }

    /// Books `rooms` deluxe rooms on BAR, room only, for two adults over `[business date + from, business date +
    /// to)`, and returns the created reservation.
    async fn book(&self, app: &TestApp, guest: &Value, from: i64, to: i64, rooms: usize, source: &str) -> Value {
        let room = json!({"room_type_id": self.deluxe, "rate_plan_id": self.bar, "meal_plan": "RO",
                          "check_in": self.day(from), "check_out": self.day(to), "adults": 2});
        let booking = json!({"booker_guest_id": guest["id"], "source": source, "rooms": vec![room; rooms]});
        post(app, &self.owner, &format!("{}/reservations", self.path), booking).await.body
    }

    /// `book`, then takes every booked room out of the room booking auto-assigned it, for tests whose point is an
    /// unassigned stay or a hand assignment. The result shows no room and each room's version after the unassign.
    async fn book_unassigned(
        &self,
        app: &TestApp,
        guest: &Value,
        from: i64,
        to: i64,
        rooms: usize,
        source: &str,
    ) -> Value {
        let mut created = self.book(app, guest, from, to, rooms, source).await;
        for index in 0..rooms {
            let version = created["rooms"][index]["version"].as_i64().unwrap();
            command(app, &self.owner, &format!("{}/unassign", self.stay(&created, index)), version, None).await;
            created["rooms"][index]["version"] = json!(version + 1);
            created["rooms"][index]["room_id"] = Value::Null;
            created["rooms"][index]["room_number"] = Value::Null;
        }
        created
    }

    fn stay(&self, created: &Value, index: usize) -> String {
        format!("{}/reservation-rooms/{}", self.path, created["rooms"][index]["id"].as_str().unwrap())
    }

    /// Seven bookings of ten deluxe rooms in all, arriving on different days, from five guests, two of them
    /// by phone; `GAL-000003` is cancelled. No room is assigned (each starts unassigned, at version 2).
    async fn bookings(&self, app: &TestApp) -> Vec<Value> {
        let guests = [
            self.guest(app, "Ada", "Silva").await,
            self.guest(app, "Ben", "Perera").await,
            self.guest(app, "Chamari", "Fernando").await,
            self.guest(app, "Dilan", "Bandara").await,
            self.guest(app, "Esha", "Dias").await,
        ];
        let plan = [
            (0, 3, 5, 1, "front_desk"),
            (1, 2, 4, 2, "phone"),
            (2, 4, 6, 1, "front_desk"),
            (3, 2, 3, 3, "email"),
            (4, 5, 7, 1, "phone"),
            (0, 1, 2, 1, "front_desk"),
            (1, 3, 4, 1, "email"),
        ];
        let mut created = Vec::new();
        for (guest, from, to, rooms, source) in plan {
            created.push(self.book_unassigned(app, &guests[guest], from, to, rooms, source).await);
        }
        command(app, &self.owner, &format!("{}/cancel", self.stay(&created[2], 0)), 2, None).await;
        created
    }
}

/// Every page of the list under `sort`, `first` at a time, and the total count each page reported.
async fn walk(app: &TestApp, hotel: &Hotel, sort: Value, first: i64) -> (Vec<Value>, Vec<i64>) {
    let (mut nodes, mut counts, mut after) = (Vec::new(), Vec::new(), Value::Null);
    loop {
        let variables = json!({"p": hotel.id, "sort": sort, "first": first, "after": after, "withCount": true});
        let page = graphql(app, &hotel.owner, LIST, variables).await;
        let page = &page["data"]["reservations"];
        assert!(page.is_object(), "{page:?}");
        nodes.extend(page["nodes"].as_array().unwrap().iter().cloned());
        counts.push(page["totalCount"].as_i64().unwrap());
        if page["pageInfo"]["hasNextPage"] == false {
            return (nodes, counts);
        }
        after = page["pageInfo"]["endCursor"].clone();
    }
}

fn ids(nodes: &[Value]) -> Vec<&str> {
    nodes.iter().map(|node| node["id"].as_str().unwrap()).collect()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn availability_offers_and_free_rooms_are_read_over_graphql(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let guest = hotel.guest(&app, "Ada", "Silva").await;
    let booked = hotel.book_unassigned(&app, &guest, 1, 3, 1, "front_desk").await;
    command(
        &app,
        &hotel.owner,
        &format!("{}/assign", hotel.stay(&booked, 0)),
        2,
        Some(json!({"room_id": hotel.room("101")["id"]})),
    )
    .await;
    let stay = |from: i64, to: i64| json!({"p": hotel.id, "in": hotel.day(from), "out": hotel.day(to)});
    let rooms = |room_type: Uuid, from: i64, to: i64| json!({"p": hotel.id, "type": room_type, "in": hotel.day(from), "out": hotel.day(to)});

    let available = graphql(&app, &hotel.owner, AVAILABILITY, stay(1, 3)).await;
    let free = graphql(&app, &hotel.owner, FREE_ROOMS, rooms(hotel.deluxe, 2, 4)).await;
    let free_after = graphql(&app, &hotel.owner, FREE_ROOMS, rooms(hotel.deluxe, 3, 5)).await;
    let too_long = graphql(&app, &hotel.owner, AVAILABILITY, stay(1, 32)).await;
    let longest = graphql(&app, &hotel.owner, AVAILABILITY, stay(1, 31)).await;
    let free_longest = graphql(&app, &hotel.owner, FREE_ROOMS, rooms(hotel.deluxe, 1, 731)).await;
    let free_too_long = graphql(&app, &hotel.owner, FREE_ROOMS, rooms(hotel.deluxe, 1, 732)).await;

    let types = available["data"]["availability"].as_array().unwrap();
    assert_eq!(types.len(), 2, "{available:?}");
    assert_eq!(
        (types[0]["roomTypeId"].clone(), types[0]["code"].clone(), types[0]["name"].clone(), types[0]["free"].clone()),
        (json!(hotel.deluxe), json!("DLX"), json!("Deluxe"), json!(11))
    );
    assert_eq!((types[1]["code"].clone(), types[1]["free"].clone()), (json!("STD"), json!(2)));
    let offers = types[0]["offers"].as_array().unwrap();
    assert_eq!(offers.len(), 2);
    assert_eq!(
        offers[1],
        json!({"ratePlanId": hotel.bar, "ratePlanCode": "BAR", "mealPlan": "BB", "total": 2 * (10_000 + 3_000),
               "currency": "USD", "restrictionsOk": true, "violations": [],
               "nights": [{"date": hotel.day(1), "room": 10_000, "meal": 3_000},
                          {"date": hotel.day(2), "room": 10_000, "meal": 3_000}]})
    );
    let numbers: Vec<&str> =
        free["data"]["freeRooms"].as_array().unwrap().iter().map(|room| room["number"].as_str().unwrap()).collect();
    assert_eq!(numbers.len(), 11, "101 is taken on day 2: {numbers:?}");
    assert!(!numbers.contains(&"101"));
    assert_eq!(free["data"]["freeRooms"][0], json!({"id": hotel.room("102")["id"], "number": "102", "section": null}));
    assert_eq!(free_after["data"]["freeRooms"][0]["number"], "101", "the stay leaves on day 3");
    assert_eq!(too_long["errors"][0]["message"], "the range must be 1 to 30 days");
    assert!(longest["data"]["availability"].is_array(), "30 nights is allowed: {longest:?}");
    assert!(free_longest["data"]["freeRooms"].is_array(), "any stay in the 730-night window: {free_longest:?}");
    assert_eq!(free_too_long["errors"][0]["message"], "the range must be 1 to 730 days");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_list_pages_through_every_room_once_in_order_under_each_sort(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    hotel.bookings(&app).await;

    let everything = graphql(&app, &hotel.owner, LIST, json!({"p": hotel.id, "first": 100, "withCount": true})).await;
    let (by_arrival, arrival_counts) = walk(&app, &hotel, Value::Null, 3).await;
    let (by_guest, guest_counts) = walk(&app, &hotel, json!({"field": "GUEST", "direction": "DESC"}), 4).await;
    let (by_confirmation, _) = walk(&app, &hotel, json!({"field": "CONFIRMATION"}), 3).await;
    let (by_created, _) = walk(&app, &hotel, json!({"field": "CREATED", "direction": "DESC"}), 3).await;

    let all = everything["data"]["reservations"]["nodes"].as_array().unwrap().clone();
    assert_eq!(all.len(), 10, "one node per room, cancelled ones included");
    assert_eq!(everything["data"]["reservations"]["totalCount"], 10);
    assert_eq!(everything["data"]["reservations"]["pageInfo"]["hasNextPage"], false);
    let first = &all[0];
    assert_eq!(
        (
            first["confirmationNo"].clone(),
            first["guestName"].clone(),
            first["arrival"].clone(),
            first["departure"].clone(),
            first["nights"].clone(),
            first["roomTypeCode"].clone()
        ),
        (json!("GAL-000006"), json!("Ada Silva"), json!(hotel.day(1)), json!(hotel.day(2)), json!(1), json!("DLX"))
    );
    assert_eq!(
        (
            first["roomNumber"].clone(),
            first["status"].clone(),
            first["source"].clone(),
            first["total"].clone(),
            first["currency"].clone(),
            first["version"].clone()
        ),
        (Value::Null, json!("CONFIRMED"), json!("FRONT_DESK"), json!(10_000), json!("USD"), json!(2))
    );

    let mut expected = all.clone();
    expected.sort_by(|a, b| (a["arrival"].as_str(), a["id"].as_str()).cmp(&(b["arrival"].as_str(), b["id"].as_str())));
    assert_eq!(ids(&by_arrival), ids(&expected), "arrival, then id, by default");
    assert_eq!(ids(&all), ids(&expected));
    assert_eq!(arrival_counts, [10, 10, 10, 10], "four pages of 3, each counting every match");

    let guest_key = |node: &Value| {
        let name = node["guestName"].as_str().unwrap();
        let (first, last) = name.split_once(' ').unwrap();
        (format!("{} {}", last.to_lowercase(), first.to_lowercase()), node["id"].as_str().unwrap().to_owned())
    };
    let mut expected = all.clone();
    expected.sort_by_key(|node| std::cmp::Reverse(guest_key(node)));
    assert_eq!(ids(&by_guest), ids(&expected), "last name, first name, then id, descending");
    assert_eq!(guest_counts.len(), 3);
    assert_eq!(by_guest[0]["guestName"], "Ada Silva");

    let mut expected = all.clone();
    expected.sort_by(|a, b| {
        (a["confirmationNo"].as_str(), a["id"].as_str()).cmp(&(b["confirmationNo"].as_str(), b["id"].as_str()))
    });
    assert_eq!(ids(&by_confirmation), ids(&expected));
    let mut expected = all;
    expected.sort_by(|a, b| {
        (b["confirmationNo"].as_str(), b["id"].as_str()).cmp(&(a["confirmationNo"].as_str(), a["id"].as_str()))
    });
    assert_eq!(ids(&by_created), ids(&expected), "newest booking first");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_cursor_works_only_under_its_own_sort_and_pages_are_1_to_100(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    hotel.bookings(&app).await;
    let page = graphql(&app, &hotel.owner, LIST, json!({"p": hotel.id, "first": 2, "withCount": true})).await;
    let cursor = page["data"]["reservations"]["pageInfo"]["endCursor"].clone();
    let list = |sort: Value, first: i64, after: Value| json!({"p": hotel.id, "sort": sort, "first": first, "after": after, "withCount": false});

    let other_sort = graphql(&app, &hotel.owner, LIST, list(json!({"field": "GUEST"}), 2, cursor.clone())).await;
    let other_direction =
        graphql(&app, &hotel.owner, LIST, list(json!({"field": "ARRIVAL", "direction": "DESC"}), 2, cursor.clone()))
            .await;
    let same_sort = graphql(&app, &hotel.owner, LIST, list(json!({"field": "ARRIVAL"}), 2, cursor)).await;
    let garbage = graphql(&app, &hotel.owner, LIST, list(Value::Null, 2, json!("not a cursor"))).await;
    let none = graphql(&app, &hotel.owner, LIST, list(Value::Null, 0, Value::Null)).await;
    let too_many = graphql(&app, &hotel.owner, LIST, list(Value::Null, 101, Value::Null)).await;

    assert_eq!(other_sort["errors"][0]["message"], "the cursor belongs to another sort; start from the first page");
    assert_eq!(
        other_direction["errors"][0]["message"],
        "the cursor belongs to another sort; start from the first page"
    );
    assert_eq!(same_sort["data"]["reservations"]["nodes"].as_array().map(Vec::len), Some(2), "{same_sort:?}");
    // A later page asks for no count (`withCount: false`), and gets none.
    assert_eq!(same_sort["data"]["reservations"].get("totalCount"), None, "{same_sort:?}");
    assert_eq!(garbage["errors"][0]["message"], "the cursor is not valid");
    assert_eq!(none["errors"][0]["message"], "first is 1 to 100");
    assert_eq!(too_many["errors"][0]["message"], "first is 1 to 100");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_list_filters_by_arrival_status_source_and_text(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    hotel.bookings(&app).await;
    let filtered = async |filter: Value| {
        let page = graphql(&app, &hotel.owner, LIST, json!({"p": hotel.id, "filter": filter, "withCount": true})).await;
        let page = &page["data"]["reservations"];
        assert!(page.is_object(), "{page:?}");
        let mut numbers: Vec<String> =
            page["nodes"].as_array().unwrap().iter().map(|n| n["confirmationNo"].as_str().unwrap().into()).collect();
        numbers.sort();
        (numbers, page["totalCount"].as_i64().unwrap())
    };

    let cancelled = filtered(json!({"statuses": ["CANCELLED"]})).await;
    let by_phone = filtered(json!({"sources": ["PHONE"]})).await;
    let confirmed_by_email = filtered(json!({"statuses": ["CONFIRMED"], "sources": ["EMAIL"]})).await;
    let no_status = filtered(json!({"statuses": []})).await;
    let arriving = filtered(json!({"arrivalFrom": hotel.day(2), "arrivalTo": hotel.day(3)})).await;
    let by_number = filtered(json!({"text": "gal-000004"})).await;
    let by_prefix = filtered(json!({"text": " GAL-00000 "})).await;
    let by_name = filtered(json!({"text": "fernand"})).await;
    let by_typo = filtered(json!({"text": "Pererra"})).await;
    let like_wildcard = filtered(json!({"text": "GAL-%"})).await;

    let numbers = |list: &[&str]| list.iter().map(|n| format!("GAL-00000{n}")).collect::<Vec<_>>();
    assert_eq!(cancelled, (numbers(&["3"]), 1));
    assert_eq!(by_phone, (numbers(&["2", "2", "5"]), 3));
    assert_eq!(confirmed_by_email, (numbers(&["4", "4", "4", "7"]), 4));
    assert_eq!(no_status, (vec![], 0), "an empty selection matches nothing");
    assert_eq!(arriving, (numbers(&["1", "2", "2", "4", "4", "4", "7"]), 7), "arrivals from day 2 to day 3, inclusive");
    assert_eq!(by_number, (numbers(&["4", "4", "4"]), 3));
    assert_eq!(by_prefix.1, 10);
    assert_eq!(by_name, (numbers(&["3"]), 1));
    assert_eq!(by_typo, (numbers(&["2", "2", "7"]), 3), "Perera, despite the typo");
    assert_eq!(like_wildcard, (vec![], 0), "`%` is matched literally");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_detail_shows_rooms_nights_terms_penalties_masked_ids_and_history(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let guest = json!({"first_name": "Ada", "last_name": "Silva", "residency": "non_resident",
                       "id_doc": {"type": "passport", "number": "N7654321"}});
    let guest = post(&app, &hotel.owner, &format!("{}/guests", hotel.path), guest).await.body;
    let booked = hotel.book_unassigned(&app, &guest, 2, 4, 2, "phone").await;
    let assign_101 = Some(json!({"room_id": hotel.room("101")["id"]}));
    command(&app, &hotel.owner, &format!("{}/assign", hotel.stay(&booked, 0)), 2, assign_101).await;
    command(&app, &hotel.owner, &format!("{}/cancel", hotel.stay(&booked, 1)), 2, None).await;

    let response = app
        .send(Method::POST, "/graphql", Some(&hotel.owner), Some(json!({"query": DETAIL,
                                                                        "variables": {"p": hotel.id, "id": booked["id"]}})))
        .await;
    let searched = graphql(&app, &hotel.owner, GUESTS, json!({"p": hotel.id, "search": "silv"})).await;
    let missing = graphql(&app, &hotel.owner, DETAIL, json!({"p": hotel.id, "id": Uuid::now_v7()})).await;

    let raw = response.body.to_string();
    assert!(!raw.contains("7654321"), "the ID number never leaves the database: {raw}");
    let detail = &response.body["data"]["reservation"];
    assert_eq!(
        (detail["id"].clone(), detail["confirmationNo"].clone(), detail["status"].clone(), detail["source"].clone()),
        (booked["id"].clone(), json!("GAL-000001"), json!("CONFIRMED"), json!("PHONE")),
        "{detail:?}"
    );
    // Created at 1; the two unassigns (booking auto-assigned both rooms), the assignment and the cancellation each
    // moved it.
    assert_eq!(detail["version"], 5);
    assert!(detail["createdAt"].is_string());
    assert_eq!(detail["booker"]["idDocMasked"], "•••• 4321");
    assert_eq!(detail["booker"]["idDocType"], "PASSPORT");
    assert_eq!(detail["totals"], json!([{"currency": "USD", "amount": 20_000}]), "the cancelled room is not owed");

    let (kept, cancelled) = (&detail["rooms"][0], &detail["rooms"][1]);
    assert_eq!(
        (kept["status"].clone(), kept["version"].clone(), kept["checkIn"].clone(), kept["checkOut"].clone()),
        (json!("CONFIRMED"), json!(3), json!(hotel.day(2)), json!(hotel.day(4)))
    );
    assert_eq!(kept["roomType"], json!({"id": hotel.deluxe, "code": "DLX", "name": "Deluxe"}));
    assert_eq!(kept["room"], json!({"id": hotel.room("101")["id"], "number": "101"}));
    assert_eq!(kept["ratePlan"], json!({"id": hotel.bar, "code": "BAR"}));
    assert_eq!(
        (kept["mealPlan"].clone(), kept["adults"].clone(), kept["children"].clone()),
        (json!("RO"), json!(2), json!(0))
    );
    assert_eq!(kept["primaryGuest"]["idDocMasked"], "•••• 4321");
    assert_eq!(
        kept["nights"],
        json!([{"date": hotel.day(2), "room": 10_000, "meal": 0}, {"date": hotel.day(3), "room": 10_000, "meal": 0}])
    );
    assert_eq!((kept["total"].clone(), kept["currency"].clone()), (json!(20_000), json!("USD")));
    assert_eq!(
        kept["cancellationTerms"],
        json!({"rules": [{"daysBeforeArrival": 30, "penalty": {"kind": "NIGHTS", "value": 1}}],
               "noShow": {"kind": "PERCENT", "value": 10000}})
    );
    assert_eq!(kept["cancellationPenalty"], 10_000, "cancelling today costs the first night");
    assert_eq!((kept["cancelledAt"].clone(), kept["recordedPenalty"].clone()), (Value::Null, Value::Null));
    assert_eq!(cancelled["status"], "CANCELLED");
    assert_eq!(cancelled["cancellationPenalty"], Value::Null, "a cancelled room can't be cancelled again");
    assert_eq!(cancelled["recordedPenalty"], 10_000);
    assert!(cancelled["cancelledAt"].is_string());
    assert_eq!(cancelled["room"], Value::Null);

    let history: Vec<(&str, &str)> = detail["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| (entry["action"].as_str().unwrap(), entry["actorName"].as_str().unwrap()))
        .collect();
    assert_eq!(
        history,
        [
            ("reservation_room.cancelled", "Owner"),
            ("reservation_room.assigned", "Owner"),
            ("reservation_room.unassigned", "Owner"),
            ("reservation_room.unassigned", "Owner"),
            ("reservation.created", "Owner")
        ],
        "newest first"
    );
    assert_eq!(detail["history"][0]["data"]["penalty"], 10_000);
    assert_eq!(detail["history"][1]["data"]["number"], "101");

    assert_eq!(searched["data"]["guests"][0]["idDocMasked"], "•••• 4321", "{searched:?}");
    assert!(!searched.to_string().contains("7654321"));
    assert_eq!(missing["errors"][0]["message"], "reservation not found");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn every_role_reads_reservations_and_another_tenant_reads_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let created = hotel.bookings(&app).await;
    let housekeeping = app.staff(&hotel.superuser, &hotel.owner, "hk@example.com", "housekeeping").await;
    let intruder = app.signup_owner("intruder@example.com", "Other Hotels").await;
    let detail = json!({"p": hotel.id, "id": created[0]["id"]});
    let free = json!({"p": hotel.id, "type": hotel.standard, "in": hotel.day(1), "out": hotel.day(2)});
    let stay = json!({"p": hotel.id, "in": hotel.day(1), "out": hotel.day(2)});

    let mut by_housekeeping = Vec::new();
    let mut by_intruder = Vec::new();
    for (query, variables) in [
        (LIST, json!({"p": hotel.id, "withCount": true})),
        (DETAIL, detail),
        (GUESTS, json!({"p": hotel.id, "search": "Silva"})),
        (FREE_ROOMS, free),
        (AVAILABILITY, stay),
    ] {
        by_housekeeping.push(graphql(&app, &housekeeping, query, variables.clone()).await);
        by_intruder.push(graphql(&app, &intruder, query, variables).await);
    }

    assert_eq!(by_housekeeping[0]["data"]["reservations"]["totalCount"], 10, "{:?}", by_housekeeping[0]);
    assert_eq!(by_housekeeping[1]["data"]["reservation"]["confirmationNo"], "GAL-000001");
    assert_eq!(by_housekeeping[2]["data"]["guests"][0]["lastName"], "Silva");
    assert_eq!(by_housekeeping[3]["data"]["freeRooms"].as_array().map(Vec::len), Some(2));
    assert_eq!(by_housekeeping[4]["data"]["availability"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        by_intruder[0]["data"]["reservations"],
        json!({"nodes": [], "pageInfo": {"endCursor": null, "hasNextPage": false}, "totalCount": 0})
    );
    assert_eq!(by_intruder[1]["errors"][0]["message"], "reservation not found");
    assert_eq!(by_intruder[2]["data"]["guests"], json!([]));
    assert_eq!(by_intruder[3]["data"]["freeRooms"], json!([]));
    assert_eq!(by_intruder[4]["errors"][0]["message"], "property not found");
}

#[tokio::test]
async fn the_spa_s_list_and_detail_queries_fit_the_depth_and_complexity_limits() {
    let schema = build_schema(false);
    let variables = json!({"p": Uuid::nil(), "id": Uuid::nil(), "withCount": true});

    for query in [LIST, DETAIL] {
        let request =
            async_graphql::Request::new(query).variables(async_graphql::Variables::from_json(variables.clone()));
        let response = schema.execute(request).await;

        // Without a database the resolver fails, which it only reaches once the query passed validation and
        // both limits.
        let messages: Vec<&str> = response.errors.iter().map(|err| err.message.as_str()).collect();
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(messages[0].starts_with("Data ") && messages[0].ends_with("does not exist."), "{messages:?}");
    }
}
