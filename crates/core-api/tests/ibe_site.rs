//! The booking site settings (`/ibe-site`) and the room type description.

mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, TestResponse, uuid};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

struct Hotel {
    owner: String,
    superuser: PgPool,
    id: Uuid,
    path: String,
    room_type: Uuid,
}

impl Hotel {
    async fn new(app: &TestApp, opts: PgConnectOptions, email: &str, code: &str) -> Self {
        let superuser = PgPool::connect_with(opts).await.unwrap();
        let owner = app.signup_owner(email, code).await;
        let property = json!({"code": code, "name": code, "timezone": "Asia/Colombo", "base_currency": "LKR"});
        let id = uuid(&post(app, &owner, "/api/v1/properties", property).await.body["id"]);
        let path = format!("/api/v1/properties/{id}");
        let deluxe = json!({"code": "DLX", "name": "Deluxe", "base_occupancy": 2, "max_adults": 2,
                            "max_children": 1, "max_occupancy": 3});
        let room_type = uuid(&post(app, &owner, &format!("{path}/room-types"), deluxe).await.body["id"]);
        Self { owner, superuser, id, path, room_type }
    }

    /// A ready asset of `purpose`; a `gallery` one is also in the room type's gallery.
    async fn asset(&self, purpose: &str, gallery_of: Option<Uuid>) -> Uuid {
        let asset = Uuid::now_v7();
        sqlx::query(
            "insert into media_asset (id, tenant_id, property_id, purpose, visibility, status, filename, declared_bytes,
                                      sha256)
             select $1, tenant_id, id, $2, 'public', 'ready', 'a.jpg', 10, sha256($1::text::bytea) from property
             where id = $3",
        )
        .bind(asset)
        .bind(purpose)
        .bind(self.id)
        .execute(&self.superuser)
        .await
        .unwrap();
        if let Some(room_type) = gallery_of {
            sqlx::query(
                "insert into room_type_media (tenant_id, room_type_id, asset_id, position)
                 select tenant_id, $1, $2, 0 from property where id = $3",
            )
            .bind(room_type)
            .bind(asset)
            .bind(self.id)
            .execute(&self.superuser)
            .await
            .unwrap();
        }
        asset
    }
}

async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    let key = Uuid::now_v7().to_string();
    app.send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)])
        .await
}

async fn versioned(
    app: &TestApp,
    method: Method,
    cookie: &str,
    path: &str,
    version: i64,
    body: Option<Value>,
) -> TestResponse {
    let if_match = format!("\"{version}\"");
    app.send_with(method, path, Some(cookie), body, &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)]).await
}

