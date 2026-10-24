mod common;

use common::Hotel;
use laundry::{
    Batch, BatchStatus, LaundryError, LinenItem, LinenLocation, LocationKind, MovementKind, NewMovement, Settle,
    create_item, create_location, list_batches, receive, record, send, settle, stock,
};
use serde_json::json;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

impl Hotel {
    async fn item(&self, code: &str) -> LinenItem {
        let mut tx = self.tx().await;
        let item = create_item(&mut tx, self.tenant, self.user, self.property, code, "Linen").await.unwrap();
        tx.commit().await.unwrap();
        item
    }

    async fn location(&self, name: &str, kind: LocationKind) -> LinenLocation {
        let mut tx = self.tx().await;
        let location = create_location(&mut tx, self.tenant, self.user, self.property, name, kind, None).await.unwrap();
        tx.commit().await.unwrap();
        location
    }

    async fn stocked(&self, item: Uuid, quantity: i32, to: Uuid) {
        let movement = NewMovement {
            item_id: item,
            kind: MovementKind::Receive,
            quantity,
            from_location_id: None,
            to_location_id: Some(to),
            reason: None,
        };
        let mut tx = self.tx().await;
        record(&mut tx, self.tenant, self.user, self.property, movement).await.unwrap();
        tx.commit().await.unwrap();
    }

    async fn held(&self, location: Uuid, item: Uuid) -> i32 {
        stock(&mut self.tx().await, self.property)
            .await
            .unwrap()
            .into_iter()
            .find(|cell| cell.location_id == location && cell.item_id == item)
            .unwrap()
            .quantity
    }

    async fn send(&self, from: Uuid, to: Uuid, counts: &[(Uuid, i32)]) -> Result<Batch, LaundryError> {
        let mut tx = self.tx().await;
        let batch =
            send(&mut tx, self.tenant, self.user, self.property, from, to, counts, Some("Week 1".into())).await?;
        tx.commit().await.unwrap();
        Ok(batch)
    }

    async fn receive(
        &self,
        batch: &Batch,
        back_to: Option<Uuid>,
        counts: &[(Uuid, i32)],
    ) -> Result<Batch, LaundryError> {
        let mut tx = self.tx().await;
        let batch =
            receive(&mut tx, self.tenant, self.user, self.property, batch.id, batch.version, back_to, counts).await?;
        tx.commit().await.unwrap();
        Ok(batch)
    }

    async fn settle(&self, batch: &Batch, how: Settle) -> Result<Batch, LaundryError> {
        let mut tx = self.tx().await;
        let batch = settle(&mut tx, self.tenant, self.user, self.property, batch.id, batch.version, how).await?;
        tx.commit().await.unwrap();
        Ok(batch)
    }
}

struct Setup {
    hotel: Hotel,
    sheet: Uuid,
    towel: Uuid,
    store: Uuid,
    laundry: Uuid,
}

async fn setup(opts: PgConnectOptions) -> Setup {
    let hotel = Hotel::new(opts).await;
    let sheet = hotel.item("SHEET_K").await.id;
    let towel = hotel.item("TOWEL_B").await.id;
    let store = hotel.location("Store", LocationKind::Store).await.id;
    let laundry = hotel.location("Laundry", LocationKind::Laundry).await.id;
    hotel.stocked(sheet, 50, store).await;
    hotel.stocked(towel, 60, store).await;
    Setup { hotel, sheet, towel, store, laundry }
}

