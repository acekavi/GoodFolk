mod common;

use common::Hotel;
use laundry::{LaundryError, Price, Service, create_price, laundry_key, list_prices, update_price};
use sqlx::postgres::{PgConnectOptions, PgListener, PgPoolOptions};
use uuid::Uuid;

impl Hotel {
    async fn price(&self, code: &str, service: Service, amount: i64) -> Result<Price, LaundryError> {
        let mut tx = self.tx().await;
        let price =
            create_price(&mut tx, self.tenant, self.user, self.property, code, service, "Shirt", amount).await?;
        tx.commit().await.unwrap();
        Ok(price)
    }

    async fn reprice(
        &self,
        id: Uuid,
        version: i32,
        name: Option<&str>,
        amount: Option<i64>,
        active: Option<bool>,
    ) -> Result<Price, LaundryError> {
        let mut tx = self.tx().await;
        let price = update_price(
            &mut tx,
            self.tenant,
            self.user,
            self.property,
            id,
            version,
            name.map(Into::into),
            amount,
            active,
        )
        .await?;
        tx.commit().await.unwrap();
        Ok(price)
    }
}

fn message(result: Result<impl std::fmt::Debug, LaundryError>) -> String {
    result.unwrap_err().to_string()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn prices_are_created_and_updated(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let price = hotel.price("SHIRT", Service::Wash, 500).await.unwrap();
    assert_eq!((price.service, price.price, price.active, price.version), (Service::Wash, 500, true, 1));
    assert_eq!(hotel.audited("laundry_price.created").await, 1);

    assert!(matches!(
        hotel.reprice(price.id, 9, None, Some(600), None).await,
        Err(LaundryError::VersionMismatch("price"))
    ));
    let changed = hotel.reprice(price.id, 1, Some(" Shirt wash "), Some(600), None).await.unwrap();
    assert_eq!((changed.name.as_str(), changed.price, changed.version), ("Shirt wash", 600, 2));
    let off = hotel.reprice(price.id, 2, None, None, Some(false)).await.unwrap();
    assert_eq!((off.active, off.version), (false, 3));
    assert_eq!(hotel.audited("laundry_price.updated").await, 2);
    assert_eq!(
        message(hotel.reprice(price.id, 3, Some("Shirt wash"), Some(600), Some(false)).await),
        "Nothing to change"
    );
    assert!(matches!(hotel.reprice(price.id, 3, None, Some(0), None).await, Err(LaundryError::Invalid(_))));
    assert!(matches!(
        hotel.reprice(Uuid::now_v7(), 1, None, Some(1), None).await,
        Err(LaundryError::NotFound("price"))
    ));

    let mut tx = hotel.tx().await;
    assert!(list_prices(&mut tx, hotel.property, false).await.unwrap().is_empty());
    assert_eq!(list_prices(&mut tx, hotel.property, true).await.unwrap(), vec![off]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_duplicate_price_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.price("SHIRT", Service::Wash, 500).await.unwrap();
    assert_eq!(message(hotel.price("SHIRT", Service::Wash, 700).await), "SHIRT already has a wash price");
    hotel.price("SHIRT", Service::Press, 300).await.unwrap();
    hotel.price("TROUSERS", Service::Wash, 500).await.unwrap();
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn prices_are_listed_by_code_then_service(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    for (code, service) in [
        ("TROUSERS", Service::Wash),
        ("SHIRT", Service::Express),
        ("SHIRT", Service::DryClean),
        ("SHIRT", Service::Wash),
        ("SHIRT", Service::Press),
    ] {
        hotel.price(code, service, 100).await.unwrap();
    }
    let mut tx = hotel.tx().await;
    let listed: Vec<_> = list_prices(&mut tx, hotel.property, false)
        .await
        .unwrap()
        .into_iter()
        .map(|p| (p.item_code, p.service))
        .collect();
    let expected = [
        ("SHIRT", Service::Wash),
        ("SHIRT", Service::Press),
        ("SHIRT", Service::DryClean),
        ("SHIRT", Service::Express),
        ("TROUSERS", Service::Wash),
    ];
    assert_eq!(listed, expected.map(|(code, service)| (code.to_owned(), service)));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn invalid_prices_are_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    for bad in ["", "shirt", "SHIRT 1", "A23456789012345678901"] {
        assert!(matches!(hotel.price(bad, Service::Wash, 500).await, Err(LaundryError::Invalid(_))), "{bad:?}");
    }
    for amount in [0, -5] {
        assert!(matches!(hotel.price("SHIRT", Service::Wash, amount).await, Err(LaundryError::Invalid(_))));
    }
    let mut tx = hotel.tx().await;
    let result =
        create_price(&mut tx, hotel.tenant, hotel.user, hotel.property, "SHIRT", Service::Wash, " ", 500).await;
    assert!(matches!(result, Err(LaundryError::Invalid(_))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn writes_notify_the_laundry_key(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut listener = PgListener::connect_with(&hotel.pool).await.unwrap();
    listener.listen(db::CHANNEL).await.unwrap();
    hotel.price("SHIRT", Service::Wash, 500).await.unwrap();
    let payload = listener.recv().await.unwrap().payload().to_owned();
    assert!(payload.contains(&laundry_key(hotel.property)), "{payload}");
}
