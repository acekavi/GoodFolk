mod common;

use common::{Hotel, asset_of, guest_target, ready, verified};
use db::{Scope, begin};
use media::{
    AssetStatus, Completed, MediaError, Purpose, Target, apply_result, complete_upload, gallery, guest_documents,
    issue_photos,
};
use media_types::MediaResult;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::time::Duration;
use uuid::Uuid;

const MIB: u64 = 1024 * 1024;

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn start_upload_checks_target_property_and_limits(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let other_property = hotel.add_property("OTH").await;
    let foreign_room_type = hotel.add_room_type(other_property, "STD").await;
    let guest = hotel.guest().await;

    let not_found = [
        (Target::RoomType(foreign_room_type), "room type"),
        (Target::RoomType(Uuid::now_v7()), "room type"),
        (Target::RoomIssue(Uuid::now_v7()), "issue"),
        (guest_target(Uuid::now_v7()), "guest"),
    ];
    for (target, what) in not_found {
        let err = hotel.start(target, 1000).await.unwrap_err();
        assert!(matches!(err, MediaError::NotFound(w) if w == what), "{target:?}: {err:?}");
    }
    // A guest is tenant-wide, but the asset still needs a property of the tenant.
    let err = hotel.start_in(Uuid::now_v7(), guest_target(guest), 1000).await.unwrap_err();
    assert!(matches!(err, MediaError::NotFound("property")), "{err:?}");

    let err = hotel.start(Target::RoomType(hotel.room_type), 25 * MIB + 1).await.unwrap_err();
    assert!(matches!(&err, MediaError::Invalid(m) if m == "Too large: the limit is 25 MB"), "{err:?}");
    let err = hotel.start(guest_target(guest), 15 * MIB + 1).await.unwrap_err();
    assert!(matches!(&err, MediaError::Invalid(m) if m == "Too large: the limit is 15 MB"), "{err:?}");
    assert!(matches!(hotel.start(Target::RoomType(hotel.room_type), 0).await, Err(MediaError::Invalid(_))));
    let mut tx = hotel.tx().await;
    let long_name = "x".repeat(256);
    let err = media::start_upload(
        &mut tx,
        hotel.tenant,
        hotel.user,
        hotel.property,
        Target::RoomType(hotel.room_type),
        &long_name,
        1000,
        [0; 32],
    )
    .await
    .unwrap_err();
    assert!(matches!(err, MediaError::Invalid(_)), "{err:?}");
    drop(tx);
    assert_eq!(hotel.assets().await, 0, "nothing refused was stored");

    let gallery_upload = hotel.start(Target::RoomType(hotel.room_type), 25 * MIB).await.unwrap();
    let document_upload = hotel.start(guest_target(guest), 15 * MIB).await.unwrap();
    assert_eq!((gallery_upload.purpose, document_upload.purpose), (Purpose::Gallery, Purpose::GuestDocument));
    let rows: Vec<(String, String, String, i64)> =
        sqlx::query_as("select purpose, visibility, status, declared_bytes from media_asset order by id")
            .fetch_all(&mut *hotel.tx().await)
            .await
            .unwrap();
    assert_eq!(
        rows,
        [
            ("gallery".into(), "public".into(), "uploading".into(), (25 * MIB) as i64),
            ("guest_document".into(), "private".into(), "uploading".into(), (15 * MIB) as i64),
        ]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn sixth_issue_photo_is_conflict(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let issue = hotel.issue().await;

    // Two finished photos and three uploads still in progress count alike.
    for file in 1..=2 {
        hotel.upload(Target::RoomIssue(issue), file).await;
    }
    for _ in 0..3 {
        hotel.start(Target::RoomIssue(issue), 1000).await.unwrap();
    }
    let err = hotel.start(Target::RoomIssue(issue), 1000).await.unwrap_err();
    assert!(matches!(&err, MediaError::Conflict(m) if m == "An issue can have at most 5 photos"), "{err:?}");

    let other_issue = hotel.issue().await;
    hotel.start(Target::RoomIssue(other_issue), 1000).await.unwrap();
    let photos = issue_photos(&mut hotel.tx().await, &[issue, other_issue]).await.unwrap();
    assert_eq!(photos[&issue].len(), 2);
    assert!(!photos.contains_key(&other_issue), "an upload in progress is not a photo yet");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn parallel_starts_never_exceed_five_issue_photos(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = std::sync::Arc::new(Hotel::new(opts).await);
    let issue = hotel.issue().await;
    let starts: Vec<_> = (0..8)
        .map(|_| {
            let hotel = hotel.clone();
            tokio::spawn(async move { hotel.start(Target::RoomIssue(issue), 1000).await })
        })
        .collect();
    let mut accepted = 0;
    for start in starts {
        match start.await.unwrap() {
            Ok(_) => accepted += 1,
            Err(MediaError::Conflict(_)) => {}
            Err(err) => panic!("{err:?}"),
        }
    }
    assert_eq!(accepted, 5);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn complete_writes_link_and_enqueues(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let upload = hotel.start(Target::RoomType(hotel.room_type), 1000).await.unwrap();

    let Completed::Processing(asset) = hotel.complete(upload.asset_id, 1).await.unwrap() else {
        panic!("a new file is processed")
    };
    assert_eq!(
        (asset.id, asset.status, asset.width, asset.mime.as_deref()),
        (upload.asset_id, AssetStatus::Processing, Some(4000), Some("image/jpeg"))
    );
    let items = gallery(&mut hotel.tx().await, hotel.room_type).await.unwrap();
    assert_eq!(items.iter().map(|i| (i.asset_id, i.position)).collect::<Vec<_>>(), [(asset.id, 0)]);
    let pending: Option<serde_json::Value> = sqlx::query_scalar("select pending_target from media_asset where id = $1")
        .bind(asset.id)
        .fetch_one(&mut *hotel.tx().await)
        .await
        .unwrap();
    assert_eq!(pending, None, "the pending link is cleared once written");

    let jobs = hotel.outbox("media-process").await;
    assert_eq!(
        jobs,
        [serde_json::json!({
            "tenant_id": hotel.tenant.0, "asset_id": asset.id, "purpose": "gallery", "mime": "image/jpeg"
        })]
    );
    assert_eq!(hotel.audited("media.uploaded").await, 1);

    let err = hotel.complete(upload.asset_id, 1).await.unwrap_err();
    assert!(matches!(err, MediaError::Conflict(_)), "completing twice: {err:?}");
    let other_property = hotel.add_property("OTH").await;
    let second = hotel.start(Target::RoomType(hotel.room_type), 1000).await.unwrap();
    let err = hotel.complete_in(other_property, second.asset_id, 2).await.unwrap_err();
    assert!(matches!(err, MediaError::NotFound("upload")), "{err:?}");

    // A guest document gets its document row.
    let guest = hotel.guest().await;
    let completed = hotel.upload(guest_target(guest), 3).await;
    let documents = guest_documents(&mut hotel.tx().await, guest).await.unwrap();
    assert_eq!(documents.iter().map(|d| d.asset_id).collect::<Vec<_>>(), [asset_of(&completed)]);
    assert_eq!(hotel.audited("guest_document.uploaded").await, 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn duplicate_links_existing_asset(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let suite = hotel.add_room_type(hotel.property, "STE").await;
    let first = asset_of(&hotel.upload(Target::RoomType(hotel.room_type), 1).await);

    let upload = hotel.start(Target::RoomType(suite), 1000).await.unwrap();
    let Completed::Duplicate { asset, upload_key } = hotel.complete(upload.asset_id, 1).await.unwrap() else {
        panic!("the same file is a duplicate")
    };
    assert_eq!(asset.id, first);
    assert_eq!(upload_key, format!("t/{}/a/{}/upload", hotel.tenant.0, upload.asset_id));
    assert_eq!(hotel.assets().await, 1, "the duplicate's row is gone");
    assert_eq!(hotel.status(upload.asset_id).await, None);
    let in_suite = gallery(&mut hotel.tx().await, suite).await.unwrap();
    assert_eq!(in_suite.iter().map(|i| i.asset_id).collect::<Vec<_>>(), [first]);
    assert_eq!(hotel.outbox("media-process").await.len(), 1, "a processing duplicate is not queued again");
    assert_eq!(hotel.audited("media.uploaded").await, 2);

    // Another purpose stores the same file separately.
    let guest = hotel.guest().await;
    assert!(matches!(hotel.upload(guest_target(guest), 1).await, Completed::Processing(_)));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn concurrent_duplicates_leave_one_asset(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = std::sync::Arc::new(Hotel::new(opts).await);
    let suite = hotel.add_room_type(hotel.property, "STE").await;
    let a = hotel.start(Target::RoomType(hotel.room_type), 1000).await.unwrap().asset_id;
    let b = hotel.start(Target::RoomType(suite), 1000).await.unwrap().asset_id;

    // The first complete holds the BLAKE3 uncommitted while the second runs on its own connection.
    let mut first = begin(&hotel.pool, Scope::tenant(hotel.tenant)).await.unwrap();
    let won = complete_upload(&mut first, hotel.tenant, hotel.user, hotel.property, a, verified(9)).await.unwrap();
    assert!(matches!(won, Completed::Processing(_)));
    let second = {
        let hotel = hotel.clone();
        tokio::spawn(async move {
            let mut tx = begin(&hotel.pool, Scope::tenant(hotel.tenant)).await.unwrap();
            let completed = complete_upload(&mut tx, hotel.tenant, hotel.user, hotel.property, b, verified(9)).await;
            tx.commit().await.unwrap();
            completed
        })
    };
    // Wait until the second is blocked on the first's index entry, then let the first commit.
    let mut waited = 0;
    loop {
        let blocked: i64 = sqlx::query_scalar("select count(*) from pg_locks where not granted")
            .fetch_one(&hotel.owner)
            .await
            .unwrap();
        if blocked > 0 {
            break;
        }
        waited += 1;
        assert!(waited < 500, "the second complete never waited on the first");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    first.commit().await.unwrap();

    let Completed::Duplicate { asset, upload_key } = second.await.unwrap().unwrap() else {
        panic!("the loser is a duplicate")
    };
    assert_eq!(asset.id, a);
    assert_eq!(upload_key, format!("t/{}/a/{b}/upload", hotel.tenant.0));
    assert_eq!(hotel.assets().await, 1);
    let links: i64 = sqlx::query_scalar("select count(*) from room_type_media where asset_id = $1")
        .bind(a)
        .fetch_one(&mut *hotel.tx().await)
        .await
        .unwrap();
    assert_eq!(links, 2);
    assert_eq!(hotel.outbox("media-process").await.len(), 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn other_tenant_same_file_gets_own_asset(_: PgPoolOptions, opts: PgConnectOptions) {
    let ours = Hotel::new(opts.clone()).await;
    let theirs = Hotel::new(opts).await;

    let mine = ours.upload(Target::RoomType(ours.room_type), 1).await;
    let their = theirs.upload(Target::RoomType(theirs.room_type), 1).await;

    assert!(matches!(mine, Completed::Processing(_)));
    assert!(matches!(their, Completed::Processing(_)), "files are never matched across tenants");
    assert_ne!(asset_of(&mine), asset_of(&their));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn other_property_same_file_gets_own_asset(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let other_property = hotel.add_property("OTH").await;
    let other_room_type = hotel.add_room_type(other_property, "STD").await;

    let here = hotel.upload(Target::RoomType(hotel.room_type), 1).await;
    let upload = hotel.start_in(other_property, Target::RoomType(other_room_type), 1000).await.unwrap();
    let there = hotel.complete_in(other_property, upload.asset_id, 1).await.unwrap();

    assert!(matches!(here, Completed::Processing(_)));
    assert!(matches!(there, Completed::Processing(_)), "files are matched only within a property");
    assert_ne!(asset_of(&here), asset_of(&there));
    assert_eq!(hotel.assets().await, 2);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_result_naming_another_assets_objects_fails_the_asset(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let asset = asset_of(&hotel.upload(Target::RoomType(hotel.room_type), 1).await);
    let victim = Uuid::now_v7();
    let tampered = |rewrite: &dyn Fn(&mut MediaResult)| {
        let mut result = ready(hotel.tenant, asset);
        rewrite(&mut result);
        result
    };
    let bad = [
        tampered(&|r| {
            if let MediaResult::Ready { archive_key, .. } = r {
                *archive_key = format!("t/{}/a/{victim}/archive.jxl", hotel.tenant.0);
            }
        }),
        tampered(&|r| {
            if let MediaResult::Ready { renditions, .. } = r {
                renditions[1].key = format!("t/{}/a/{asset}/../{victim}/md.webp", hotel.tenant.0);
            }
        }),
    ];
    for result in &bad {
        hotel.apply(result).await;
        let row: (String, Option<String>, Option<String>) =
            sqlx::query_as("select status, error, archive_key from media_asset where id = $1")
                .bind(asset)
                .fetch_one(&mut *hotel.tx().await)
                .await
                .unwrap();
        assert_eq!(row, ("failed".to_owned(), Some("Processing failed".to_owned()), None));
        let mut tx = hotel.tx().await;
        sqlx::query("update media_asset set status = 'processing', error = null where id = $1")
            .bind(asset)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    hotel.ready(asset).await;
    assert_eq!(hotel.status(asset).await.as_deref(), Some("ready"), "the asset's own keys are accepted");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn failed_duplicate_is_requeued(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let issue = hotel.issue().await;
    let first = asset_of(&hotel.upload(Target::RoomIssue(issue), 1).await);
    hotel
        .apply(&MediaResult::Failed { tenant_id: hotel.tenant.0, asset_id: first, reason: "could not be read".into() })
        .await;
    assert_eq!(hotel.status(first).await.as_deref(), Some("failed"));

    let Completed::Duplicate { asset, .. } = hotel.upload(Target::RoomIssue(issue), 1).await else {
        panic!("the same file is a duplicate")
    };
    assert_eq!((asset.id, asset.status, asset.error), (first, AssetStatus::Processing, None));
    assert_eq!(hotel.outbox("media-process").await.len(), 2, "queued again");
    let photos = issue_photos(&mut hotel.tx().await, &[issue]).await.unwrap();
    assert_eq!(photos[&issue].len(), 1, "the issue already had this photo");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn apply_ready_is_idempotent(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts.clone()).await;
    let theirs = Hotel::new(opts).await;
    let asset = asset_of(&hotel.upload(Target::RoomType(hotel.room_type), 1).await);
    let ready_at = || async {
        sqlx::query_scalar::<_, Option<time::OffsetDateTime>>("select ready_at from media_asset where id = $1")
            .bind(asset)
            .fetch_one(&mut *hotel.tx().await)
            .await
            .unwrap()
    };

    hotel.ready(asset).await;
    let first = ready_at().await;
    hotel.ready(asset).await;
    hotel.apply(&MediaResult::Failed { tenant_id: hotel.tenant.0, asset_id: asset, reason: "late".into() }).await;

    let items = gallery(&mut hotel.tx().await, hotel.room_type).await.unwrap();
    assert_eq!((items[0].status, items[0].width, items[0].renditions.len()), (AssetStatus::Ready, Some(4000), 4));
    assert!(first.is_some());
    assert_eq!(ready_at().await, first, "the second result changed nothing");

    // Unknown assets, and another tenant's, are ignored.
    hotel.ready(Uuid::now_v7()).await;
    let mut tx = theirs.tx().await;
    apply_result(&mut tx, &ready(theirs.tenant, asset)).await.unwrap();
    tx.commit().await.unwrap();

    // A failure reason is cut to 200 characters.
    let failing = asset_of(&hotel.upload(Target::RoomType(hotel.room_type), 2).await);
    hotel.apply(&MediaResult::Failed { tenant_id: hotel.tenant.0, asset_id: failing, reason: "é".repeat(300) }).await;
    let error: String = sqlx::query_scalar("select error from media_asset where id = $1")
        .bind(failing)
        .fetch_one(&mut *hotel.tx().await)
        .await
        .unwrap();
    assert_eq!(error.chars().count(), 200);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn refusing_deletes_only_an_upload_in_progress(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let upload = hotel.start(Target::RoomType(hotel.room_type), 1000).await.unwrap();
    let mut tx = hotel.tx().await;
    media::set_s3_upload_id(&mut tx, hotel.property, upload.asset_id, "s3-upload").await.unwrap();
    media::refuse_upload(&mut tx, hotel.property, upload.asset_id).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(hotel.assets().await, 0);

    let done = asset_of(&hotel.upload(Target::RoomType(hotel.room_type), 1).await);
    let err = media::refuse_upload(&mut hotel.tx().await, hotel.property, done).await.unwrap_err();
    assert!(matches!(err, MediaError::NotFound("upload")), "{err:?}");
}
