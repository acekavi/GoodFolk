//! Guest laundry orders and the charge their delivery posts, on stays seeded as the night audit's tests seed theirs.

#[path = "../../audit/tests/common/mod.rs"]
mod stays;

use folio::{NewCharge, Source};
use laundry::{LaundryError, Order, OrderStatus, Service, create_order, set_status, update_lines};
use sqlx::postgres::{PgConnectOptions, PgListener, PgPoolOptions};
use stays::{Booked, Hotel, Stay};
use std::time::Duration;
use uuid::Uuid;

impl Hotel {
    /// An active price of the hotel's property, committed.
    async fn price(&self, code: &str, service: Service, name: &str, amount: i64) -> Uuid {
        let mut tx = self.tx().await;
        let price = laundry::create_price(&mut tx, self.tenant, self.user, self.property, code, service, name, amount)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        price.id
    }

    /// A checked-in stay in room `room` of the hotel's property.
    async fn in_house(&self, room: &str) -> Booked {
        self.stay(Stay { room: Some(room), ..Stay::new(self.property, "checked_in", -1, 2) }).await
    }

    async fn order(&self, stay: Uuid, lines: &[(Uuid, i32)]) -> Result<Order, LaundryError> {
        let mut tx = self.tx().await;
        let order = create_order(&mut tx, self.tenant, self.user, self.property, stay, lines, None).await?;
        tx.commit().await.unwrap();
        Ok(order)
    }

    async fn move_to(&self, order: &Order, to: OrderStatus) -> Result<Order, LaundryError> {
        let mut tx = self.tx().await;
        let moved = set_status(&mut tx, self.tenant, self.user, self.property, order.id, order.version, to).await?;
        tx.commit().await.unwrap();
        Ok(moved)
    }

    /// `order` moved step by step to `ready`.
    async fn ready(&self, order: Order) -> Order {
        let processing = self.move_to(&order, OrderStatus::Processing).await.unwrap();
        self.move_to(&processing, OrderStatus::Ready).await.unwrap()
    }

    /// The data of the audit entries named `action`, oldest first.
    async fn audited(&self, action: &str) -> Vec<serde_json::Value> {
        sqlx::query_scalar("select data from audit_log where action = $1 order by id")
            .bind(action)
            .fetch_all(&mut *self.tx().await)
            .await
            .unwrap()
    }

    async fn reload(&self, stay: Uuid) -> Vec<Order> {
        laundry::orders_for_stay(&mut self.tx().await, self.property, stay).await.unwrap()
    }

