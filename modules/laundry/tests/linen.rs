mod common;

use common::Hotel;
use laundry::{
    LaundryError, LinenItem, LinenLocation, LocationKind, ParRow, create_item, create_location, laundry_key, linen_key,
    list_items, list_locations, list_par, set_par, update_item, update_location,
};
use sqlx::postgres::{PgConnectOptions, PgListener, PgPoolOptions};
use uuid::Uuid;

impl Hotel {
    async fn item(&self, code: &str) -> Result<LinenItem, LaundryError> {
        self.item_in(self.property, code).await
    }

    async fn item_in(&self, property: Uuid, code: &str) -> Result<LinenItem, LaundryError> {
        let mut tx = self.tx().await;
        let item = create_item(&mut tx, self.tenant, self.user, property, code, "Linen").await?;
        tx.commit().await.unwrap();
        Ok(item)
    }

    async fn location(
        &self,
        name: &str,
        kind: LocationKind,
        section: Option<Uuid>,
    ) -> Result<LinenLocation, LaundryError> {
        let mut tx = self.tx().await;
        let location = create_location(&mut tx, self.tenant, self.user, self.property, name, kind, section).await?;
        tx.commit().await.unwrap();
        Ok(location)
    }

    async fn rename_item(
        &self,
        id: Uuid,
        version: i32,
        name: Option<&str>,
        active: Option<bool>,
    ) -> Result<LinenItem, LaundryError> {
        let mut tx = self.tx().await;
        let item =
            update_item(&mut tx, self.tenant, self.user, self.property, id, version, name.map(Into::into), active)
                .await?;
        tx.commit().await.unwrap();
        Ok(item)
    }

    async fn change_location(
        &self,
        id: Uuid,
        version: i32,
        name: Option<&str>,
        active: Option<bool>,
    ) -> Result<LinenLocation, LaundryError> {
        let mut tx = self.tx().await;
        let location =
            update_location(&mut tx, self.tenant, self.user, self.property, id, version, name.map(Into::into), active)
                .await?;
        tx.commit().await.unwrap();
        Ok(location)
    }

    async fn par(&self, room_type: Uuid, rows: &[(Uuid, i32)]) -> Result<Vec<ParRow>, LaundryError> {
        let mut tx = self.tx().await;
        let saved = set_par(&mut tx, self.tenant, self.user, self.property, room_type, rows).await?;
        tx.commit().await.unwrap();
        Ok(saved)
    }
}

