mod common;

use audit::{AlertKind, alerts};
use common::Hotel;
use laundry::{
    LaundryError, LinenItem, LinenLocation, LocationKind, Movement, MovementKind, NewMovement, StockCell, create_item,
    create_location, movements, record, set_par, stock,
};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use sqlx::postgres::{PgConnectOptions, PgListener, PgPoolOptions};
use std::time::Duration;
use uuid::Uuid;

impl Hotel {
    async fn item(&self, code: &str) -> LinenItem {
        let mut tx = self.tx().await;
        let item = create_item(&mut tx, self.tenant, self.user, self.property, code, "Linen").await.unwrap();
        tx.commit().await.unwrap();
        item
    }

    async fn location(&self, name: &str, kind: LocationKind, section: Option<Uuid>) -> LinenLocation {
        let mut tx = self.tx().await;
        let location =
            create_location(&mut tx, self.tenant, self.user, self.property, name, kind, section).await.unwrap();
        tx.commit().await.unwrap();
        location
    }

    async fn room(&self, number: &str, section: Option<Uuid>) -> Uuid {
        let mut tx = self.tx().await;
        let room =
            rooms::NewRoom { room_type_id: self.room_type, number: number.into(), floor: None, section_id: section };
        let room = rooms::create_room(&mut tx, self.tenant, self.user, self.property, room).await.unwrap();
        tx.commit().await.unwrap();
        room.id
    }

    async fn record(&self, movement: NewMovement) -> Result<Movement, LaundryError> {
        let mut tx = self.tx().await;
        let movement = record(&mut tx, self.tenant, self.user, self.property, movement).await?;
        tx.commit().await.unwrap();
        Ok(movement)
    }

    async fn stock(&self) -> Vec<StockCell> {
        stock(&mut self.tx().await, self.property).await.unwrap()
    }

    async fn held(&self, location: Uuid, item: Uuid) -> i32 {
        self.stock()
            .await
            .into_iter()
            .find(|cell| cell.location_id == location && cell.item_id == item)
            .unwrap()
            .quantity
    }

    async fn alert(&self) -> Option<String> {
        let alerts = alerts::list(&mut self.tx().await, self.property).await.unwrap();
        alerts.into_iter().find(|alert| alert.kind == AlertKind::Linen).map(|alert| alert.message)
    }

    async fn movement_count(&self) -> i64 {
        sqlx::query_scalar("select count(*) from linen_movement").fetch_one(&mut *self.tx().await).await.unwrap()
    }
}

fn receive(item: Uuid, quantity: i32, to: Uuid) -> NewMovement {
    NewMovement {
        item_id: item,
        kind: MovementKind::Receive,
        quantity,
        from_location_id: None,
        to_location_id: Some(to),
        reason: None,
    }
}

fn transfer(item: Uuid, quantity: i32, from: Uuid, to: Uuid) -> NewMovement {
    NewMovement {
        item_id: item,
        kind: MovementKind::Transfer,
        quantity,
        from_location_id: Some(from),
        to_location_id: Some(to),
        reason: None,
    }
}

fn write_off(item: Uuid, quantity: i32, from: Uuid, reason: Option<&str>) -> NewMovement {
    NewMovement {
        item_id: item,
        kind: MovementKind::WriteOff,
        quantity,
        from_location_id: Some(from),
        to_location_id: None,
        reason: reason.map(Into::into),
    }
}

