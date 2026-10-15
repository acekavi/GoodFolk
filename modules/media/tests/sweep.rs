mod common;

use common::{Hotel, asset_of, guest_target, verified};
use db::{Scope, begin};
use media::{Completed, Purpose, StaleUpload, Target, complete_upload, gallery, guest_documents, sweep};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

impl Hotel {
    /// A guest with one document uploaded `days` ago, returning the guest and the document's asset.
    async fn old_document(&self, file: u8, days: i64) -> (Uuid, Uuid) {
        let guest = self.guest().await;
        let asset = asset_of(&self.upload(guest_target(guest), file).await);
        self.ready(asset).await;
        self.age(asset, days).await;
        (guest, asset)
    }

    /// Backdates the asset and its guest document by `days`.
    async fn age(&self, asset: Uuid, days: i64) {
        let mut tx = self.tx().await;
        sqlx::query("update media_asset set created_at = now() - make_interval(days => $2::int) where id = $1")
            .bind(asset)
            .bind(days)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query(
            "update guest_document set uploaded_at = now() - make_interval(days => $2::int) where asset_id = $1",
        )
        .bind(asset)
        .bind(days)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    async fn documents_of(&self, guest: Uuid) -> usize {
        guest_documents(&mut self.tx().await, guest).await.unwrap().len()
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn sweep_keeps_documents_of_guest_with_future_stay(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    // However old the document, a stay departing within the last 365 days (or later) keeps it, whichever way the
    // guest is on the stay.
    let (booker, _) = hotel.old_document(1, 900).await;
    hotel.stay(booker, "booker", 30, 32, "confirmed").await;
    let (primary, _) = hotel.old_document(2, 900).await;
    hotel.stay(primary, "primary", -370, -364, "checked_out").await;
    let (occupant, _) = hotel.old_document(3, 900).await;
    hotel.stay(occupant, "occupant", 200, 201, "tentative").await;
    // Departed 366 days ago, or only cancelled: deleted.
    let (departed, _) = hotel.old_document(4, 900).await;
    hotel.stay(departed, "primary", -370, -366, "checked_out").await;
    let (cancelled, _) = hotel.old_document(5, 900).await;
    hotel.stay(cancelled, "booker", 10, 12, "cancelled").await;
    // A young document stays whatever the stays.
    let (young, _) = hotel.old_document(6, 300).await;

    let report = sweep(&hotel.pool).await.unwrap();

    assert_eq!(report.expired_documents, 2);
    for (guest, kept) in [(booker, 1), (primary, 1), (occupant, 1), (departed, 0), (cancelled, 0), (young, 1)] {
        assert_eq!(hotel.documents_of(guest).await, kept, "guest {guest}");
    }
    assert_eq!(hotel.audited("guest_document.deleted").await, 2);
    let by: Vec<String> =
        sqlx::query_scalar("select data->>'by' from audit_log where action = 'guest_document.deleted'")
            .fetch_all(&mut *hotel.tx().await)
            .await
            .unwrap();
    assert_eq!(by, ["retention", "retention"]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn sweep_deletes_expired_documents_and_orphans(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let (guest, document_asset) = hotel.old_document(1, 400).await;

    // An unlinked gallery photo two days old goes; one unlinked an hour ago, and a linked one, stay.
    let unused = asset_of(&hotel.upload(Target::RoomType(hotel.room_type), 2).await);
    hotel.ready(unused).await;
    hotel.age(unused, 2).await;
    let recent = asset_of(&hotel.upload(Target::RoomType(hotel.room_type), 3).await);
    let linked = asset_of(&hotel.upload(Target::RoomType(hotel.room_type), 4).await);
    hotel.age(linked, 2).await;
    let mut tx = hotel.tx().await;
    for asset in [unused, recent] {
        media::unlink_gallery(&mut tx, hotel.tenant, hotel.user, hotel.property, hotel.room_type, asset).await.unwrap();
    }
    tx.commit().await.unwrap();

    // An upload abandoned for a day goes, with its multipart upload to abort.
    let stale = hotel.start(Target::RoomType(hotel.room_type), 1000).await.unwrap().asset_id;
    let mut tx = hotel.tx().await;
    media::set_s3_upload_id(&mut tx, hotel.property, stale, "s3-stale").await.unwrap();
    tx.commit().await.unwrap();
    hotel.exec("update media_asset set created_at = now() - interval '25 hours' where id = $1", stale).await;
    let fresh = hotel.start(Target::RoomType(hotel.room_type), 1000).await.unwrap().asset_id;

    // Sent outbox rows go after 7 days, receiver ids after 30.
    sqlx::query("update outbox set sent_at = now() - interval '8 days'").execute(&hotel.owner).await.unwrap();
    sqlx::query(
        "insert into processed_event (message_id, processed_at) values ('old', now() - interval '31 days'), ('new', now())",
    )
    .execute(&hotel.owner)
    .await
    .unwrap();

    let mut report = sweep(&hotel.pool).await.unwrap();
    report.orphans.sort_by_key(|(_, id, _, _)| *id);

    assert_eq!(report.expired_documents, 1);
    assert_eq!(hotel.documents_of(guest).await, 0);
    let prefix = |asset: Uuid| format!("t/{}/a/{asset}", hotel.tenant.0);
    let keys = |asset: Uuid| {
        ["upload", "archive.jxl", "thumb.avif", "thumb.webp", "md.avif", "md.webp"]
            .map(|name| format!("{}/{name}", prefix(asset)))
            .to_vec()
    };
    assert_eq!(
        report.orphans,
        [
            (hotel.tenant, document_asset, Purpose::GuestDocument, keys(document_asset)),
            (hotel.tenant, unused, Purpose::Gallery, keys(unused))
        ]
    );
    assert_eq!(
        report.stale_uploads,
        [StaleUpload {
            tenant: hotel.tenant,
            asset: stale,
            s3_upload_id: Some("s3-stale".to_owned()),
            upload_key: format!("{}/upload", prefix(stale)),
        }]
    );
    assert_eq!(report.failures, 0);
    for (asset, exists) in
        [(document_asset, false), (unused, false), (stale, false), (recent, true), (linked, true), (fresh, true)]
    {
        assert_eq!(hotel.status(asset).await.is_some(), exists, "asset {asset}");
    }
    assert_eq!(report.purged_outbox, 4);
    assert_eq!(report.purged_processed_events, 1);
    assert!(hotel.outbox("media-process").await.is_empty());

    let again = sweep(&hotel.pool).await.unwrap();
    assert_eq!(again, media::SweepReport::default(), "a second sweep finds nothing");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn sweep_fails_stuck_processing(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let stuck = asset_of(&hotel.upload(Target::RoomType(hotel.room_type), 1).await);
    hotel.exec("update media_asset set processing_at = now() - interval '61 minutes' where id = $1", stuck).await;
    let busy = asset_of(&hotel.upload(Target::RoomType(hotel.room_type), 2).await);

    let report = sweep(&hotel.pool).await.unwrap();

    assert_eq!(report.stuck, 1);
    let rows: Vec<(Uuid, String, Option<String>)> =
        sqlx::query_as("select id, status, error from media_asset order by id")
            .fetch_all(&mut *hotel.tx().await)
            .await
            .unwrap();
    assert_eq!(
        rows,
        [(stuck, "failed".to_owned(), Some("Processing timed out".to_owned())), (busy, "processing".to_owned(), None)]
    );
}

/// A duplicate upload that links an orphan while the sweep is deleting it wins: the sweep re-checks the links after
/// taking the row lock, so neither the asset nor the new link is deleted.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn sweep_never_deletes_an_asset_a_duplicate_just_linked(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = std::sync::Arc::new(Hotel::new(opts).await);
    let orphan = asset_of(&hotel.upload(Target::RoomType(hotel.room_type), 1).await);
    hotel.ready(orphan).await;
    let mut tx = hotel.tx().await;
    media::unlink_gallery(&mut tx, hotel.tenant, hotel.user, hotel.property, hotel.room_type, orphan).await.unwrap();
    tx.commit().await.unwrap();
    hotel.exec("update media_asset set created_at = now() - interval '2 days' where id = $1", orphan).await;

    // The duplicate complete links the orphan and holds its row lock, uncommitted.
    let upload = hotel.start(Target::RoomType(hotel.room_type), 1000).await.unwrap().asset_id;
    let mut complete = begin(&hotel.pool, Scope::tenant(hotel.tenant)).await.unwrap();
    let completed =
        complete_upload(&mut complete, hotel.tenant, hotel.user, hotel.property, upload, verified(1)).await.unwrap();
    assert!(matches!(completed, Completed::Duplicate { ref asset, .. } if asset.id == orphan));

    let sweeping = {
        let hotel = hotel.clone();
        tokio::spawn(async move { sweep(&hotel.pool).await })
    };
    let mut waited = 0;
    loop {
        let blocked: i64 = sqlx::query_scalar(
            "select count(*) from pg_locks l join pg_stat_activity s on s.pid = l.pid
             where not l.granted and s.datname = current_database()",
        )
        .fetch_one(&hotel.owner)
        .await
        .unwrap();
        if blocked > 0 {
            break;
        }
        waited += 1;
        assert!(waited < 500, "the sweep never waited on the complete");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    complete.commit().await.unwrap();

    let report = sweeping.await.unwrap().unwrap();
    assert!(report.orphans.is_empty(), "{:?}", report.orphans);
    assert_eq!(report.failures, 0);
    assert_eq!(hotel.status(orphan).await.as_deref(), Some("ready"));
    let items = gallery(&mut hotel.tx().await, hotel.room_type).await.unwrap();
    assert_eq!(items.iter().map(|i| i.asset_id).collect::<Vec<_>>(), [orphan]);
}

/// A kept document waits for its next possible expiry, so kept documents never crowd expired ones out of the sweep's
/// candidate limit.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn kept_documents_wait_for_their_next_possible_expiry(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let (future, _) = hotel.old_document(1, 900).await;
    hotel.stay(future, "booker", 30, 32, "confirmed").await;
    let (recent, _) = hotel.old_document(2, 900).await;
    hotel.stay(recent, "occupant", -103, -100, "checked_out").await;
    let (expired, _) = hotel.old_document(3, 900).await;

    let report = sweep(&hotel.pool).await.unwrap();

    assert_eq!(report.expired_documents, 1);
    assert_eq!(hotel.documents_of(expired).await, 0);
    let recheck = async |guest: Uuid| -> String {
        sqlx::query_scalar("select (recheck_after at time zone 'UTC')::text from guest_document where guest_id = $1")
            .bind(guest)
            .fetch_one(&mut *hotel.tx().await)
            .await
            .unwrap()
    };
    let midnight = |days: i64| format!("{} 00:00:00", hotel.business_date + time::Duration::days(days));
    assert_eq!(recheck(future).await, midnight(32 + 365), "the future stay's departure plus 365 days");
    assert_eq!(recheck(recent).await, midnight(-100 + 365));

    let due: Vec<(Uuid, String, Uuid)> =
        sqlx::query_as("select tenant_id, kind, id from app.media_sweep_due()").fetch_all(&hotel.pool).await.unwrap();
    assert!(due.iter().all(|(_, kind, _)| kind != "expired_document"), "kept documents are not listed: {due:?}");
    assert_eq!(sweep(&hotel.pool).await.unwrap(), media::SweepReport::default());
}
