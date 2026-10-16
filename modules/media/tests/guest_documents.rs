mod common;

use common::{Hotel, asset_of, guest_target, ready};
use media::{
    LinkSize, MediaError, Verified, Visibility, complete_upload, delete_guest_document, guest_document_link_key,
    guest_documents,
};
use media_types::MediaResult;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

impl Hotel {
    async fn link(&self, document: Uuid, size: LinkSize) -> Result<(Visibility, String), MediaError> {
        let mut tx = self.tx().await;
        let link = guest_document_link_key(&mut tx, self.tenant, self.user, self.property, document, size).await?;
        tx.commit().await.unwrap();
        Ok(link)
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn guest_document_link_audits_each_call(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let guest = hotel.guest().await;
    let asset = asset_of(&hotel.upload(guest_target(guest), 1).await);
    let document = guest_documents(&mut hotel.tx().await, hotel.property, guest).await.unwrap()[0].id;

    let err = hotel.link(document, LinkSize::Md).await.unwrap_err();
    assert!(matches!(err, MediaError::Conflict(_)), "not ready yet: {err:?}");
    assert_eq!(hotel.audited("guest_document.viewed").await, 0, "no link, no audit entry");

    hotel.ready(asset).await;
    let prefix = format!("t/{}/a/{asset}", hotel.tenant.0);
    assert_eq!(
        hotel.link(document, LinkSize::Thumb).await.unwrap(),
        (Visibility::Private, format!("{prefix}/thumb.webp"))
    );
    assert_eq!(hotel.link(document, LinkSize::Thumb).await.unwrap().1, format!("{prefix}/thumb.webp"));
    let err = hotel.link(document, LinkSize::Original).await.unwrap_err();
    assert!(matches!(&err, MediaError::Invalid(m) if m == "Images are viewed at the md size"), "{err:?}");
    assert_eq!(hotel.audited("guest_document.viewed").await, 2, "one entry per link, none for a refusal");
    let sizes: Vec<String> = sqlx::query_scalar(
        "select data->>'size' from audit_log where action = 'guest_document.viewed' and entity_id = $1 order by id",
    )
    .bind(document)
    .fetch_all(&mut *hotel.tx().await)
    .await
    .unwrap();
    assert_eq!(sizes, ["thumb", "thumb"]);

    assert!(matches!(hotel.link(Uuid::now_v7(), LinkSize::Md).await, Err(MediaError::NotFound("document"))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_image_kept_as_uploaded_jpeg_is_still_viewed_through_its_renditions(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let hotel = Hotel::new(opts).await;
    let guest = hotel.guest().await;
    let asset = asset_of(&hotel.upload(guest_target(guest), 1).await);
    let document = guest_documents(&mut hotel.tx().await, hotel.property, guest).await.unwrap()[0].id;
    let mut result = ready(hotel.tenant, asset);
    let MediaResult::Ready { archive_key, .. } = &mut result else { unreachable!() };
    *archive_key = format!("t/{}/a/{asset}/archive.jpg", hotel.tenant.0);
    hotel.apply(&result).await;

    let err = hotel.link(document, LinkSize::Original).await.unwrap_err();

    assert!(matches!(&err, MediaError::Invalid(m) if m == "Images are viewed at the md size"), "{err:?}");
    assert_eq!(hotel.audited("guest_document.viewed").await, 0);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn deleting_a_document_is_audited(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let guest = hotel.guest().await;
    hotel.upload(guest_target(guest), 1).await;
    let document = guest_documents(&mut hotel.tx().await, hotel.property, guest).await.unwrap()[0].id;

    let mut tx = hotel.tx().await;
    delete_guest_document(&mut tx, hotel.tenant, hotel.user, hotel.property, document).await.unwrap();
    let err = delete_guest_document(&mut tx, hotel.tenant, hotel.user, hotel.property, document).await.unwrap_err();
    assert!(matches!(err, MediaError::NotFound("document")), "{err:?}");
    tx.commit().await.unwrap();

    assert!(guest_documents(&mut hotel.tx().await, hotel.property, guest).await.unwrap().is_empty());
    let by: String = sqlx::query_scalar("select data->>'by' from audit_log where action = 'guest_document.deleted'")
        .fetch_one(&mut *hotel.tx().await)
        .await
        .unwrap();
    assert_eq!(by, "user");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_pdf_is_linked_as_uploaded_and_has_no_preview(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let guest = hotel.guest().await;
    let upload = hotel.start(guest_target(guest), 1000).await.unwrap().asset_id;
    let pdf = Verified { mime: "application/pdf".into(), width: None, height: None, ..common::verified(1) };
    let mut tx = hotel.tx().await;
    complete_upload(&mut tx, hotel.tenant, hotel.user, hotel.property, upload, pdf).await.unwrap();
    tx.commit().await.unwrap();
    let archive_key = format!("t/{}/a/{upload}/archive.pdf", hotel.tenant.0);
    hotel
        .apply(&MediaResult::Ready {
            tenant_id: hotel.tenant.0,
            asset_id: upload,
            width: 0,
            height: 0,
            archive_key: archive_key.clone(),
            renditions: vec![],
        })
        .await;
    let document = guest_documents(&mut hotel.tx().await, hotel.property, guest).await.unwrap()[0].id;

    assert_eq!(hotel.link(document, LinkSize::Original).await.unwrap(), (Visibility::Private, archive_key));
    assert!(matches!(hotel.link(document, LinkSize::Thumb).await, Err(MediaError::Invalid(_))));
    assert_eq!(hotel.audited("guest_document.viewed").await, 1);
}
