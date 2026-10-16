mod common;

use common::{Hotel, asset_of};
use db::UserId;
use media::{
    MediaError, Target, gallery, issue_photos, reorder_gallery, set_caption, unlink_gallery, unlink_issue_photo,
};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

impl Hotel {
    async fn reorder(&self, room_type: Uuid, ids: &[Uuid]) -> Result<(), MediaError> {
        let mut tx = self.tx().await;
        reorder_gallery(&mut tx, self.tenant, self.user, self.property, room_type, ids).await?;
        tx.commit().await.unwrap();
        Ok(())
    }

    async fn order(&self) -> Vec<Uuid> {
        gallery(&mut self.tx().await, self.room_type).await.unwrap().into_iter().map(|i| i.asset_id).collect()
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn reorder_rejects_foreign_or_missing_ids(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut photos = Vec::new();
    for file in 1..=3 {
        photos.push(asset_of(&hotel.upload(Target::RoomType(hotel.room_type), file).await));
    }
    assert_eq!(hotel.order().await, photos, "photos are appended in upload order");
    let suite = hotel.add_room_type(hotel.property, "STE").await;
    let elsewhere = asset_of(&hotel.upload(Target::RoomType(suite), 4).await);

    let (a, b, c) = (photos[0], photos[1], photos[2]);
    for wrong in [vec![a, b], vec![a, b, c, elsewhere], vec![a, b, elsewhere], vec![a, a, b, c], vec![]] {
        let err = hotel.reorder(hotel.room_type, &wrong).await.unwrap_err();
        assert!(matches!(err, MediaError::Invalid(_)), "{wrong:?}: {err:?}");
    }
    assert_eq!(hotel.order().await, photos, "a refused order changed nothing");

    hotel.reorder(hotel.room_type, &[c, a, b]).await.unwrap();
    assert_eq!(hotel.order().await, [c, a, b]);
    let positions: Vec<i32> =
        gallery(&mut hotel.tx().await, hotel.room_type).await.unwrap().into_iter().map(|i| i.position).collect();
    assert_eq!(positions, [0, 1, 2]);

    let other_property = hotel.add_property("OTH").await;
    let mut tx = hotel.tx().await;
    let err = reorder_gallery(&mut tx, hotel.tenant, hotel.user, other_property, hotel.room_type, &[c, a, b])
        .await
        .unwrap_err();
    assert!(matches!(err, MediaError::NotFound("room type")), "{err:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn captions_and_unlinking(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let photo = asset_of(&hotel.upload(Target::RoomType(hotel.room_type), 1).await);

    let mut tx = hotel.tx().await;
    set_caption(&mut tx, hotel.tenant, hotel.user, hotel.property, hotel.room_type, photo, "  Sea view  ")
        .await
        .unwrap();
    let err = set_caption(&mut tx, hotel.tenant, hotel.user, hotel.property, hotel.room_type, photo, &"x".repeat(201))
        .await
        .unwrap_err();
    assert!(matches!(err, MediaError::Invalid(_)), "{err:?}");
    tx.commit().await.unwrap();
    assert_eq!(gallery(&mut hotel.tx().await, hotel.room_type).await.unwrap()[0].caption, "Sea view");

    let mut tx = hotel.tx().await;
    unlink_gallery(&mut tx, hotel.tenant, hotel.user, hotel.property, hotel.room_type, photo).await.unwrap();
    let err =
        unlink_gallery(&mut tx, hotel.tenant, hotel.user, hotel.property, hotel.room_type, photo).await.unwrap_err();
    assert!(matches!(err, MediaError::NotFound("photo")), "{err:?}");
    tx.commit().await.unwrap();
    assert!(hotel.order().await.is_empty());
    assert_eq!(hotel.assets().await, 1, "the asset stays until the sweep finds it unused");

    let issue = hotel.issue().await;
    let issue_photo = asset_of(&hotel.upload(Target::RoomIssue(issue), 2).await);
    let mut tx = hotel.tx().await;
    let someone_else = UserId(Uuid::now_v7());
    let err = unlink_issue_photo(&mut tx, hotel.tenant, someone_else, false, hotel.property, issue, issue_photo)
        .await
        .unwrap_err();
    assert!(matches!(err, MediaError::Forbidden(_)), "only the uploader or a supervisor: {err:?}");
    unlink_issue_photo(&mut tx, hotel.tenant, hotel.user, false, hotel.property, issue, issue_photo).await.unwrap();
    tx.commit().await.unwrap();
    assert!(issue_photos(&mut hotel.tx().await, &[issue]).await.unwrap().is_empty());
    assert_eq!(hotel.audited("media.removed").await, 2);
}