    /// `(folio, source, category, description, quantity, net, total, posting key)` of the order's lines, by key.
    async fn order_lines(&self, order: Uuid) -> Vec<(Uuid, String, String, String, i32, i64, i64, String)> {
        sqlx::query_as(
            "select folio_id, source, category, description, quantity, net, total, posting_key from folio_line
             where starts_with(posting_key, $1) order by posting_key",
        )
        .bind(format!("laundry:{order}:"))
        .fetch_all(&mut *self.tx().await)
        .await
        .unwrap()
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_order_keeps_the_prices_it_was_taken_at(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let shirt = hotel.price("SHIRT", Service::Wash, "Shirt", 500).await;
    let suit = hotel.price("SUIT", Service::DryClean, "Suit", 2_000).await;
    let stay = hotel.in_house("101").await;

    let order = hotel.order(stay.id, &[(shirt, 2), (suit, 1), (shirt, 1)]).await.unwrap();

    assert_eq!(
        (order.room_number.as_str(), order.guest.as_str(), order.status, order.total, order.currency.as_str()),
        ("101", "Ada Silva", OrderStatus::Collected, 3_500, "LKR")
    );
    let positions: Vec<(i32, &str, Service)> =
        order.lines.iter().map(|line| (line.position, line.item_code.as_str(), line.service)).collect();
    assert_eq!(
        positions,
        vec![(1, "SHIRT", Service::Wash), (2, "SUIT", Service::DryClean), (3, "SHIRT", Service::Wash)]
    );
    assert_eq!(hotel.audited("laundry_order.created").await.len(), 1);

    let mut tx = hotel.tx().await;
    laundry::update_price(&mut tx, hotel.tenant, hotel.user, hotel.property, shirt, 1, None, Some(900), None)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let reloaded = hotel.reload(stay.id).await;
    assert_eq!(reloaded, vec![order.clone()]);
    let listed = laundry::list_orders(&mut hotel.tx().await, hotel.property, None).await.unwrap();
    assert_eq!(listed, vec![order]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn lines_need_an_active_price_and_a_quantity_of_1_to_100(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let shirt = hotel.price("SHIRT", Service::Wash, "Shirt", 500).await;
    let stay = hotel.in_house("101").await;

    for lines in [vec![], vec![(shirt, 1); 51], vec![(shirt, 0)], vec![(shirt, 101)], vec![(Uuid::now_v7(), 1)]] {
        assert!(matches!(hotel.order(stay.id, &lines).await, Err(LaundryError::Invalid(_))), "{lines:?}");
    }
    let mut tx = hotel.tx().await;
    laundry::update_price(&mut tx, hotel.tenant, hotel.user, hotel.property, shirt, 1, None, None, Some(false))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(matches!(hotel.order(stay.id, &[(shirt, 1)]).await, Err(LaundryError::Invalid(_))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_order_is_taken_only_for_an_in_house_stay(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let shirt = hotel.price("SHIRT", Service::Wash, "Shirt", 500).await;
    let arriving = hotel.stay(Stay { room: Some("101"), ..Stay::new(hotel.property, "confirmed", 0, 2) }).await;
    let other = hotel.add_property("KAN").await;
    let elsewhere = hotel.stay(Stay { room: Some("201"), ..Stay::new(other, "checked_in", -1, 2) }).await;

    for stay in [arriving.id, elsewhere.id] {
        let err = hotel.order(stay, &[(shirt, 1)]).await.unwrap_err();
        assert_eq!(err.to_string(), "Laundry is taken for an in-house stay");
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_order_moves_one_step_at_a_time_or_is_cancelled(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let shirt = hotel.price("SHIRT", Service::Wash, "Shirt", 500).await;
    let stay = hotel.in_house("101").await;
    let order = hotel.order(stay.id, &[(shirt, 1)]).await.unwrap();

    let err = hotel.move_to(&order, OrderStatus::Ready).await.unwrap_err();
    assert_eq!(err.to_string(), "An order can't move from collected to ready");
    assert!(matches!(
        hotel.move_to(&Order { version: 9, ..order.clone() }, OrderStatus::Processing).await,
        Err(LaundryError::VersionMismatch("order"))
    ));
    let processing = hotel.move_to(&order, OrderStatus::Processing).await.unwrap();
    assert_eq!((processing.status, processing.version, processing.delivered_at), (OrderStatus::Processing, 2, None));
    let ready = hotel.move_to(&processing, OrderStatus::Ready).await.unwrap();
    let cancelled = hotel.move_to(&ready, OrderStatus::Cancelled).await.unwrap();
    assert_eq!((cancelled.status, cancelled.version), (OrderStatus::Cancelled, 4));

    let fresh = hotel.order(stay.id, &[(shirt, 1)]).await.unwrap();
    assert_eq!(hotel.move_to(&fresh, OrderStatus::Cancelled).await.unwrap().status, OrderStatus::Cancelled);

    let moves: Vec<(String, String)> = hotel
        .audited("laundry_order.status_changed")
        .await
        .into_iter()
        .map(|data| (data["from"].as_str().unwrap().into(), data["to"].as_str().unwrap().into()))
        .collect();
    let expected =
        [("collected", "processing"), ("processing", "ready"), ("ready", "cancelled"), ("collected", "cancelled")];
    assert_eq!(moves, expected.map(|(from, to)| (from.to_owned(), to.to_owned())));
    assert!(hotel.order_lines(order.id).await.is_empty());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_cancelled_or_delivered_order_moves_no_further(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let shirt = hotel.price("SHIRT", Service::Wash, "Shirt", 500).await;
    let stay = hotel.in_house("101").await;
    let cancelled = hotel.order(stay.id, &[(shirt, 1)]).await.unwrap();
    let cancelled = hotel.move_to(&cancelled, OrderStatus::Cancelled).await.unwrap();
    let delivered = hotel.ready(hotel.order(stay.id, &[(shirt, 1)]).await.unwrap()).await;
    let delivered = hotel.move_to(&delivered, OrderStatus::Delivered).await.unwrap();

    for (order, to) in [
        (&cancelled, OrderStatus::Delivered),
        (&cancelled, OrderStatus::Cancelled),
        (&delivered, OrderStatus::Delivered),
        (&delivered, OrderStatus::Cancelled),
    ] {
        let err = hotel.move_to(order, to).await.unwrap_err();
        assert_eq!(err.to_string(), format!("An order can't move from {} to {}", order.status.as_str(), to.as_str()));
    }
    assert_eq!(hotel.order_lines(delivered.id).await.len(), 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn delivery_posts_a_taxed_laundry_line_per_order_line(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let shirt = hotel.price("SHIRT", Service::Wash, "Shirt", 500).await;
    let suit = hotel.price("SUIT", Service::DryClean, "Suit", 2_000).await;
    let stay = hotel.in_house("101").await;
    let order = hotel.ready(hotel.order(stay.id, &[(shirt, 3), (suit, 1)]).await.unwrap()).await;
    let mut listener = PgListener::connect_with(&hotel.pool).await.unwrap();
    listener.listen(db::CHANNEL).await.unwrap();

    let delivered = hotel.move_to(&order, OrderStatus::Delivered).await.unwrap();

    let payload = loop {
        let payload = listener.recv().await.unwrap().payload().to_owned();
        if payload.contains(&laundry::laundry_key(hotel.property)) {
            break payload;
        }
    };
    assert!(payload.contains(&folio::folio_key(stay.guest_folio)), "{payload}");

    assert_eq!(delivered.status, OrderStatus::Delivered);
    assert!(delivered.delivered_at.is_some());
    let lines = hotel.order_lines(order.id).await;
    let posted: Vec<_> = lines
        .iter()
        .map(|(folio, source, category, description, quantity, net, _, key)| {
            (*folio, source.as_str(), category.as_str(), description.as_str(), *quantity, *net, key.clone())
        })
        .collect();
    let key = |n: i32| format!("laundry:{}:{n}", order.id);
    assert_eq!(
        posted,
        vec![
            (stay.guest_folio, "laundry", "laundry", "Laundry: Shirt × 3", 3, 1_500, key(1)),
            (stay.guest_folio, "laundry", "laundry", "Laundry: Suit × 1", 1, 2_000, key(2)),
        ]
    );
    assert!(lines.iter().all(|(.., net, total, _)| total > net), "the lines are taxed: {lines:?}");
    let data = hotel.audited("laundry_order.status_changed").await.pop().unwrap();
    assert_eq!(data, serde_json::json!({ "from": "ready", "to": "delivered" }));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_routed_laundry_category_sends_the_lines_to_the_account(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let shirt = hotel.price("SHIRT", Service::Wash, "Shirt", 500).await;
    let stay = hotel.in_house("101").await;
    hotel.bill_to_account(stay.reservation).await;
    let mut tx = hotel.tx().await;
    folio::set_routing(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, stay.reservation, &["laundry".into()])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let order = hotel.ready(hotel.order(stay.id, &[(shirt, 1)]).await.unwrap()).await;

    hotel.move_to(&order, OrderStatus::Delivered).await.unwrap();

    let lines = hotel.order_lines(order.id).await;
    assert_eq!(lines.len(), 1);
    let kind: String = sqlx::query_scalar("select kind from folio where id = $1")
        .bind(lines[0].0)
        .fetch_one(&mut *hotel.tx().await)
        .await
        .unwrap();
    assert_eq!(kind, "account");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_line_already_posted_under_its_key_is_not_posted_again(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let shirt = hotel.price("SHIRT", Service::Wash, "Shirt", 500).await;
    let stay = hotel.in_house("101").await;
    let order = hotel.ready(hotel.order(stay.id, &[(shirt, 1), (shirt, 2)]).await.unwrap()).await;
    let first = NewCharge {
        source: Source::Laundry,
        category: "laundry".into(),
        description: "Laundry: Shirt × 1".into(),
        quantity: 1,
        unit_net: 500,
        currency: "LKR".into(),
        business_date: hotel.business_date,
        posting_key: Some(format!("laundry:{}:1", order.id)),
        inclusive: false,
    };
    let mut tx = hotel.tx().await;
    folio::post_for_stay(&mut tx, hotel.tenant, Some(hotel.user), stay.id, first).await.unwrap();
    tx.commit().await.unwrap();

    hotel.move_to(&order, OrderStatus::Delivered).await.unwrap();

    let quantities: Vec<i32> = hotel.order_lines(order.id).await.into_iter().map(|line| line.4).collect();
    assert_eq!(quantities, vec![1, 2]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_delivery_the_folio_refuses_leaves_the_order_ready(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let shirt = hotel.price("SHIRT", Service::Wash, "Shirt", 500).await;
    let stay = hotel.in_house("101").await;
    let order = hotel.ready(hotel.order(stay.id, &[(shirt, 1), (shirt, 2)]).await.unwrap()).await;
    let mut tx = hotel.tx().await;
    sqlx::query("update folio set status = 'invoiced' where id = $1")
        .bind(stay.guest_folio)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let err = hotel.move_to(&order, OrderStatus::Delivered).await.unwrap_err();

    assert!(matches!(err, LaundryError::Folio(folio::FolioError::Conflict(_))), "{err:?}");
    assert_eq!(hotel.reload(stay.id).await, vec![order.clone()]);
    assert!(hotel.order_lines(order.id).await.is_empty());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_guest_who_checked_out_is_not_charged_on_delivery(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let shirt = hotel.price("SHIRT", Service::Wash, "Shirt", 500).await;
    let stay = hotel.in_house("101").await;
    let order = hotel.ready(hotel.order(stay.id, &[(shirt, 1)]).await.unwrap()).await;
    hotel.check_out(stay.id).await;

    let err = hotel.move_to(&order, OrderStatus::Delivered).await.unwrap_err();

    assert_eq!(err.to_string(), "The guest has checked out; post the charge on their folio");
    assert_eq!(hotel.reload(stay.id).await, vec![order.clone()]);
    assert!(hotel.order_lines(order.id).await.is_empty());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn lines_change_only_while_collected(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let shirt = hotel.price("SHIRT", Service::Wash, "Shirt", 500).await;
    let suit = hotel.price("SUIT", Service::DryClean, "Suit", 2_000).await;
    let stay = hotel.in_house("101").await;
    let order = hotel.order(stay.id, &[(shirt, 1), (shirt, 2)]).await.unwrap();
    let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);

    let mut tx = hotel.tx().await;
    let note = Some(Some(" Starch the collar ".into()));
    let changed = update_lines(&mut tx, tenant, user, property, order.id, 1, &[(suit, 2)], note).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!((changed.version, changed.total, changed.note.as_deref()), (2, 4_000, Some("Starch the collar")));
    let lines: Vec<(i32, &str, i32)> =
        changed.lines.iter().map(|line| (line.position, line.item_code.as_str(), line.quantity)).collect();
    assert_eq!(lines, vec![(1, "SUIT", 2)]);

    let processing = hotel.move_to(&changed, OrderStatus::Processing).await.unwrap();
    let mut tx = hotel.tx().await;
    let err = update_lines(&mut tx, tenant, user, property, order.id, processing.version, &[(shirt, 1)], None)
        .await
        .unwrap_err();
    drop(tx);
    assert_eq!(err.to_string(), "The order is processing; its lines can only be changed while it is collected");
    assert_eq!(hotel.reload(stay.id).await, vec![processing]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_delivery_waits_for_a_concurrent_check_out(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let shirt = hotel.price("SHIRT", Service::Wash, "Shirt", 500).await;
    let stay = hotel.in_house("101").await;
    let order = hotel.ready(hotel.order(stay.id, &[(shirt, 1)]).await.unwrap()).await;

    // A check-out holding the stay's row.
    let mut checking_out = hotel.tx().await;
    sqlx::query("update reservation_room set status = 'checked_out', checked_out_at = now() where id = $1")
        .bind(stay.id)
        .execute(&mut *checking_out)
        .await
        .unwrap();

    let delivery = hotel.move_to(&order, OrderStatus::Delivered);
    tokio::pin!(delivery);
    let waited = tokio::time::timeout(Duration::from_millis(300), &mut delivery).await;
    assert!(waited.is_err(), "the delivery did not wait for the check-out: {waited:?}");
    checking_out.commit().await.unwrap();

    let err = tokio::time::timeout(Duration::from_secs(10), delivery).await.unwrap().unwrap_err();
    assert_eq!(err.to_string(), "The guest has checked out; post the charge on their folio");
    assert!(hotel.order_lines(order.id).await.is_empty());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_edit_keeps_the_prices_already_taken(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let shirt = hotel.price("SHIRT", Service::Wash, "Shirt", 500).await;
    let suit = hotel.price("SUIT", Service::DryClean, "Suit", 2_000).await;
    let tie = hotel.price("TIE", Service::Press, "Tie", 300).await;
    let stay = hotel.in_house("101").await;
    let order = hotel.order(stay.id, &[(shirt, 1), (suit, 1)]).await.unwrap();
    let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);

    let mut tx = hotel.tx().await;
    laundry::update_price(&mut tx, tenant, user, property, shirt, 1, Some("Shirt wash".into()), Some(900), None)
        .await
        .unwrap();
    laundry::update_price(&mut tx, tenant, user, property, suit, 1, None, None, Some(false)).await.unwrap();
    tx.commit().await.unwrap();

    // The shirt keeps its old price and name, the switched-off suit stays, and the new tie takes today's price.
    let mut tx = hotel.tx().await;
    let lines = [(shirt, 1), (suit, 3), (tie, 2)];
    let edited = update_lines(&mut tx, tenant, user, property, order.id, order.version, &lines, None).await.unwrap();
    tx.commit().await.unwrap();
    let priced: Vec<(i32, &str, &str, i64, i32)> = edited
        .lines
        .iter()
        .map(|line| (line.position, line.item_code.as_str(), line.name.as_str(), line.unit_price, line.quantity))
        .collect();
    assert_eq!(priced, vec![(1, "SHIRT", "Shirt", 500, 1), (2, "SUIT", "Suit", 2_000, 3), (3, "TIE", "Tie", 300, 2)]);
    assert_eq!(edited.total, 7_100);

    // A switched-off price new to the order is still refused.
    let mut tx = hotel.tx().await;
    laundry::update_price(&mut tx, tenant, user, property, tie, 1, None, None, Some(false)).await.unwrap();
    tx.commit().await.unwrap();
    let fresh = hotel.order(stay.id, &[(shirt, 1)]).await.unwrap();
    let mut tx = hotel.tx().await;
    let refused = update_lines(&mut tx, tenant, user, property, fresh.id, fresh.version, &[(tie, 1)], None).await;
    assert!(matches!(refused, Err(LaundryError::Invalid(_))), "{refused:?}");
}