fn site() -> Value {
    json!({
        "enabled": true,
        "custom_host": "stay.galle.example",
        "accent": "#1d4ed8",
        "description": "By the sea",
        "contact_phone": "+94 91 000 0000",
        "contact_email": "hello@galle.example",
        "terms": "No smoking.",
        "privacy": "We keep your name.",
    })
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_settings_start_as_defaults_and_round_trip_with_if_match(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL").await;
    let path = format!("{}/ibe-site", hotel.path);

    let defaults = app.send(Method::GET, &path, Some(&hotel.owner), None).await;
    let no_version = app.send(Method::PUT, &path, Some(&hotel.owner), Some(site())).await;
    let saved = versioned(&app, Method::PUT, &hotel.owner, &path, 0, Some(site())).await;
    let stale = versioned(&app, Method::PUT, &hotel.owner, &path, 0, Some(site())).await;
    let read = app.send(Method::GET, &path, Some(&hotel.owner), None).await;
    let cleared = versioned(
        &app,
        Method::PUT,
        &hotel.owner,
        &path,
        1,
        Some(json!({"enabled": false, "custom_host": null, "contact_phone": null})),
    )
    .await;

    assert_eq!(defaults.status, StatusCode::OK, "{:?}", defaults.body);
    assert_eq!(defaults.headers["etag"], "\"0\"");
    assert_eq!((&defaults.body["enabled"], &defaults.body["version"]), (&json!(false), &json!(0)));
    assert_eq!(no_version.status, StatusCode::PRECONDITION_REQUIRED);
    assert_eq!(saved.status, StatusCode::OK, "{:?}", saved.body);
    assert_eq!(saved.headers["etag"], "\"1\"");
    assert_eq!(stale.status, StatusCode::PRECONDITION_FAILED, "{:?}", stale.body);
    assert_eq!(read.body, saved.body);
    assert_eq!(read.body["custom_host"], "stay.galle.example");
    assert_eq!(read.body["logo_url"], Value::Null);
    assert_eq!(cleared.status, StatusCode::OK, "{:?}", cleared.body);
    assert_eq!(
        (&cleared.body["custom_host"], &cleared.body["terms"], &cleared.body["version"]),
        (&Value::Null, &json!(""), &json!(2))
    );
    let audit: Vec<Value> =
        sqlx::query_scalar("select data from audit_log where action = 'ibe_site.updated' order by id")
            .fetch_all(&hotel.superuser)
            .await
            .unwrap();
    assert_eq!(audit.len(), 2);
    assert_eq!(audit[0]["before"], Value::Null);
    assert_eq!(audit[0]["after"]["custom_host"], "stay.galle.example");
    assert_eq!(audit[1]["before"]["enabled"], true);
    assert_eq!(audit[1]["after"]["enabled"], false);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_accent_needs_contrast_with_white_text(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL").await;
    let path = format!("{}/ibe-site", hotel.path);
    let with_accent = |accent: &str| json!({"enabled": false, "accent": accent});

    let white = versioned(&app, Method::PUT, &hotel.owner, &path, 0, Some(with_accent("#ffffff"))).await;
    let upper = versioned(&app, Method::PUT, &hotel.owner, &path, 0, Some(with_accent("#1D4ED8"))).await;
    let short = versioned(&app, Method::PUT, &hotel.owner, &path, 0, Some(with_accent("#fff"))).await;
    let blue = versioned(&app, Method::PUT, &hotel.owner, &path, 0, Some(with_accent("#1d4ed8"))).await;

    assert_eq!(white.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(white.body["detail"], "The accent colour needs more contrast with white text");
    assert_eq!(upper.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(short.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(blue.status, StatusCode::OK, "{:?}", blue.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_custom_host_is_lowercase_and_unique_across_tenants(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let galle = Hotel::new(&app, opts.clone(), "owner@example.com", "GAL").await;
    let kandy = Hotel::new(&app, opts, "other@example.com", "KAN").await;
    let galle_path = format!("{}/ibe-site", galle.path);
    let kandy_path = format!("{}/ibe-site", kandy.path);

    let upper = versioned(
        &app,
        Method::PUT,
        &galle.owner,
        &galle_path,
        0,
        Some(json!({"enabled": false, "custom_host": "Stay.Galle.Example"})),
    )
    .await;
    let taken_first = versioned(&app, Method::PUT, &galle.owner, &galle_path, 0, Some(site())).await;
    let taken = versioned(&app, Method::PUT, &kandy.owner, &kandy_path, 0, Some(site())).await;

    assert_eq!(upper.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(taken_first.status, StatusCode::OK, "{:?}", taken_first.body);
    assert_eq!(taken.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", taken.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_hero_must_be_a_gallery_photo_of_this_property(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let galle = Hotel::new(&app, opts.clone(), "owner@example.com", "GAL").await;
    let kandy = Hotel::new(&app, opts, "owner@example.net", "KAN").await;
    let path = format!("{}/ibe-site", galle.path);
    let own_photo = galle.asset("gallery", Some(galle.room_type)).await;
    let own_logo = galle.asset("ibe_logo", None).await;
    let other_photo = kandy.asset("gallery", Some(kandy.room_type)).await;
    let with = |hero: Uuid| json!({"enabled": false, "hero_asset_id": hero});

    let foreign = versioned(&app, Method::PUT, &galle.owner, &path, 0, Some(with(other_photo))).await;
    let logo = versioned(&app, Method::PUT, &galle.owner, &path, 0, Some(with(own_logo))).await;
    let photo_as_logo = versioned(
        &app,
        Method::PUT,
        &galle.owner,
        &path,
        0,
        Some(json!({"enabled": false, "logo_asset_id": own_photo})),
    )
    .await;
    let fine = versioned(&app, Method::PUT, &galle.owner, &path, 0, Some(with(own_photo))).await;

    for refused in [&foreign, &logo, &photo_as_logo] {
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", refused.body);
    }
    assert_eq!(fine.status, StatusCode::OK, "{:?}", fine.body);
    assert_eq!(fine.body["hero_asset_id"], json!(own_photo));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn fields_are_bounded(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL").await;
    let path = format!("{}/ibe-site", hotel.path);

    for refused in [
        json!({"enabled": false, "description": "x".repeat(501)}),
        json!({"enabled": false, "terms": "x".repeat(10_001)}),
        json!({"enabled": false, "privacy": "x".repeat(10_001)}),
        json!({"enabled": false, "contact_email": "not an email"}),
        json!({"enabled": false, "custom_host": "ab"}),
        json!({"enabled": false, "custom_host": "localhost"}),
        json!({"enabled": false, "custom_host": "-a-.com"}),
        json!({"enabled": false, "custom_host": "a..b"}),
    ] {
        let response = versioned(&app, Method::PUT, &hotel.owner, &path, 0, Some(refused.clone())).await;
        assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}: {:?}", response.body);
    }
    let real = json!({"enabled": false, "custom_host": "book.galleforthotel.lk"});
    assert_eq!(versioned(&app, Method::PUT, &hotel.owner, &path, 0, Some(real)).await.status, StatusCode::OK);
    let longest = json!({"enabled": false, "description": "x".repeat(500), "terms": "é".repeat(10_000)});
    let response = versioned(&app, Method::PUT, &hotel.owner, &path, 1, Some(longest)).await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn owners_and_managers_manage_the_site_and_nobody_else(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL").await;
    let path = format!("{}/ibe-site", hotel.path);
    let manager = app.staff(&hotel.superuser, &hotel.owner, "manager@example.com", "manager").await;

    let managed = versioned(&app, Method::PUT, &manager, &path, 0, Some(json!({"enabled": true}))).await;
    assert_eq!(managed.status, StatusCode::OK, "{:?}", managed.body);
    assert_eq!(app.send(Method::GET, &path, Some(&manager), None).await.status, StatusCode::OK);
    for (n, role) in ["front_desk", "housekeeping", "hk_supervisor", "accountant", "pos"].into_iter().enumerate() {
        let cookie = app.staff(&hotel.superuser, &hotel.owner, &format!("{role}{n}@example.com"), role).await;
        let read = app.send(Method::GET, &path, Some(&cookie), None).await;
        let write = versioned(&app, Method::PUT, &cookie, &path, 1, Some(json!({"enabled": false}))).await;
        assert_eq!(read.status, StatusCode::FORBIDDEN, "{role}");
        assert_eq!(write.status, StatusCode::FORBIDDEN, "{role}");
    }
    let unknown = app
        .send(Method::GET, &format!("/api/v1/properties/{}/ibe-site", Uuid::now_v7()), Some(&hotel.owner), None)
        .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_room_type_description_round_trips_and_is_at_most_1000_characters(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL").await;
    let path = format!("{}/room-types/{}", hotel.path, hotel.room_type);

    let set = versioned(&app, Method::PATCH, &hotel.owner, &path, 1, Some(json!({"description": "A sea view"}))).await;
    let too_long =
        versioned(&app, Method::PATCH, &hotel.owner, &path, 2, Some(json!({"description": "x".repeat(1001)}))).await;
    let longest =
        versioned(&app, Method::PATCH, &hotel.owner, &path, 2, Some(json!({"description": "é".repeat(1000)}))).await;
    let query = "query ($p: UUID!) { roomTypes(propertyId: $p) { description } }";
    let read = app
        .send(Method::POST, "/graphql", Some(&hotel.owner), Some(json!({"query": query, "variables": {"p": hotel.id}})))
        .await;

    assert_eq!(set.status, StatusCode::OK, "{:?}", set.body);
    assert_eq!(set.body["description"], "A sea view");
    assert_eq!(too_long.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(longest.status, StatusCode::OK, "{:?}", longest.body);
    assert_eq!(read.body["data"]["roomTypes"][0]["description"], "é".repeat(1000));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn two_hotels_cannot_both_publish_under_the_same_code(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let first = Hotel::new(&app, opts.clone(), "one@example.com", "GAL").await;
    let second = Hotel::new(&app, opts, "two@example.com", "GAL").await;
    let first_path = format!("{}/ibe-site", first.path);
    let second_path = format!("{}/ibe-site", second.path);

    let taken = versioned(&app, Method::PUT, &first.owner, &first_path, 0, Some(json!({"enabled": true}))).await;
    let refused = versioned(&app, Method::PUT, &second.owner, &second_path, 0, Some(json!({"enabled": true}))).await;
    let draft = versioned(&app, Method::PUT, &second.owner, &second_path, 0, Some(json!({"enabled": false}))).await;
    let released = versioned(&app, Method::PUT, &first.owner, &first_path, 1, Some(json!({"enabled": false}))).await;
    let now_free = versioned(&app, Method::PUT, &second.owner, &second_path, 1, Some(json!({"enabled": true}))).await;

    assert_eq!(taken.status, StatusCode::OK, "{:?}", taken.body);
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", refused.body);
    assert_eq!(refused.body["field"], "enabled");
    assert_eq!(
        refused.body["detail"],
        "Another hotel already uses the code GAL for its booking site. Change this property's code to publish it."
    );
    assert_eq!(draft.status, StatusCode::OK, "{:?}", draft.body);
    assert_eq!(released.status, StatusCode::OK, "{:?}", released.body);
    assert_eq!(now_free.status, StatusCode::OK, "{:?}", now_free.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_custom_host_cannot_be_the_booking_domain(_: PgPoolOptions, opts: PgConnectOptions) {
    let mut app = TestApp::new(opts.clone()).await;
    let ibe = core_api::config::IbeConfig {
        public_origin: "https://book.example.lk".into(),
        ..core_api::config::IbeConfig::local()
    };
    app.state = app.state.clone().with_ibe(ibe);
    app.router = core_api::router(app.state.clone());
    let hotel = Hotel::new(&app, opts, "owner@example.com", "GAL").await;
    let path = format!("{}/ibe-site", hotel.path);

    let own = json!({"enabled": false, "custom_host": "book.example.lk"});
    let refused = versioned(&app, Method::PUT, &hotel.owner, &path, 0, Some(own)).await;
    let other = json!({"enabled": false, "custom_host": "stay.book.example.lk"});
    let fine = versioned(&app, Method::PUT, &hotel.owner, &path, 0, Some(other)).await;

    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", refused.body);
    assert_eq!(refused.body["field"], "custom_host");
    assert_eq!(fine.status, StatusCode::OK, "{:?}", fine.body);
}
