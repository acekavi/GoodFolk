//! The media engine through core-api: uploads against RustFS and an in-process media-svc, galleries, issue photos,
//! guest documents, `/internal/events` and the sweep. RustFS must be running (`deploy/dev/rustfs.sh start`), and the
//! full flow runs media-svc's worker binary, which these tests build.

mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use common::media::{Hotel, SECRET, fixture, http, post, services, stack, start_body, storage, upload};
use common::{TestApp, uuid};
use core_api::{AppState, router};
use events::{PushEnvelope, PushMessage};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Connection as _;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::collections::BTreeMap;
use std::time::Duration;
use storage::{Bucket, Storage, upload_key};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use uuid::Uuid;

async fn graphql(app: &TestApp, cookie: &str, query: &str, variables: Value) -> Value {
    let response =
        app.send(Method::POST, "/graphql", Some(cookie), Some(json!({"query": query, "variables": variables}))).await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
    response.body
}

const GALLERY: &str = "query ($p: UUID!) {
    roomTypes(propertyId: $p) {
        id media { assetId status caption width height renditions { size format width height url } }
    }
}";
const ISSUE_PHOTOS: &str = "query ($p: UUID!) {
    roomIssues(propertyId: $p) { id photos { assetId status renditions { size format url } } }
}";
const GUEST_DOCUMENTS: &str = "query ($p: UUID!, $g: UUID!) {
    guestDocuments(propertyId: $p, guestId: $g) { id kind status mime uploadedAt uploadedBy }
}";

