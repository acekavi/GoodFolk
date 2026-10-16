mod common;

use common::Hotel;
use folio::{FolioError, NewCharge, Source};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

/// The account's folio in the hotel's property, if it has one.
async fn account_folio(hotel: &Hotel, account: Uuid) -> Option<Uuid> {
    sqlx::query_scalar("select id from folio where kind = 'account' and account_id = $1 and property_id = $2")
        .bind(account)
        .bind(hotel.property)
        .fetch_optional(&mut *hotel.tx().await)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn routing_sends_room_to_the_account_folio_and_the_bar_to_the_guest_folio(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let hotel = Hotel::new(opts).await;
    let acme = hotel.account().await;
    let stay = hotel.book(Some(acme)).await;
    hotel.route(stay.id, &["room", "meal_plan", "room"]).await.unwrap();
    assert_eq!(account_folio(&hotel, acme).await, None, "an account folio opens on first use");

    let room = hotel.post_for_stay(stay.rooms[0], hotel.charge(Source::Room, "room", 10_000, "LKR")).await.unwrap();
    let bar = hotel.post_for_stay(stay.rooms[0], hotel.charge(Source::Manual, "pos:BAR", 500, "LKR")).await.unwrap();

    let acme_folio = account_folio(&hotel, acme).await.expect("opened by the routed charge");
    assert_eq!(room.folio_id, acme_folio);
    assert_eq!(bar.folio_id, stay.guests[0]);
    let rules: Vec<(String, Uuid)> =
        sqlx::query_as("select category, account_id from routing_rule where reservation_id = $1 order by category")
            .bind(stay.id)
            .fetch_all(&mut *hotel.tx().await)
            .await
            .unwrap();
    assert_eq!(rules, vec![("meal_plan".into(), acme), ("room".into(), acme)]);
    let audited = hotel.audited("reservation.routing_set").await;
    assert_eq!(audited.len(), 1);
    assert_eq!(audited[0].1, serde_json::json!({ "categories": ["meal_plan", "room"], "account_id": acme }));

    hotel.route(stay.id, &[]).await.unwrap();
    let after = hotel.post_for_stay(stay.rooms[0], hotel.charge(Source::Room, "room", 10_000, "LKR")).await.unwrap();
    assert_eq!(after.folio_id, stay.guests[0], "with no rules every charge stays on the guest folio");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_routing_change_never_posts_a_key_twice(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let acme = hotel.account().await;
    let stay = hotel.book(Some(acme)).await;
    let key = format!("room:{}:{}", stay.rooms[0], hotel.business_date);
    let charge = NewCharge { posting_key: Some(key.clone()), ..hotel.charge(Source::Room, "room", 10_000, "LKR") };

    let first = hotel.post_for_stay(stay.rooms[0], charge.clone()).await.unwrap();
    hotel.route(stay.id, &["room"]).await.unwrap();
    let again = hotel.post_for_stay(stay.rooms[0], charge).await.unwrap();

    assert_eq!(again, first, "the replay returns the line where it was posted");
    let acme_folio = account_folio(&hotel, acme).await.expect("the routed replay opened it");
    assert_eq!(
        hotel.lines_on(&[stay.guests[0], acme_folio, stay.deposit]).await,
        vec![(stay.guests[0], first.total, Some(key))],
        "one line, still on the guest folio"
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_rule_for_a_former_account_routes_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let (acme, globex) = (hotel.account().await, hotel.account().await);
    let stay = hotel.book(Some(acme)).await;
    hotel.route(stay.id, &["room"]).await.unwrap();
    let mut tx = hotel.tx().await;
    sqlx::query("update reservation set account_id = $2 where id = $1")
        .bind(stay.id)
        .bind(globex)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let line = hotel.post_for_stay(stay.rooms[0], hotel.charge(Source::Room, "room", 10_000, "LKR")).await.unwrap();

    assert_eq!(line.folio_id, stay.guests[0]);
    assert_eq!(account_folio(&hotel, acme).await, None);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn routing_needs_an_account_and_real_categories(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let acme = hotel.account().await;
    let (billed, unbilled) = (hotel.book(Some(acme)).await, hotel.book(None).await);

    assert!(matches!(hotel.route(unbilled.id, &["room"]).await, Err(FolioError::Invalid(_))));
    hotel.route(unbilled.id, &[]).await.unwrap();
    assert!(matches!(hotel.route(billed.id, &["spa"]).await, Err(FolioError::Invalid(_))));
    assert!(matches!(hotel.route(Uuid::now_v7(), &["room"]).await, Err(FolioError::NotFound("reservation"))));
    let charge = hotel.charge(Source::Room, "room", 1, "LKR");
    assert!(matches!(hotel.post_for_stay(Uuid::now_v7(), charge).await, Err(FolioError::NotFound("reservation room"))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_is_taxed_with_its_rate_plan_s_residency(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut tx = hotel.tx().await;
    sqlx::query(
        "insert into tax_rule (id, tenant_id, property_id, code, name, kind, rate_bp, applies_to, base, sequence,
                               presentation, condition, effective_from)
         values ($1, $2, $3, 'NRL', 'Non-resident levy', 'levy', 1000, '{*}', '{net}', 50, 'on_bill',
                 '{\"type\": \"residency\", \"resident\": false}', date '2026-01-01')",
    )
    .bind(Uuid::now_v7())
    .bind(hotel.tenant.0)
    .bind(hotel.property)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let foreign = hotel.book_in(hotel.property, 1, None, Some("non_resident")).await;
    let local = hotel.book_in(hotel.property, 1, None, Some("resident")).await;
    let anyone = hotel.book(None).await;

    let mut levied_lines = Vec::new();
    for stay in [&foreign, &local, &anyone] {
        let line = hotel.post_for_stay(stay.rooms[0], hotel.charge(Source::Room, "room", 10_000, "LKR")).await.unwrap();
        let direct = hotel.post(stay.guests[0], hotel.charge(Source::Manual, "misc", 10_000, "LKR")).await.unwrap();
        let levied: i64 = sqlx::query_scalar(
            "select count(*) from folio_line_tax where line_id = any(array[$1, $2]) and code = 'NRL'",
        )
        .bind(line.id)
        .bind(direct.id)
        .fetch_one(&mut *hotel.tx().await)
        .await
        .unwrap();
        levied_lines.push(levied);
    }

    assert_eq!(levied_lines, vec![2, 0, 0], "only the non-resident plan's stay pays the levy, posted either way");
}