fn message(result: Result<impl std::fmt::Debug, LaundryError>) -> String {
    result.unwrap_err().to_string()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn items_are_created_and_updated(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let item = hotel.item("SHEET_K").await.unwrap();
    assert_eq!((item.code.as_str(), item.active, item.version), ("SHEET_K", true, 1));
    assert_eq!(hotel.audited("linen_item.created").await, 1);

    assert!(matches!(
        hotel.rename_item(item.id, 9, Some("King sheet"), None).await,
        Err(LaundryError::VersionMismatch("item"))
    ));
    let renamed = hotel.rename_item(item.id, 1, Some(" King sheet "), None).await.unwrap();
    assert_eq!((renamed.name.as_str(), renamed.version), ("King sheet", 2));
    let off = hotel.rename_item(item.id, 2, None, Some(false)).await.unwrap();
    assert_eq!((off.active, off.version), (false, 3));
    assert_eq!(hotel.audited("linen_item.updated").await, 2);
    assert_eq!(message(hotel.rename_item(item.id, 3, Some("King sheet"), Some(false)).await), "Nothing to change");
    assert!(matches!(hotel.rename_item(Uuid::now_v7(), 1, Some("x"), None).await, Err(LaundryError::NotFound("item"))));

    let mut tx = hotel.tx().await;
    assert_eq!(list_items(&mut tx, hotel.property).await.unwrap(), vec![off]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn item_codes_are_checked_and_unique(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.item("TOWEL_B").await.unwrap();
    assert_eq!(message(hotel.item("TOWEL_B").await), "an item with the code TOWEL_B already exists");
    for bad in ["", "towel", "TOWEL B", "A23456789012345678901"] {
        assert!(matches!(hotel.item(bad).await, Err(LaundryError::Invalid(_))), "{bad:?}");
    }
    let (other, _) = hotel.other_property().await;
    hotel.item_in(other, "TOWEL_B").await.unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn locations_are_created_and_updated(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let section = hotel.section("Floor 1").await;
    let store = hotel.location("Main store", LocationKind::Store, None).await.unwrap();
    let pantry = hotel.location("Pantry 1", LocationKind::Pantry, Some(section)).await.unwrap();
    assert_eq!((store.kind, store.section_id, store.version), (LocationKind::Store, None, 1));
    assert_eq!((pantry.kind, pantry.section_id), (LocationKind::Pantry, Some(section)));
    hotel.location("Laundry", LocationKind::Laundry, None).await.unwrap();
    hotel.location("Vendor", LocationKind::Vendor, None).await.unwrap();
    assert_eq!(hotel.audited("linen_location.created").await, 4);

    let renamed = hotel.change_location(store.id, 1, Some("Store"), None).await.unwrap();
    assert_eq!((renamed.name.as_str(), renamed.version), ("Store", 2));
    assert!(matches!(
        hotel.change_location(store.id, 1, Some("Again"), None).await,
        Err(LaundryError::VersionMismatch("location"))
    ));
    assert_eq!(message(hotel.change_location(store.id, 2, None, Some(true)).await), "Nothing to change");
    let off = hotel.change_location(store.id, 2, None, Some(false)).await.unwrap();
    assert!(!off.active);
    assert_eq!(hotel.audited("linen_location.updated").await, 2);

    let mut tx = hotel.tx().await;
    assert_eq!(list_locations(&mut tx, hotel.property).await.unwrap().len(), 4);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn location_rules_are_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let section = hotel.section("Floor 1").await;
    let store = hotel.location("Main store", LocationKind::Store, None).await.unwrap();
    assert_eq!(
        message(hotel.location("Main store", LocationKind::Vendor, None).await),
        "a location named Main store already exists"
    );
    assert_eq!(message(hotel.location("Store 2", LocationKind::Store, None).await), "The property already has a store");

    assert!(matches!(hotel.location("P", LocationKind::Pantry, None).await, Err(LaundryError::Invalid(_))));
    assert!(matches!(
        hotel.location("P", LocationKind::Pantry, Some(Uuid::now_v7())).await,
        Err(LaundryError::NotFound("section"))
    ));
    for kind in [LocationKind::Store, LocationKind::Laundry, LocationKind::Vendor] {
        assert!(matches!(hotel.location("Other", kind, Some(section)).await, Err(LaundryError::Invalid(_))));
    }
    hotel.location("Pantry 1", LocationKind::Pantry, Some(section)).await.unwrap();
    assert_eq!(
        message(hotel.location("Pantry 2", LocationKind::Pantry, Some(section)).await),
        "The section already has a pantry"
    );

    // A second store is refused on reactivation too.
    let off = hotel.change_location(store.id, 1, None, Some(false)).await.unwrap();
    let second = hotel.location("Store 2", LocationKind::Store, None).await.unwrap();
    assert_eq!(
        message(hotel.change_location(store.id, off.version, None, Some(true)).await),
        "The property already has a store"
    );
    assert!(second.active);

    // A section of another property cannot be used.
    let (other, _) = hotel.other_property().await;
    let mut tx = hotel.tx().await;
    let result =
        create_location(&mut tx, hotel.tenant, hotel.user, other, "P", LocationKind::Pantry, Some(section)).await;
    assert!(matches!(result, Err(LaundryError::NotFound("section"))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_location_holding_stock_stays_active(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let store = hotel.location("Main store", LocationKind::Store, None).await.unwrap();
    let item = hotel.item("SHEET_K").await.unwrap();
    let mut tx = hotel.tx().await;
    sqlx::query(
        "insert into linen_stock (tenant_id, property_id, location_id, item_id, quantity) values ($1, $2, $3, $4, 5)",
    )
    .bind(hotel.tenant.0)
    .bind(hotel.property)
    .bind(store.id)
    .bind(item.id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        message(hotel.change_location(store.id, 1, None, Some(false)).await),
        "Move or write off its stock first"
    );

    let mut tx = hotel.tx().await;
    sqlx::query("update linen_stock set quantity = 0").execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    assert!(!hotel.change_location(store.id, 1, None, Some(false)).await.unwrap().active);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_item_with_stock_stays_active(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let laundry = hotel.location("Laundry", LocationKind::Laundry, None).await.unwrap();
    let item = hotel.item("SHEET_K").await.unwrap();
    let mut tx = hotel.tx().await;
    sqlx::query(
        "insert into linen_stock (tenant_id, property_id, location_id, item_id, quantity) values ($1, $2, $3, $4, 2)",
    )
    .bind(hotel.tenant.0)
    .bind(hotel.property)
    .bind(laundry.id)
    .bind(item.id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let refused = hotel.rename_item(item.id, 1, None, Some(false)).await;
    assert!(matches!(&refused, Err(LaundryError::Conflict(_))), "{refused:?}");
    assert_eq!(message(refused), "Move or write off its stock first");
    // Renaming it is still fine.
    let renamed = hotel.rename_item(item.id, 1, Some("King sheet"), None).await.unwrap();

    let mut tx = hotel.tx().await;
    sqlx::query("update linen_stock set quantity = 0").execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    assert!(!hotel.rename_item(item.id, renamed.version, None, Some(false)).await.unwrap().active);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn par_is_replaced_whole(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let (sheet, towel) = (hotel.item("SHEET_K").await.unwrap(), hotel.item("TOWEL_B").await.unwrap());
    let first = hotel.par(hotel.room_type, &[(sheet.id, 3), (towel.id, 2)]).await.unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(hotel.audited("linen_par.updated").await, 1);

    let second = hotel.par(hotel.room_type, &[(towel.id, 4)]).await.unwrap();
    assert_eq!(second, vec![ParRow { room_type_id: hotel.room_type, item_id: towel.id, quantity: 4 }]);
    let mut tx = hotel.tx().await;
    assert_eq!(list_par(&mut tx, hotel.property).await.unwrap(), second);
    drop(tx);

    assert!(hotel.par(hotel.room_type, &[]).await.unwrap().is_empty());
    let mut tx = hotel.tx().await;
    assert!(list_par(&mut tx, hotel.property).await.unwrap().is_empty());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn bad_par_is_refused_and_changes_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let sheet = hotel.item("SHEET_K").await.unwrap();
    hotel.par(hotel.room_type, &[(sheet.id, 3)]).await.unwrap();

    for quantity in [0, 101, -1] {
        assert!(matches!(hotel.par(hotel.room_type, &[(sheet.id, quantity)]).await, Err(LaundryError::Invalid(_))));
    }
    assert!(matches!(hotel.par(hotel.room_type, &[(sheet.id, 1), (sheet.id, 2)]).await, Err(LaundryError::Invalid(_))));
    let mut many = Vec::new();
    for n in 0..51 {
        many.push((Uuid::now_v7(), 1 + n % 5));
    }
    assert!(matches!(hotel.par(hotel.room_type, &many).await, Err(LaundryError::Invalid(_))));
    assert!(matches!(hotel.par(Uuid::now_v7(), &[(sheet.id, 1)]).await, Err(LaundryError::NotFound("room type"))));

    let (other, other_room_type) = hotel.other_property().await;
    let foreign = hotel.item_in(other, "SHEET_K").await.unwrap();
    assert!(matches!(hotel.par(hotel.room_type, &[(foreign.id, 1)]).await, Err(LaundryError::NotFound("item"))));
    assert!(matches!(hotel.par(other_room_type, &[(sheet.id, 1)]).await, Err(LaundryError::NotFound("room type"))));

    let mut tx = hotel.tx().await;
    let rows = list_par(&mut tx, hotel.property).await.unwrap();
    assert_eq!(rows.iter().map(|row| row.quantity).collect::<Vec<_>>(), vec![3]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn writes_notify_the_linen_key(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut listener = PgListener::connect_with(&hotel.pool).await.unwrap();
    listener.listen(db::CHANNEL).await.unwrap();
    hotel.item("SHEET_K").await.unwrap();
    let payload = listener.recv().await.unwrap().payload().to_owned();
    assert!(payload.contains(&linen_key(hotel.property)), "{payload}");
    assert_eq!(linen_key(hotel.property), format!("linen:{}", hotel.property));
    assert_eq!(laundry_key(hotel.property), format!("laundry:{}", hotel.property));
}