/// Reads the event stream until an invalidation naming `key` arrives, for up to `within`.
async fn expect_invalidation(stream: &mut Body, key: &str, within: Duration) {
    let found = tokio::time::timeout(within, async {
        loop {
            let frame = stream.frame().await.expect("the stream stays open").unwrap();
            if let Ok(data) = frame.into_data()
                && String::from_utf8_lossy(&data).contains(key)
            {
                return;
            }
        }
    })
    .await;
    assert!(found.is_ok(), "no invalidation named {key} arrived");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn upload_flow_gallery_reaches_ready(_: PgPoolOptions, opts: PgConnectOptions) {
    let stack = stack(opts).await;
    let app = &stack.app;
    let hotel = Hotel::new(app, "owner@example.com", "GAL").await;
    let request = Request::get(format!("/api/v1/events?property={}", hotel.id))
        .header(header::COOKIE, &hotel.owner)
        .body(Body::empty())
        .unwrap();
    let mut stream = app.router.clone().oneshot(request).await.unwrap().into_body();

    // 2400×1800: every gallery size, each as AVIF and WebP.
    let photo = fixture("onesided.jpg");
    let completed =
        upload(app, &hotel.owner, &hotel.path, start_body("gallery", hotel.room_type, &photo), &photo).await;
    assert_eq!(completed.status, StatusCode::OK, "{:?}", completed.body);
    assert_eq!(completed.body["status"], "processing");
    assert_eq!(completed.body["mime"], "image/jpeg");
    let asset = completed.body["asset_id"].clone();
    // Completing invalidates the property's media once; the next invalidation can only be the result's.
    let key = format!("media:{}", hotel.id);
    expect_invalidation(&mut stream, &key, Duration::from_secs(5)).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let media = loop {
        let body = graphql(app, &hotel.owner, GALLERY, json!({"p": hotel.id})).await;
        let media = body["data"]["roomTypes"][0]["media"][0].clone();
        assert_eq!(media["assetId"], asset);
        if media["status"] == "READY" {
            break media;
        }
        assert_ne!(media["status"], "FAILED", "{media:?}");
        assert!(tokio::time::Instant::now() < deadline, "the photo was not ready in time");
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    assert_eq!((media["width"].as_i64(), media["height"].as_i64()), (Some(2400), Some(1800)));
    let renditions = media["renditions"].as_array().unwrap();
    assert_eq!(renditions.len(), 10, "{renditions:?}");
    for rendition in renditions {
        let url = rendition["url"].as_str().unwrap();
        assert!(!url.contains("X-Amz-Signature"), "gallery URLs are permanent: {url}");
        let response = http().get(url).send().await.unwrap();
        assert_eq!(response.status(), 200, "{url}");
        assert_eq!(response.headers()["cache-control"], storage::IMMUTABLE);
    }
    expect_invalidation(&mut stream, &key, Duration::from_secs(5)).await;
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn refused_upload_is_422_and_row_gone(_: PgPoolOptions, opts: PgConnectOptions) {
    let superuser = PgPool::connect_with(opts.clone()).await.unwrap();
    let stack = stack(opts).await;
    let app = &stack.app;
    let hotel = Hotel::new(app, "owner@example.com", "GAL").await;

    // A PDF is not a photo, whatever its name says.
    let pdf = fixture("fake.jpg");
    let refused = upload(app, &hotel.owner, &hotel.path, start_body("gallery", hotel.room_type, &pdf), &pdf).await;

    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", refused.body);
    assert_eq!(refused.body["detail"], "This file type is not accepted here");
    let rows: i64 = sqlx::query_scalar("select count(*) from media_asset").fetch_one(&superuser).await.unwrap();
    assert_eq!(rows, 0);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_duplicate_upload_links_the_existing_asset(_: PgPoolOptions, opts: PgConnectOptions) {
    let superuser = PgPool::connect_with(opts.clone()).await.unwrap();
    let stack = stack(opts).await;
    let app = &stack.app;
    let hotel = Hotel::new(app, "owner@example.com", "GAL").await;
    let photo = fixture("small.jpg");

    let first = upload(app, &hotel.owner, &hotel.path, start_body("issue_photo", hotel.issue, &photo), &photo).await;
    let second = upload(app, &hotel.owner, &hotel.path, start_body("gallery", hotel.room_type, &photo), &photo).await;
    let third = upload(app, &hotel.owner, &hotel.path, start_body("issue_photo", hotel.issue, &photo), &photo).await;

    assert_eq!(first.status, StatusCode::OK, "{:?}", first.body);
    assert_ne!(second.body["asset_id"], first.body["asset_id"], "another purpose stores its own copy");
    assert_eq!(third.status, StatusCode::OK, "{:?}", third.body);
    assert_eq!(third.body["asset_id"], first.body["asset_id"]);
    let rows: i64 = sqlx::query_scalar("select count(*) from media_asset").fetch_one(&superuser).await.unwrap();
    assert_eq!(rows, 2);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn start_without_media_svc_is_502_and_nothing_left(_: PgPoolOptions, opts: PgConnectOptions) {
    let superuser = PgPool::connect_with(opts.clone()).await.unwrap();
    // A port nothing listens on.
    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
    let pool = db::testing::app_pool(opts, 5).await;
    let state =
        AppState::with_media(pool.clone(), false, db::testing::guest_id_keys(), services(storage().await, closed));
    let app = TestApp { router: router(state.clone()), state, pool };
    let hotel = Hotel::new(&app, "owner@example.com", "GAL").await;
    let photo = fixture("small.jpg");

    let started = post(
        &app,
        &hotel.owner,
        &format!("{}/media/uploads", hotel.path),
        start_body("gallery", hotel.room_type, &photo),
    )
    .await;

    assert_eq!(started.status, StatusCode::BAD_GATEWAY, "{:?}", started.body);
    assert_eq!(started.body["detail"], "Uploads are unavailable; try again");
    let rows: i64 = sqlx::query_scalar("select count(*) from media_asset").fetch_one(&superuser).await.unwrap();
    assert_eq!(rows, 0);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_start_is_checked_before_media_svc_is_called(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, "owner@example.com", "GAL").await;
    let uploads = format!("{}/media/uploads", hotel.path);
    let photo = fixture("small.jpg");
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let housekeeper = app.staff(&superuser, &hotel.owner, "hk@example.com", "housekeeping").await;

    let mut too_big = start_body("gallery", hotel.room_type, &photo);
    too_big["bytes"] = json!(26_214_401);
    let mut bad_sha = start_body("gallery", hotel.room_type, &photo);
    bad_sha["sha256"] = json!("not hex");
    let mut no_kind = start_body("guest_document", hotel.guest, &photo);
    no_kind["filename"] = json!("passport.jpg");
    let mut long_name = start_body("gallery", hotel.room_type, &photo);
    long_name["filename"] = json!("x".repeat(256));
    for (body, status) in [
        (too_big, StatusCode::UNPROCESSABLE_ENTITY),
        (bad_sha, StatusCode::UNPROCESSABLE_ENTITY),
        (no_kind, StatusCode::UNPROCESSABLE_ENTITY),
        (long_name, StatusCode::UNPROCESSABLE_ENTITY),
        (start_body("gallery", Uuid::now_v7(), &photo), StatusCode::NOT_FOUND),
    ] {
        let response = post(&app, &hotel.owner, &uploads, body.clone()).await;
        assert_eq!(response.status, status, "{body}: {:?}", response.body);
    }
    // A housekeeper reports issues but manages no gallery.
    let gallery = post(&app, &housekeeper, &uploads, start_body("gallery", hotel.room_type, &photo)).await;
    assert_eq!(gallery.status, StatusCode::FORBIDDEN, "{:?}", gallery.body);
}

/// A ready asset written straight to the database, with `renditions` (`[(size, format)]`) and an archive put in
/// storage under its keys; linked by `link`, an SQL insert taking the tenant ($1) and the asset ($2).
#[allow(clippy::too_many_arguments)]
async fn ready_asset(
    superuser: &PgPool,
    storage: &Storage,
    hotel: &Hotel,
    purpose: &str,
    mime: &str,
    renditions: &[(&str, &str)],
    archive_ext: &str,
    link: &str,
) -> Uuid {
    let asset = Uuid::now_v7();
    let private = purpose != "gallery";
    let rendition_bucket = if private { Bucket::Private } else { Bucket::Public };
    let mut out = Vec::new();
    for (size, format) in renditions {
        let key = storage::rendition_key(hotel.tenant, asset, size, format);
        storage.put(rendition_bucket, &key, format!("{size}.{format}").into_bytes(), "image/webp", None).await.unwrap();
        out.push(json!({"size": size, "format": format, "width": 320, "height": 240, "bytes": 9, "key": key}));
    }
    let archive = storage::archive_key(hotel.tenant, asset, archive_ext);
    storage.put(Bucket::Private, &archive, b"%PDF-1.4 archived".to_vec(), "application/pdf", None).await.unwrap();
    sqlx::query(
        "insert into media_asset (id, tenant_id, property_id, purpose, visibility, status, filename, declared_bytes,
                                  sha256, blake3, mime, bytes, archive_key, renditions, ready_at)
         values ($1, $2, $3, $4, $5, 'ready', 'file', 17, $6, $7, $8, 17, $9, $10, now())",
    )
    .bind(asset)
    .bind(hotel.tenant)
    .bind(hotel.id)
    .bind(purpose)
    .bind(if private { "private" } else { "public" })
    .bind(&Sha256::digest(asset.as_bytes())[..])
    .bind(&Sha256::digest(asset.as_bytes())[..])
    .bind(mime)
    .bind(archive)
    .bind(Value::Array(out))
    .execute(superuser)
    .await
    .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(link)).bind(hotel.tenant).bind(asset).execute(superuser).await.unwrap();
    asset
}

/// A ready PDF guest document of the hotel's guest; returns the document's id.
async fn guest_document(superuser: &PgPool, storage: &Storage, hotel: &Hotel) -> Uuid {
    let link = format!(
        "insert into guest_document (id, tenant_id, guest_id, asset_id, kind) values ('{}', $1, '{}', $2, 'passport')",
        Uuid::now_v7(),
        hotel.guest
    );
    let asset = ready_asset(superuser, storage, hotel, "guest_document", "application/pdf", &[], "pdf", &link).await;
    sqlx::query_scalar("select id from guest_document where asset_id = $1")
        .bind(asset)
        .fetch_one(superuser)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn guest_document_link_expires_and_audits(_: PgPoolOptions, opts: PgConnectOptions) {
    let superuser = PgPool::connect_with(opts.clone()).await.unwrap();
    let app = TestApp::new(opts).await;
    let storage = storage().await;
    let hotel = Hotel::new(&app, "owner@example.com", "GAL").await;
    let desk = app.staff(&superuser, &hotel.owner, "desk@example.com", "front_desk").await;
    let document = guest_document(&superuser, &storage, &hotel).await;
    let link = format!("{}/guest-documents/{document}/link?size=original", hotel.path);

    let first = app.send(Method::GET, &link, Some(&desk), None).await;
    let second = app.send(Method::GET, &link, Some(&desk), None).await;

    assert_eq!(first.status, StatusCode::OK, "{:?}", first.body);
    assert_eq!(second.status, StatusCode::OK);
    let url = first.body["url"].as_str().unwrap();
    assert!(url.contains("X-Amz-Expires=300"), "a 5-minute link: {url}");
    let response = http().get(url).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.bytes().await.unwrap().as_ref(), b"%PDF-1.4 archived");
    let viewed: i64 =
        sqlx::query_scalar("select count(*) from audit_log where action = 'guest_document.viewed' and entity_id = $1")
            .bind(document)
            .fetch_one(&superuser)
            .await
            .unwrap();
    assert_eq!(viewed, 2, "one audit entry per link issued");

    // Storage refuses a link once it has expired.
    let key: String = sqlx::query_scalar(
        "select a.archive_key from guest_document d join media_asset a on a.id = d.asset_id where d.id = $1",
    )
    .bind(document)
    .fetch_one(&superuser)
    .await
    .unwrap();
    let short = storage.presign_get(Bucket::Private, &key, Duration::from_secs(1));
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(http().get(short).send().await.unwrap().status(), 403);

    // An image's original is its JPEG XL archive, which browsers cannot show.
    let md = app.send(Method::GET, &link.replace("original", "md"), Some(&desk), None).await;
    assert_eq!(md.status, StatusCode::UNPROCESSABLE_ENTITY, "a PDF has no md rendition: {:?}", md.body);

    let deleted =
        app.send(Method::DELETE, &format!("{}/guest-documents/{document}", hotel.path), Some(&desk), None).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{:?}", deleted.body);
    let after = app.send(Method::GET, &link, Some(&desk), None).await;
    assert_eq!(after.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn housekeeper_cannot_list_or_link_guest_documents(_: PgPoolOptions, opts: PgConnectOptions) {
    let superuser = PgPool::connect_with(opts.clone()).await.unwrap();
    let app = TestApp::new(opts).await;
    let storage = storage().await;
    let hotel = Hotel::new(&app, "owner@example.com", "GAL").await;
    let housekeeper = app.staff(&superuser, &hotel.owner, "hk@example.com", "housekeeping").await;
    let desk = app.staff(&superuser, &hotel.owner, "desk@example.com", "front_desk").await;
    let document = guest_document(&superuser, &storage, &hotel).await;
    let variables = json!({"p": hotel.id, "g": hotel.guest});

    let listed = graphql(&app, &housekeeper, GUEST_DOCUMENTS, variables.clone()).await;
    let link = app
        .send(
            Method::GET,
            &format!("{}/guest-documents/{document}/link?size=original", hotel.path),
            Some(&housekeeper),
            None,
        )
        .await;
    let delete =
        app.send(Method::DELETE, &format!("{}/guest-documents/{document}", hotel.path), Some(&housekeeper), None).await;

    assert_eq!(listed["errors"][0]["message"], "you do not have permission for this property", "{listed:?}");
    assert_eq!(link.status, StatusCode::FORBIDDEN, "{:?}", link.body);
    assert_eq!(delete.status, StatusCode::FORBIDDEN, "{:?}", delete.body);
    let allowed = graphql(&app, &desk, GUEST_DOCUMENTS, variables).await;
    let documents = &allowed["data"]["guestDocuments"];
    assert_eq!(documents.as_array().unwrap().len(), 1, "{allowed:?}");
    assert_eq!(documents[0]["id"], json!(document));
    assert_eq!(documents[0]["kind"], "PASSPORT");
    assert_eq!(documents[0]["status"], "READY");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn no_graphql_field_exposes_guest_document_urls(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;

    let body = graphql(&app, &owner, r#"{ __type(name: "GuestDocumentNode") { fields { name } } }"#, json!({})).await;

    let fields: Vec<&str> =
        body["data"]["__type"]["fields"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert_eq!(fields, ["id", "kind", "status", "mime", "uploadedAt", "uploadedBy"]);
    for field in fields {
        let lower = field.to_lowercase();
        assert!(!["url", "link", "key", "href", "src"].iter().any(|bad| lower.contains(bad)), "{field}");
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn issue_photo_url_is_signed_and_private(_: PgPoolOptions, opts: PgConnectOptions) {
    let superuser = PgPool::connect_with(opts.clone()).await.unwrap();
    let app = TestApp::new(opts).await;
    let storage = storage().await;
    let hotel = Hotel::new(&app, "owner@example.com", "GAL").await;
    let housekeeper = app.staff(&superuser, &hotel.owner, "hk@example.com", "housekeeping").await;
    let link =
        format!("insert into room_issue_media (tenant_id, issue_id, asset_id) values ($1, '{}', $2)", hotel.issue);
    let asset =
        ready_asset(&superuser, &storage, &hotel, "issue_photo", "image/jpeg", &[("thumb", "webp")], "jxl", &link)
            .await;

    let body = graphql(&app, &housekeeper, ISSUE_PHOTOS, json!({"p": hotel.id})).await;

    let photo = &body["data"]["roomIssues"][0]["photos"][0];
    assert_eq!(photo["assetId"], json!(asset), "{body:?}");
    let url = photo["renditions"][0]["url"].as_str().unwrap();
    assert!(url.contains("X-Amz-Signature") && url.contains("X-Amz-Expires=3600"), "{url}");
    assert_eq!(http().get(url).send().await.unwrap().status(), 200);
    let key = storage::rendition_key(hotel.tenant, asset, "thumb", "webp");
    let anonymous = http().get(storage.public_url(&key)).send().await.unwrap();
    assert!(!anonymous.status().is_success(), "an issue photo is never public");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn only_the_uploader_or_a_supervisor_removes_an_issue_photo(_: PgPoolOptions, opts: PgConnectOptions) {
    let superuser = PgPool::connect_with(opts.clone()).await.unwrap();
    let app = TestApp::new(opts).await;
    let storage = storage().await;
    let hotel = Hotel::new(&app, "owner@example.com", "GAL").await;
    let uploader = app.staff(&superuser, &hotel.owner, "hk@example.com", "housekeeping").await;
    let colleague = app.staff(&superuser, &hotel.owner, "hk2@example.com", "housekeeping").await;
    let supervisor = app.staff(&superuser, &hotel.owner, "sup@example.com", "hk_supervisor").await;
    let uploader_id = uuid(&app.send(Method::GET, "/api/v1/me", Some(&uploader), None).await.body["user_id"]);
    let link =
        format!("insert into room_issue_media (tenant_id, issue_id, asset_id) values ($1, '{}', $2)", hotel.issue);
    let mut photos = Vec::new();
    for _ in 0..2 {
        let asset = ready_asset(&superuser, &storage, &hotel, "issue_photo", "image/jpeg", &[], "jxl", &link).await;
        sqlx::query("update media_asset set created_by = $2 where id = $1")
            .bind(asset)
            .bind(uploader_id)
            .execute(&superuser)
            .await
            .unwrap();
        photos.push(asset);
    }
    let remove = |asset: Uuid| format!("{}/room-issues/{}/media/{asset}", hotel.path, hotel.issue);

    let by_colleague = app.send(Method::DELETE, &remove(photos[0]), Some(&colleague), None).await;
    let by_uploader = app.send(Method::DELETE, &remove(photos[0]), Some(&uploader), None).await;
    let by_supervisor = app.send(Method::DELETE, &remove(photos[1]), Some(&supervisor), None).await;

    assert_eq!(by_colleague.status, StatusCode::FORBIDDEN, "{:?}", by_colleague.body);
    assert_eq!(by_uploader.status, StatusCode::NO_CONTENT, "{:?}", by_uploader.body);
    assert_eq!(by_supervisor.status, StatusCode::NO_CONTENT, "{:?}", by_supervisor.body);
    let body = graphql(&app, &supervisor, ISSUE_PHOTOS, json!({"p": hotel.id})).await;
    assert_eq!(body["data"]["roomIssues"][0]["photos"], json!([]));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn guest_documents_stay_with_the_property_they_were_uploaded_at(_: PgPoolOptions, opts: PgConnectOptions) {
    let superuser = PgPool::connect_with(opts.clone()).await.unwrap();
    let app = TestApp::new(opts).await;
    let storage = storage().await;
    let hotel = Hotel::new(&app, "owner@example.com", "GAL").await;
    let other = json!({"code": "KAN", "name": "Kandy", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let other = uuid(&post(&app, &hotel.owner, "/api/v1/properties", other).await.body["id"]);
    let other_path = format!("/api/v1/properties/{other}");
    // A front desk clerk of the first property only.
    let desk = app.staff(&superuser, &hotel.owner, "desk@example.com", "front_desk").await;
    sqlx::query(
        "update role_grant set property_id = $1
         where tenant_id = $3 and user_id = (select id from app_user where email = $2)",
    )
    .bind(hotel.id)
    .bind("desk@example.com")
    .bind(hotel.tenant)
    .execute(&superuser)
    .await
    .unwrap();
    let here = guest_document(&superuser, &storage, &hotel).await;
    let there = guest_document(&superuser, &storage, &hotel).await;
    sqlx::query(
        "update media_asset set property_id = $2 where id = (select asset_id from guest_document where id = $1)",
    )
    .bind(there)
    .bind(other)
    .execute(&superuser)
    .await
    .unwrap();
    let link = |path: &str, document: Uuid| format!("{path}/guest-documents/{document}/link?size=original");
    let delete = |path: &str, document: Uuid| format!("{path}/guest-documents/{document}");
    let listed = |body: &Value| -> Vec<Value> {
        body["data"]["guestDocuments"].as_array().unwrap().iter().map(|d| d["id"].clone()).collect()
    };

    // The clerk: the other property's document is not found here, and the other property is not theirs.
    for (path, status) in [(hotel.path.as_str(), StatusCode::NOT_FOUND), (other_path.as_str(), StatusCode::FORBIDDEN)] {
        let linked = app.send(Method::GET, &link(path, there), Some(&desk), None).await;
        let deleted = app.send(Method::DELETE, &delete(path, there), Some(&desk), None).await;
        assert_eq!((linked.status, deleted.status), (status, status), "{path}: {:?} {:?}", linked.body, deleted.body);
    }
    let here_list = graphql(&app, &desk, GUEST_DOCUMENTS, json!({"p": hotel.id, "g": hotel.guest})).await;
    assert_eq!(listed(&here_list), [json!(here)]);
    let there_list = graphql(&app, &desk, GUEST_DOCUMENTS, json!({"p": other, "g": hotel.guest})).await;
    assert_eq!(there_list["errors"][0]["message"], "you do not have permission for this property");

    // The owner sees each document only through its own property.
    let owner = &hotel.owner;
    assert_eq!(app.send(Method::GET, &link(&hotel.path, there), Some(owner), None).await.status, StatusCode::NOT_FOUND);
    assert_eq!(app.send(Method::GET, &link(&other_path, here), Some(owner), None).await.status, StatusCode::NOT_FOUND);
    assert_eq!(app.send(Method::GET, &link(&other_path, there), Some(owner), None).await.status, StatusCode::OK);
    let there_list = graphql(&app, owner, GUEST_DOCUMENTS, json!({"p": other, "g": hotel.guest})).await;
    assert_eq!(listed(&there_list), [json!(there)]);
    let deleted = app.send(Method::DELETE, &delete(&hotel.path, there), Some(owner), None).await;
    assert_eq!(deleted.status, StatusCode::NOT_FOUND);
    let deleted = app.send(Method::DELETE, &delete(&other_path, there), Some(owner), None).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn gallery_order_caption_and_removal(_: PgPoolOptions, opts: PgConnectOptions) {
    let superuser = PgPool::connect_with(opts.clone()).await.unwrap();
    let app = TestApp::new(opts).await;
    let storage = storage().await;
    let hotel = Hotel::new(&app, "owner@example.com", "GAL").await;
    let manager = app.staff(&superuser, &hotel.owner, "mgr@example.com", "manager").await;
    let desk = app.staff(&superuser, &hotel.owner, "desk@example.com", "front_desk").await;
    let mut photos = Vec::new();
    for position in 0..2 {
        let link = format!(
            "insert into room_type_media (tenant_id, room_type_id, asset_id, position) values ($1, '{}', $2, {position})",
            hotel.room_type
        );
        photos.push(
            ready_asset(&superuser, &storage, &hotel, "gallery", "image/jpeg", &[("thumb", "webp")], "jxl", &link)
                .await,
        );
    }
    let media = format!("{}/room-types/{}/media", hotel.path, hotel.room_type);

    let reorder = json!({"asset_ids": [photos[1], photos[0]]});
    assert_eq!(
        app.send(Method::PUT, &format!("{media}/order"), Some(&desk), Some(reorder.clone())).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(Method::PUT, &format!("{media}/order"), Some(&manager), Some(reorder)).await.status,
        StatusCode::NO_CONTENT
    );
    let caption = app
        .send(Method::PATCH, &format!("{media}/{}", photos[0]), Some(&manager), Some(json!({"caption": "Sea view"})))
        .await;
    assert_eq!(caption.status, StatusCode::NO_CONTENT, "{:?}", caption.body);
    let too_long = app
        .send(
            Method::PATCH,
            &format!("{media}/{}", photos[0]),
            Some(&manager),
            Some(json!({"caption": "x".repeat(201)})),
        )
        .await;
    assert_eq!(too_long.status, StatusCode::UNPROCESSABLE_ENTITY);

    let body = graphql(&app, &desk, GALLERY, json!({"p": hotel.id})).await;
    let gallery = body["data"]["roomTypes"][0]["media"].as_array().unwrap().clone();
    assert_eq!(gallery.iter().map(|m| m["assetId"].clone()).collect::<Vec<_>>(), [json!(photos[1]), json!(photos[0])]);
    assert_eq!((gallery[0]["caption"].clone(), gallery[1]["caption"].clone()), (Value::Null, json!("Sea view")));
    let url = gallery[0]["renditions"][0]["url"].as_str().unwrap();
    assert_eq!(url, storage.public_url(&storage::rendition_key(hotel.tenant, photos[1], "thumb", "webp")).as_str());

    let removed = app.send(Method::DELETE, &format!("{media}/{}", photos[1]), Some(&manager), None).await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT);
    let body = graphql(&app, &desk, GALLERY, json!({"p": hotel.id})).await;
    assert_eq!(body["data"]["roomTypes"][0]["media"].as_array().unwrap().len(), 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn cross_tenant_asset_is_404(_: PgPoolOptions, opts: PgConnectOptions) {
    let superuser = PgPool::connect_with(opts.clone()).await.unwrap();
    let app = TestApp::new(opts).await;
    let storage = storage().await;
    let hotel = Hotel::new(&app, "owner@example.com", "GAL").await;
    let intruder = Hotel::new(&app, "intruder@example.com", "INT").await;
    let gallery_link = format!(
        "insert into room_type_media (tenant_id, room_type_id, asset_id, position) values ($1, '{}', $2, 0)",
        hotel.room_type
    );
    let photo = ready_asset(&superuser, &storage, &hotel, "gallery", "image/jpeg", &[], "jxl", &gallery_link).await;
    let issue_link =
        format!("insert into room_issue_media (tenant_id, issue_id, asset_id) values ($1, '{}', $2)", hotel.issue);
    let issue_photo =
        ready_asset(&superuser, &storage, &hotel, "issue_photo", "image/jpeg", &[], "jxl", &issue_link).await;
    let document = guest_document(&superuser, &storage, &hotel).await;
    let upload = Uuid::now_v7();
    sqlx::query(
        "insert into media_asset (id, tenant_id, property_id, purpose, visibility, filename, declared_bytes, sha256,
                                  s3_upload_id, pending_target)
         values ($1, $2, $3, 'gallery', 'public', 'x.jpg', 10, $4, 'upload-id', $5)",
    )
    .bind(upload)
    .bind(hotel.tenant)
    .bind(hotel.id)
    .bind(&[7u8; 32][..])
    .bind(json!({"kind": "room_type", "id": hotel.room_type}))
    .execute(&superuser)
    .await
    .unwrap();
    let small = fixture("small.jpg");

    // Through the other tenant's property, and through the intruder's own property with the other tenant's ids.
    for path in [hotel.path.as_str(), intruder.path.as_str()] {
        let rt = format!("{path}/room-types/{}/media", hotel.room_type);
        let responses = [
            post(
                &app,
                &intruder.owner,
                &format!("{path}/media/uploads"),
                start_body("gallery", hotel.room_type, &small),
            )
            .await,
            post(
                &app,
                &intruder.owner,
                &format!("{path}/media/uploads/{upload}/complete"),
                json!({"parts": [{"number": 1, "etag": "x"}]}),
            )
            .await,
            app.send(Method::DELETE, &format!("{path}/media/uploads/{upload}"), Some(&intruder.owner), None).await,
            app.send(Method::PUT, &format!("{rt}/order"), Some(&intruder.owner), Some(json!({"asset_ids": [photo]})))
                .await,
            app.send(Method::PATCH, &format!("{rt}/{photo}"), Some(&intruder.owner), Some(json!({"caption": "Mine"})))
                .await,
            app.send(Method::DELETE, &format!("{rt}/{photo}"), Some(&intruder.owner), None).await,
            app.send(
                Method::DELETE,
                &format!("{path}/room-issues/{}/media/{issue_photo}", hotel.issue),
                Some(&intruder.owner),
                None,
            )
            .await,
            app.send(
                Method::GET,
                &format!("{path}/guest-documents/{document}/link?size=original"),
                Some(&intruder.owner),
                None,
            )
            .await,
            app.send(Method::DELETE, &format!("{path}/guest-documents/{document}"), Some(&intruder.owner), None).await,
        ];
        for (index, response) in responses.iter().enumerate() {
            assert_eq!(response.status, StatusCode::NOT_FOUND, "{path} route {index}: {:?}", response.body);
        }
    }
    let documents = graphql(&app, &intruder.owner, GUEST_DOCUMENTS, json!({"p": intruder.id, "g": hotel.guest})).await;
    assert_eq!(documents["data"]["guestDocuments"], json!([]));
    let gallery = graphql(&app, &hotel.owner, GALLERY, json!({"p": hotel.id})).await;
    assert_eq!(gallery["data"]["roomTypes"][0]["media"][0]["assetId"], json!(photo), "untouched");
}

/// A `media-results` push for `result`, delivered as message `id`.
fn envelope(id: Uuid, result: &Value) -> Value {
    serde_json::to_value(PushEnvelope {
        message: PushMessage {
            data: STANDARD.encode(serde_json::to_vec(result).unwrap()),
            message_id: Uuid::now_v7().to_string(),
            attributes: BTreeMap::from([("message_id".to_owned(), id.to_string())]),
        },
        subscription: "push/media-results".to_owned(),
    })
    .unwrap()
}

async fn push(app: &TestApp, body: &str, authorization: Option<&str>) -> StatusCode {
    let mut request = Request::post("/internal/events").header(header::CONTENT_TYPE, "application/json");
    if let Some(authorization) = authorization {
        request = request.header(header::AUTHORIZATION, authorization);
    }
    app.router.clone().oneshot(request.body(Body::from(body.to_owned())).unwrap()).await.unwrap().status()
}

/// An app whose `/internal/events` accepts the test secret.
async fn internal_app(opts: PgConnectOptions) -> TestApp {
    let pool = db::testing::app_pool(opts, 5).await;
    let unused = "127.0.0.1:9".parse().unwrap();
    let state =
        AppState::with_media(pool.clone(), false, db::testing::guest_id_keys(), services(storage().await, unused));
    TestApp { router: router(state.clone()), state, pool }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn internal_events_refuses_without_secret(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = internal_app(opts).await;
    let failed = json!({"type": "failed", "tenant_id": Uuid::now_v7(), "asset_id": Uuid::now_v7(), "reason": "x"});
    let body = envelope(Uuid::now_v7(), &failed).to_string();

    assert_eq!(push(&app, &body, None).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        push(&app, &body, Some("Bearer wrong-secret-wrong-secret-wrong-secret")).await,
        StatusCode::UNAUTHORIZED
    );
    // The session cookie and CSRF header mean nothing here.
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let with_session =
        app.send_raw(Method::POST, "/internal/events", Some(&owner), Some("application/json"), &body).await;
    assert_eq!(with_session.status, StatusCode::UNAUTHORIZED);
    assert_eq!(push(&app, &body, Some(&format!("Bearer {SECRET}"))).await, StatusCode::NO_CONTENT);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn internal_events_dedupes(_: PgPoolOptions, opts: PgConnectOptions) {
    let superuser = PgPool::connect_with(opts.clone()).await.unwrap();
    let app = internal_app(opts).await;
    let hotel = Hotel::new(&app, "owner@example.com", "GAL").await;
    let asset = Uuid::now_v7();
    sqlx::query(
        "insert into media_asset (id, tenant_id, property_id, purpose, visibility, status, filename, declared_bytes,
                                  sha256, processing_at)
         values ($1, $2, $3, 'gallery', 'public', 'processing', 'x.jpg', 10, $4, now())",
    )
    .bind(asset)
    .bind(hotel.tenant)
    .bind(hotel.id)
    .bind(&[7u8; 32][..])
    .execute(&superuser)
    .await
    .unwrap();
    let failed = json!({"type": "failed", "tenant_id": hotel.tenant, "asset_id": asset, "reason": "This file could not be read"});
    let body = envelope(Uuid::now_v7(), &failed).to_string();
    let auth = format!("Bearer {SECRET}");
    let status = || sqlx::query_scalar::<_, String>("select status from media_asset where id = $1").bind(asset);

    assert_eq!(push(&app, &body, Some(&auth)).await, StatusCode::NO_CONTENT);
    assert_eq!(status().fetch_one(&superuser).await.unwrap(), "failed");
    sqlx::query("update media_asset set status = 'processing' where id = $1")
        .bind(asset)
        .execute(&superuser)
        .await
        .unwrap();
    assert_eq!(push(&app, &body, Some(&auth)).await, StatusCode::NO_CONTENT, "a duplicate is acknowledged");
    assert_eq!(status().fetch_one(&superuser).await.unwrap(), "processing", "and not applied again");

    // A message that can never be applied is refused for good.
    assert_eq!(push(&app, "not json", Some(&auth)).await, StatusCode::BAD_REQUEST);
    let not_a_result = envelope(Uuid::now_v7(), &json!({"type": "something else"})).to_string();
    assert_eq!(push(&app, &not_a_result, Some(&auth)).await, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_sweep_removes_an_orphans_objects_from_their_buckets(_: PgPoolOptions, opts: PgConnectOptions) {
    let superuser = PgPool::connect_with(opts.clone()).await.unwrap();
    let app = TestApp::new(opts.clone()).await;
    let storage = storage().await;
    let hotel = Hotel::new(&app, "owner@example.com", "GAL").await;
    let orphan =
        ready_asset(&superuser, &storage, &hotel, "gallery", "image/jpeg", &[("md", "avif")], "jxl", "select $1, $2")
            .await;
    sqlx::query("update media_asset set created_at = now() - interval '2 days' where id = $1")
        .bind(orphan)
        .execute(&superuser)
        .await
        .unwrap();
    let rendition = storage::rendition_key(hotel.tenant, orphan, "md", "avif");
    let archive = storage::archive_key(hotel.tenant, orphan, "jxl");
    storage.put(Bucket::Private, &upload_key(hotel.tenant, orphan), b"raw".to_vec(), "image/jpeg", None).await.unwrap();

    // Another session holding the lock means another instance is sweeping.
    let mut holder = sqlx::PgConnection::connect_with(&opts).await.unwrap();
    sqlx::query("select pg_advisory_lock(hashtext('media_sweep'))").execute(&mut holder).await.unwrap();
    assert!(
        core_api::tasks::sweep_once(&app.pool, &opts, &storage, &CancellationToken::new()).await.unwrap().is_none()
    );
    sqlx::query("select pg_advisory_unlock(hashtext('media_sweep'))").execute(&mut holder).await.unwrap();

    let report =
        core_api::tasks::sweep_once(&app.pool, &opts, &storage, &CancellationToken::new()).await.unwrap().unwrap();

    assert_eq!(report.orphans.len(), 1);
    assert_eq!(storage.head(Bucket::Public, &rendition).await.unwrap(), None);
    assert_eq!(storage.head(Bucket::Private, &archive).await.unwrap(), None);
    assert_eq!(storage.head(Bucket::Private, &upload_key(hotel.tenant, orphan)).await.unwrap(), None);
}