fn invalid(result: Result<Movement, LaundryError>) -> String {
    match result {
        Err(LaundryError::Invalid(message)) => message,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn receive_transfer_and_write_off_update_stock(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let sheet = hotel.item("SHEET_K").await.id;
    let store = hotel.location("Store", LocationKind::Store, None).await.id;
    let section = hotel.section("East").await;
    let pantry = hotel.location("Pantry East", LocationKind::Pantry, Some(section)).await.id;

    let received = hotel.record(receive(sheet, 40, store)).await.unwrap();
    assert_eq!((received.kind, received.quantity, received.to_location_id), (MovementKind::Receive, 40, Some(store)));
    assert_eq!(received.moved_by, Some(hotel.user.0));
    hotel.record(transfer(sheet, 15, store, pantry)).await.unwrap();
    let written_off = hotel.record(write_off(sheet, 2, pantry, Some("  Torn  "))).await.unwrap();
    assert_eq!(written_off.reason.as_deref(), Some("Torn"));

    assert_eq!(hotel.held(store, sheet).await, 25);
    assert_eq!(hotel.held(pantry, sheet).await, 13);
    assert_eq!(hotel.audited("linen_movement.recorded").await, 3);

    let mut tx = hotel.tx().await;
    let newest = movements(&mut tx, hotel.property, 2, None, None).await.unwrap();
    assert_eq!(newest.iter().map(|m| m.kind).collect::<Vec<_>>(), [MovementKind::WriteOff, MovementKind::Transfer]);
    let older = movements(&mut tx, hotel.property, 2, Some(newest[1].id), None).await.unwrap();
    assert_eq!(older.iter().map(|m| m.id).collect::<Vec<_>>(), [received.id]);
    let at_store = movements(&mut tx, hotel.property, 10, None, Some(store)).await.unwrap();
    assert_eq!(at_store.len(), 2);
    assert!(matches!(movements(&mut tx, hotel.property, 0, None, None).await, Err(LaundryError::Invalid(_))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_movement_below_zero_is_refused_and_changes_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let sheet = hotel.item("SHEET_K").await.id;
    let store = hotel.location("Store", LocationKind::Store, None).await.id;
    let section = hotel.section("East").await;
    let pantry = hotel.location("Pantry East", LocationKind::Pantry, Some(section)).await.id;
    hotel.record(receive(sheet, 5, store)).await.unwrap();

    let refused = hotel.record(transfer(sheet, 6, store, pantry)).await;
    match refused {
        Err(LaundryError::Conflict(message)) => assert_eq!(message, "Only 5 SHEET_K at Store"),
        other => panic!("expected Conflict, got {other:?}"),
    }
    let refused = hotel.record(write_off(sheet, 1, pantry, Some("Lost"))).await;
    assert_eq!(refused.unwrap_err().to_string(), "Only 0 SHEET_K at Pantry East");

    assert_eq!(hotel.held(store, sheet).await, 5);
    assert_eq!(hotel.held(pantry, sheet).await, 0);
    assert_eq!(hotel.movement_count().await, 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_kind_must_suit_its_locations(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let sheet = hotel.item("SHEET_K").await;
    let store = hotel.location("Store", LocationKind::Store, None).await.id;
    let laundry = hotel.location("Laundry", LocationKind::Laundry, None).await.id;
    let vendor = hotel.location("Vendor", LocationKind::Vendor, None).await.id;
    hotel.record(receive(sheet.id, 5, store)).await.unwrap();

    assert_eq!(invalid(hotel.record(receive(sheet.id, 5, laundry)).await), "a receive goes to a store or pantry");
    assert_eq!(
        invalid(hotel.record(transfer(sheet.id, 1, store, laundry)).await),
        "a transfer moves between stores and pantries"
    );
    assert!(hotel.record(transfer(sheet.id, 1, laundry, vendor)).await.is_err());
    let send = NewMovement { kind: MovementKind::Send, ..transfer(sheet.id, 1, store, laundry) };
    assert_eq!(invalid(hotel.record(send).await), "send and return go with a laundry batch");
    let ret = NewMovement { kind: MovementKind::Return, ..transfer(sheet.id, 1, laundry, store) };
    assert!(matches!(hotel.record(ret).await, Err(LaundryError::Invalid(_))));
    let both = NewMovement { from_location_id: Some(laundry), ..receive(sheet.id, 1, store) };
    assert!(matches!(hotel.record(both).await, Err(LaundryError::Invalid(_))));
    assert!(matches!(hotel.record(transfer(sheet.id, 1, store, store)).await, Err(LaundryError::Invalid(_))));
    assert!(matches!(hotel.record(receive(sheet.id, 0, store)).await, Err(LaundryError::Invalid(_))));

    // Locations and items of another property, or switched off, are refused too.
    let (other, _) = hotel.other_property().await;
    let mut tx = hotel.tx().await;
    let foreign =
        create_location(&mut tx, hotel.tenant, hotel.user, other, "Store", LocationKind::Store, None).await.unwrap().id;
    tx.commit().await.unwrap();
    assert!(matches!(hotel.record(receive(sheet.id, 1, foreign)).await, Err(LaundryError::NotFound("location"))));
    assert!(matches!(hotel.record(receive(Uuid::now_v7(), 1, store)).await, Err(LaundryError::NotFound("item"))));
    let towel = hotel.item("TOWEL").await;
    let mut tx = hotel.tx().await;
    laundry::update_item(&mut tx, hotel.tenant, hotel.user, hotel.property, towel.id, towel.version, None, Some(false))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(invalid(hotel.record(receive(towel.id, 1, store)).await), "TOWEL is switched off");
    assert_eq!(hotel.movement_count().await, 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_write_off_needs_a_reason(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let sheet = hotel.item("SHEET_K").await.id;
    let store = hotel.location("Store", LocationKind::Store, None).await.id;
    let laundry = hotel.location("Laundry", LocationKind::Laundry, None).await.id;
    hotel.record(receive(sheet, 5, store)).await.unwrap();

    assert_eq!(invalid(hotel.record(write_off(sheet, 1, store, None)).await), "a write-off needs a reason");
    assert_eq!(invalid(hotel.record(write_off(sheet, 1, store, Some("   "))).await), "a write-off needs a reason");
    // Linen at a laundry or vendor is written off against its batch, so every short batch stays settleable.
    let vendor = hotel.location("Vendor", LocationKind::Vendor, None).await.id;
    assert_eq!(
        invalid(hotel.record(write_off(sheet, 1, laundry, Some("Lost"))).await),
        "Write it off against its batch"
    );
    assert_eq!(
        invalid(hotel.record(write_off(sheet, 1, vendor, Some("Lost"))).await),
        "Write it off against its batch"
    );
    hotel.record(write_off(sheet, 1, store, Some("Stained"))).await.unwrap();
    assert_eq!(hotel.held(store, sheet).await, 4);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn stock_always_equals_the_sum_of_movements(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let items = [hotel.item("SHEET_K").await.id, hotel.item("TOWEL").await.id];
    let east = hotel.section("East").await;
    let west = hotel.section("West").await;
    let locations = [
        hotel.location("Store", LocationKind::Store, None).await.id,
        hotel.location("Pantry East", LocationKind::Pantry, Some(east)).await.id,
        hotel.location("Pantry West", LocationKind::Pantry, Some(west)).await.id,
    ];
    let mut rng = StdRng::seed_from_u64(0x11_4E_E7);
    let (mut accepted, mut refused) = (0, 0);
    for _ in 0..200 {
        let item = items[rng.random_range(0..items.len())];
        let from = locations[rng.random_range(0..locations.len())];
        let to = locations[rng.random_range(0..locations.len())];
        let quantity = rng.random_range(1..=30);
        let movement = match rng.random_range(0..10) {
            0..=2 => receive(item, quantity, to),
            3..=7 => transfer(item, quantity, from, to),
            8 => write_off(item, quantity, from, Some("Worn")),
            _ => write_off(item, quantity, from, None),
        };
        match hotel.record(movement).await {
            Ok(_) => accepted += 1,
            Err(LaundryError::Conflict(_) | LaundryError::Invalid(_)) => refused += 1,
            Err(err) => panic!("unexpected {err:?}"),
        }
        let differences: Vec<(Uuid, Uuid, i64, i64)> = sqlx::query_as(
            "with ledger as (
               select to_location_id as location_id, item_id, quantity::bigint as quantity from linen_movement
               where to_location_id is not null
               union all
               select from_location_id, item_id, -quantity from linen_movement where from_location_id is not null
             ), summed as (select location_id, item_id, sum(quantity)::bigint as quantity from ledger group by 1, 2)
             select coalesce(s.location_id, m.location_id), coalesce(s.item_id, m.item_id),
               coalesce(s.quantity, 0)::bigint, coalesce(m.quantity, 0)
             from linen_stock s full join summed m on m.location_id = s.location_id and m.item_id = s.item_id
             where coalesce(s.quantity, 0) <> coalesce(m.quantity, 0)",
        )
        .fetch_all(&mut *hotel.tx().await)
        .await
        .unwrap();
        assert!(differences.is_empty(), "stock differs from the ledger: {differences:?}");
    }
    assert!(accepted > 50 && refused > 20, "accepted {accepted}, refused {refused}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn par_totals_and_the_low_stock_alert(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let sheet = hotel.item("SHEET_K").await.id;
    let towel = hotel.item("TOWEL").await.id;
    let east = hotel.section("East").await;
    let west = hotel.section("West").await;
    let store = hotel.location("Store", LocationKind::Store, None).await.id;
    let pantry = hotel.location("Pantry East", LocationKind::Pantry, Some(east)).await.id;
    let laundry = hotel.location("Laundry", LocationKind::Laundry, None).await.id;
    hotel.room("101", Some(east)).await;
    hotel.room("102", Some(east)).await;
    let closed = hotel.room("103", Some(east)).await;
    hotel.room("201", Some(west)).await;
    hotel.room("301", None).await;
    let mut tx = hotel.tx().await;
    sqlx::query("update room set active = false where id = $1").bind(closed).execute(&mut *tx).await.unwrap();
    set_par(&mut tx, hotel.tenant, hotel.user, hotel.property, hotel.room_type, &[(sheet, 2)]).await.unwrap();
    tx.commit().await.unwrap();

    // The pantry covers its section's two active rooms; the store covers the West room and the one in no section.
    let cells = hotel.stock().await;
    let par =
        |location: Uuid, item: Uuid| cells.iter().find(|c| c.location_id == location && c.item_id == item).unwrap().par;
    assert_eq!(par(pantry, sheet), Some(4));
    assert_eq!(par(store, sheet), Some(4));
    assert_eq!(par(laundry, sheet), None);
    assert_eq!(par(store, towel), None);
    assert_eq!(cells.len(), 6);
    assert!(hotel.alert().await.is_none());

    hotel.record(receive(sheet, 10, store)).await.unwrap();
    assert_eq!(hotel.alert().await.as_deref(), Some("Pantry East: SHEET_K 0 of 4"));
    let cells = hotel.stock().await;
    let pantry_cell = cells.iter().find(|c| c.location_id == pantry && c.item_id == sheet).unwrap();
    assert!(pantry_cell.low);
    hotel.record(transfer(sheet, 4, store, pantry)).await.unwrap();
    assert!(hotel.alert().await.is_none());
    hotel.record(write_off(sheet, 4, store, Some("Torn"))).await.unwrap();
    hotel.record(write_off(sheet, 1, pantry, Some("Torn"))).await.unwrap();
    assert_eq!(hotel.alert().await.as_deref(), Some("Pantry East: SHEET_K 3 of 4; Store: SHEET_K 2 of 4"));

    // A movement that leaves the alert's text as it was neither raises it again nor notifies.
    let raised_at = |alerts: Vec<alerts::OpsAlert>| {
        alerts.into_iter().find(|alert| alert.kind == AlertKind::Linen).map(|alert| alert.raised_at)
    };
    let before = raised_at(alerts::list(&mut hotel.tx().await, hotel.property).await.unwrap());
    let mut listener = PgListener::connect_with(&hotel.pool).await.unwrap();
    listener.listen(db::CHANNEL).await.unwrap();
    hotel.record(receive(towel, 3, store)).await.unwrap();
    let after = raised_at(alerts::list(&mut hotel.tx().await, hotel.property).await.unwrap());
    assert_eq!(before, after);
    let mut keys = Vec::new();
    while let Ok(received) = tokio::time::timeout(Duration::from_millis(500), listener.recv()).await {
        let event: db::Event = serde_json::from_str(received.unwrap().payload()).unwrap();
        keys.extend(event.keys);
    }
    assert_eq!(keys, [format!("linen:{}", hotel.property)]);
    hotel.record(receive(sheet, 2, store)).await.unwrap();
    hotel.record(receive(sheet, 1, pantry)).await.unwrap();
    assert!(hotel.alert().await.is_none());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_long_alert_is_cut_to_2000_characters(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut items = Vec::new();
    for n in 0..51 {
        items.push((hotel.item(&format!("ITEM_WITH_LONG_C{n:03}")).await.id, 1));
    }
    hotel.room("101", None).await;
    let mut tx = hotel.tx().await;
    set_par(&mut tx, hotel.tenant, hotel.user, hotel.property, hotel.room_type, &items[..50]).await.unwrap();
    tx.commit().await.unwrap();
    let store = hotel.location("A store with a fairly long name", LocationKind::Store, None).await.id;
    hotel.record(receive(items[50].0, 1, store)).await.unwrap();

    let alert = hotel.alert().await.unwrap();
    assert_eq!(alert.chars().count(), 2000);
    assert!(alert.ends_with('…'));
    assert!(alert.starts_with("A store with a fairly long name: ITEM_WITH_LONG_C000 0 of 1; "));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn opposite_concurrent_transfers_do_not_deadlock(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let sheet = hotel.item("SHEET_K").await.id;
    let towel = hotel.item("TOWEL").await.id;
    let east = hotel.section("East").await;
    let store = hotel.location("Store", LocationKind::Store, None).await.id;
    let pantry = hotel.location("Pantry East", LocationKind::Pantry, Some(east)).await.id;
    for (item, location) in [(sheet, store), (sheet, pantry), (towel, store), (towel, pantry)] {
        hotel.record(receive(item, 100, location)).await.unwrap();
    }

    let move_both = |from: Uuid, to: Uuid| {
        let hotel = &hotel;
        async move {
            let mut tx = hotel.tx().await;
            let apply = |item| transfer(item, 3, from, to);
            let sheet_moved = record(&mut tx, hotel.tenant, hotel.user, hotel.property, apply(sheet)).await?;
            let towel_moved = record(&mut tx, hotel.tenant, hotel.user, hotel.property, apply(towel)).await?;
            tx.commit().await.unwrap();
            Ok::<_, LaundryError>((sheet_moved, towel_moved))
        }
    };
    for round in 0..20 {
        let both = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(move_both(store, pantry), move_both(pantry, store))
        })
        .await
        .unwrap_or_else(|_| panic!("round {round} timed out"));
        both.0.unwrap();
        both.1.unwrap();
    }
    for (item, location) in [(sheet, store), (sheet, pantry), (towel, store), (towel, pantry)] {
        assert_eq!(hotel.held(location, item).await, 100);
    }
}
