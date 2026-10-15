mod common;

use common::{Hotel, asset_of, guest_target};
use media::{LinkSize, MediaError, Visibility, delete_guest_document, guest_document_link_key, guest_documents};
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
    let document = guest_documents(&mut hotel.tx().await, guest).await.unwrap()[0].id;

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
    assert_eq!(hotel.link(document, LinkSize::Original).await.unwrap().1, format!("{prefix}/archive.jxl"));
    assert_eq!(hotel.audited("guest_document.viewed").await, 3, "one entry per link");
    let sizes: Vec<String> = sqlx::query_scalar(
        "select data->>'size' from audit_log where action = 'guest_document.viewed' and entity_id = $1 order by id",
    )
    .bind(document)
    .fetch_all(&mut *hotel.tx().await)
    .await
    .unwrap();
    assert_eq!(sizes, ["thumb", "thumb", "original"]);

    assert!(matches!(hotel.link(Uuid::now_v7(), LinkSize::Md).await, Err(MediaError::NotFound("document"))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn deleting_a_document_is_audited(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let guest = hotel.guest().await;
    hotel.upload(guest_target(guest), 1).await;
    let document = guest_documents(&mut hotel.tx().await, guest).await.unwrap()[0].id;

    let mut tx = hotel.tx().await;
    delete_guest_document(&mut tx, hotel.tenant, hotel.user, hotel.property, document).await.unwrap();
    let err = delete_guest_document(&mut tx, hotel.tenant, hotel.user, hotel.property, document).await.unwrap_err();
    assert!(matches!(err, MediaError::NotFound("document")), "{err:?}");
    tx.commit().await.unwrap();

    assert!(guest_documents(&mut hotel.tx().await, guest).await.unwrap().is_empty());
    let by: String = sqlx::query_scalar("select data->>'by' from audit_log where action = 'guest_document.deleted'")
        .fetch_one(&mut *hotel.tx().await)
        .await
        .unwrap();
    assert_eq!(by, "user");
}