fn line(batch: &Batch, item: Uuid) -> (i32, i32, i32) {
    let line = batch.lines.iter().find(|line| line.item_id == item).unwrap();
    (line.sent, line.returned, line.written_off)
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn send_moves_stock_to_the_laundry(_: PgPoolOptions, opts: PgConnectOptions) {
    let s = setup(opts).await;
    let batch = s.hotel.send(s.store, s.laundry, &[(s.sheet, 20), (s.towel, 30)]).await.unwrap();
    assert_eq!((batch.status, batch.version, batch.outstanding()), (BatchStatus::Sent, 1, 50));
    assert_eq!(batch.note.as_deref(), Some("Week 1"));
    assert_eq!(line(&batch, s.sheet), (20, 0, 0));
    assert_eq!(s.hotel.held(s.store, s.sheet).await, 30);
    assert_eq!(s.hotel.held(s.laundry, s.sheet).await, 20);
    assert_eq!(s.hotel.held(s.store, s.towel).await, 30);
    assert_eq!(s.hotel.held(s.laundry, s.towel).await, 30);
    assert_eq!(s.hotel.audited("laundry_batch.sent").await, 1);

    let listed = list_batches(&mut s.hotel.tx().await, s.hotel.property, Some(BatchStatus::Sent)).await.unwrap();
    assert_eq!(listed, vec![batch]);
    assert!(
        list_batches(&mut s.hotel.tx().await, s.hotel.property, Some(BatchStatus::Received)).await.unwrap().is_empty()
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn receive_records_the_difference_and_it_is_settled_later(_: PgPoolOptions, opts: PgConnectOptions) {
    let s = setup(opts).await;
    let batch = s.hotel.send(s.store, s.laundry, &[(s.sheet, 20), (s.towel, 30)]).await.unwrap();

    let received = s.hotel.receive(&batch, None, &[(s.sheet, 18), (s.towel, 30)]).await.unwrap();
    assert_eq!((received.status, received.version, received.outstanding()), (BatchStatus::Received, 2, 2));
    assert!(received.received_at.is_some() && !received.settled());
    assert_eq!(line(&received, s.sheet), (20, 18, 0));
    assert_eq!(s.hotel.held(s.laundry, s.sheet).await, 2);
    assert_eq!(s.hotel.held(s.store, s.sheet).await, 48);
    assert_eq!(s.hotel.held(s.laundry, s.towel).await, 0);
    assert_eq!(s.hotel.audited("laundry_batch.received").await, 1);
    let data = &s.hotel.audit_data("laundry_batch.received").await[0];
    assert_eq!(data["back_to_location_id"], json!(s.store));
    let audited_line =
        |item: Uuid| data["items"].as_array().unwrap().iter().find(|i| i["item_id"] == json!(item)).cloned();
    assert_eq!(audited_line(s.sheet), Some(json!({ "item_id": s.sheet, "sent": 20, "received": 18, "difference": 2 })));
    assert_eq!(audited_line(s.towel), Some(json!({ "item_id": s.towel, "sent": 30, "received": 30, "difference": 0 })));

    let found = s.hotel.settle(&received, Settle::Return { back_to: None, counts: vec![(s.sheet, 1)] }).await.unwrap();
    assert_eq!((found.version, found.outstanding()), (3, 1));
    let reason = || "Lost".to_owned();
    let none = s.hotel.settle(&found, Settle::WriteOff { reason: " ".into(), counts: vec![(s.sheet, 1)] }).await;
    assert!(matches!(none, Err(LaundryError::Invalid(_))));
    let settled =
        s.hotel.settle(&found, Settle::WriteOff { reason: reason(), counts: vec![(s.sheet, 1)] }).await.unwrap();
    assert!(settled.settled());
    assert_eq!(line(&settled, s.sheet), (20, 19, 1));
    assert_eq!(s.hotel.held(s.laundry, s.sheet).await, 0);
    assert_eq!(s.hotel.audited("laundry_batch.settled").await, 2);
    let data = s.hotel.audit_data("laundry_batch.settled").await;
    assert_eq!(
        data[0],
        json!({ "how": "return", "back_to_location_id": s.store, "items": [{ "item_id": s.sheet, "quantity": 1 }] })
    );
    assert_eq!(
        data[1],
        json!({ "how": "write_off", "reason": "Lost", "items": [{ "item_id": s.sheet, "quantity": 1 }] })
    );

    let again = s.hotel.settle(&settled, Settle::WriteOff { reason: reason(), counts: vec![(s.sheet, 1)] }).await;
    assert!(matches!(again, Err(LaundryError::Conflict(_))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_all_zero_receive_still_marks_received(_: PgPoolOptions, opts: PgConnectOptions) {
    let s = setup(opts).await;
    let batch = s.hotel.send(s.store, s.laundry, &[(s.sheet, 5)]).await.unwrap();
    let received = s.hotel.receive(&batch, None, &[]).await.unwrap();
    assert_eq!((received.status, received.outstanding()), (BatchStatus::Received, 5));
    assert_eq!(s.hotel.held(s.laundry, s.sheet).await, 5);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn receive_refuses_bad_counts_twice_and_stale_versions(_: PgPoolOptions, opts: PgConnectOptions) {
    let s = setup(opts).await;
    let batch = s.hotel.send(s.store, s.laundry, &[(s.sheet, 20)]).await.unwrap();

    match s.hotel.receive(&batch, None, &[(s.sheet, 21)]).await {
        Err(LaundryError::Invalid(message)) => assert_eq!(message, "Only 20 SHEET_K are still out"),
        other => panic!("expected Invalid, got {other:?}"),
    }
    assert!(matches!(s.hotel.receive(&batch, None, &[(s.towel, 1)]).await, Err(LaundryError::Invalid(_))));
    assert!(matches!(s.hotel.receive(&batch, None, &[(s.sheet, -1)]).await, Err(LaundryError::Invalid(_))));
    assert!(matches!(s.hotel.receive(&batch, Some(s.laundry), &[(s.sheet, 1)]).await, Err(LaundryError::Invalid(_))));

    let stale = Batch { version: 9, ..batch.clone() };
    assert!(matches!(
        s.hotel.receive(&stale, None, &[(s.sheet, 1)]).await,
        Err(LaundryError::VersionMismatch("batch"))
    ));
    assert!(matches!(
        s.hotel.settle(&batch, Settle::Return { back_to: None, counts: vec![(s.sheet, 1)] }).await,
        Err(LaundryError::Conflict(_))
    ));

    let received = s.hotel.receive(&batch, None, &[(s.sheet, 20)]).await.unwrap();
    assert!(received.settled());
    assert!(matches!(s.hotel.receive(&received, None, &[]).await, Err(LaundryError::Conflict(_))));
    assert_eq!(s.hotel.held(s.laundry, s.sheet).await, 0);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn sending_more_than_the_store_holds_changes_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let s = setup(opts).await;
    let result = s.hotel.send(s.store, s.laundry, &[(s.towel, 10), (s.sheet, 51)]).await;
    assert!(matches!(result, Err(LaundryError::Conflict(_))));
    assert_eq!(s.hotel.held(s.store, s.towel).await, 60);
    assert_eq!(s.hotel.held(s.laundry, s.towel).await, 0);
    assert!(list_batches(&mut s.hotel.tx().await, s.hotel.property, None).await.unwrap().is_empty());
    assert_eq!(s.hotel.audited("laundry_batch.sent").await, 0);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn send_checks_its_input(_: PgPoolOptions, opts: PgConnectOptions) {
    let s = setup(opts).await;
    for counts in [vec![], vec![(s.sheet, 1), (s.sheet, 2)], vec![(s.sheet, 0)]] {
        assert!(matches!(s.hotel.send(s.store, s.laundry, &counts).await, Err(LaundryError::Invalid(_))));
    }
    assert!(matches!(s.hotel.send(s.laundry, s.store, &[(s.sheet, 1)]).await, Err(LaundryError::Invalid(_))));
    assert!(matches!(s.hotel.send(s.store, s.store, &[(s.sheet, 1)]).await, Err(LaundryError::Invalid(_))));
    assert!(matches!(
        s.hotel.send(s.store, Uuid::now_v7(), &[(s.sheet, 1)]).await,
        Err(LaundryError::NotFound("location"))
    ));
}
