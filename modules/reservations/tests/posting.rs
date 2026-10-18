//! Posting a business date's room and meal-plan charges (`folio::post_business_date_charges`), and the nights
//! check-out posts (`folio::post_unposted_nights`), on stays booked and checked in through this crate.

mod common;

use common::{Hotel, Plans, new_account, new_guest};
use folio::PostReport;
use rates::{MealPlan, RatePlan};
use reservations::{CreatedReservation, NewReservationRoom};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::Date;
use uuid::Uuid;

/// A posted line: `(folio kind, category, description, total, currency, posting key)`.
type Line = (String, String, String, i64, String, Option<String>);

impl Hotel {
    /// Books `rooms` for a new guest and checks every stay in (they all arrive today), committed.
    async fn checked_in(&self, account: Option<Uuid>, rooms: Vec<NewReservationRoom>) -> CreatedReservation {
        let booker = self.guest(new_guest("Ada", "Silva")).await;
        let booked = self.try_book_for_account(&booker, account, rooms).await.unwrap();
        for stay in &booked.rooms {
            let mut tx = self.tx().await;
            reservations::check_in(&mut tx, self.tenant, self.user, self.property, stay.id, stay.version, None)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
        booked
    }

    /// One stay on `plan` with `meal_plan`, from `business date + from` to `+ to`.
    fn stay(&self, plan: &RatePlan, meal_plan: MealPlan, from: i64, to: i64) -> NewReservationRoom {
        NewReservationRoom { meal_plan, ..self.room(self.deluxe.id, plan, from, to) }
    }

    async fn post(&self, date: Date) -> PostReport {
        folio::post_business_date_charges(&self.pool, self.tenant, Some(self.user), self.property, date).await.unwrap()
    }

    async fn check_out(&self, stay: Uuid) {
        let version: i32 = sqlx::query_scalar("select version from reservation_room where id = $1")
            .bind(stay)
            .fetch_one(&mut *self.tx().await)
            .await
            .unwrap();
        let mut tx = self.tx().await;
        reservations::check_out(&mut tx, self.tenant, self.user, self.property, stay, version).await.unwrap();
        tx.commit().await.unwrap();
    }

    /// Moves the business date to `business date + days`, as the night audit would.
    async fn move_business_date(&self, days: i64) {
        let mut tx = self.tx().await;
        sqlx::query("update property set business_date = $2 where id = $1")
            .bind(self.property)
            .bind(self.day(days))
            .execute(&mut *tx)
            .await
            .unwrap();
        rooms::extend_window(&mut tx, self.property).await.unwrap();
        tx.commit().await.unwrap();
    }

    /// Every line of the property, in posting order.
    async fn lines(&self) -> Vec<Line> {
        sqlx::query_as(
            "select f.kind, l.category, l.description, l.total, l.currency::text, l.posting_key
             from folio_line l join folio f on f.id = l.folio_id
             where f.property_id = $1
             order by l.id",
        )
        .bind(self.property)
        .fetch_all(&mut *self.tx().await)
        .await
        .unwrap()
    }

    /// The posting keys of every line, sorted.
    async fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.lines().await.into_iter().filter_map(|line| line.5).collect();
        keys.sort();
        keys
    }
}

fn room_key(stay: Uuid, date: Date) -> String {
    format!("room:{stay}:{date}")
}

fn meal_key(stay: Uuid, date: Date) -> String {
    format!("meal_plan:{stay}:{date}")
}

fn sorted(mut keys: Vec<String>) -> Vec<String> {
    keys.sort();
    keys
}

async fn hotel(opts: PgConnectOptions, rooms: usize) -> (Hotel, Plans) {
    Hotel::for_booking(opts, rooms).await
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn posting_a_date_charges_each_in_house_stay_once(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = hotel(opts, 2).await;
    let booked = hotel
        .checked_in(None, vec![hotel.stay(&plans.bar, MealPlan::Bb, 0, 3), hotel.stay(&plans.bar, MealPlan::Ro, 0, 3)])
        .await;
    let (bb, ro) = (&booked.rooms[0], &booked.rooms[1]);
    let today = hotel.day(0);

    let first = hotel.post(today).await;
    let second = hotel.post(today).await;

    assert_eq!(first, PostReport { stays: 2, lines: 3, skipped_existing: 0, failures: vec![] });
    assert_eq!(second, PostReport { stays: 2, lines: 0, skipped_existing: 3, failures: vec![] });
    let mut lines = hotel.lines().await;
    lines.sort_by(|a, b| a.5.cmp(&b.5));
    let (bb_room, ro_room) = (bb.room_number.clone().unwrap(), ro.room_number.clone().unwrap());
    let mut expected = vec![
        ("meal_plan", format!("Meal plan BB — {today}"), 3_000, meal_key(bb.id, today)),
        ("room", format!("Room {bb_room} — {today}"), 10_000, room_key(bb.id, today)),
        ("room", format!("Room {ro_room} — {today}"), 10_000, room_key(ro.id, today)),
    ];
    expected.sort_by(|a, b| a.3.cmp(&b.3));
    assert_eq!(lines.len(), expected.len());
    for ((kind, category, description, total, currency, key), (want_category, want_description, price, want_key)) in
        lines.iter().zip(&expected)
    {
        assert_eq!((kind.as_str(), category.as_str(), currency.as_str()), ("guest", *want_category, "USD"));
        assert_eq!((description, key.as_ref()), (want_description, Some(want_key)));
        // Exclusive prices: the taxes come on top of the night's price.
        assert!(total > price, "{total} > {price}");
    }
    let audited: Vec<(Uuid, serde_json::Value)> =
        sqlx::query_as("select entity_id, data from audit_log where action = 'folio.business_date_posted' order by id")
            .fetch_all(&mut *hotel.tx().await)
            .await
            .unwrap();
    let entry = serde_json::json!({ "date": today, "stays": 2, "lines": 3, "failures": [] });
    let rerun = serde_json::json!({ "date": today, "stays": 2, "lines": 0, "failures": [] });
    assert_eq!(audited, vec![(hotel.property, entry), (hotel.property, rerun)]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn only_stays_in_house_on_the_date_are_posted(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = hotel(opts, 4).await;
    let staying = hotel.checked_in(None, vec![hotel.stay(&plans.bar, MealPlan::Ro, 0, 3)]).await.rooms[0].id;
    // Checked in, but departing on day 1: in house on day 0 only.
    let departing = hotel.checked_in(None, vec![hotel.stay(&plans.bar, MealPlan::Ro, 0, 1)]).await.rooms[0].id;
    let booker = hotel.guest(new_guest("Bo", "Perera")).await;
    // Booked for today but not checked in, and booked from tomorrow.
    hotel.try_book(&booker, vec![hotel.stay(&plans.bar, MealPlan::Ro, 0, 3)]).await.unwrap();
    hotel.try_book(&booker, vec![hotel.stay(&plans.bar, MealPlan::Ro, 1, 3)]).await.unwrap();

    let report = hotel.post(hotel.day(0)).await;
    assert_eq!((report.stays, report.lines), (2, 2));
    assert_eq!(hotel.keys().await, sorted(vec![room_key(staying, hotel.day(0)), room_key(departing, hotel.day(0))]));

    // Day 1: the departing stay is still checked in (a late check-out) but no longer in house.
    hotel.move_business_date(1).await;
    let report = hotel.post(hotel.day(1)).await;
    assert_eq!((report.stays, report.lines), (1, 1));
    assert!(hotel.keys().await.contains(&room_key(staying, hotel.day(1))));
    assert!(!hotel.keys().await.contains(&room_key(departing, hotel.day(1))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_checked_out_today_is_not_posted_for_today(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = hotel(opts, 1).await;
    let stay = hotel.checked_in(None, vec![hotel.stay(&plans.bar, MealPlan::Ro, 0, 3)]).await.rooms[0].id;
    hotel.post(hotel.day(0)).await;
    hotel.move_business_date(1).await;

    hotel.check_out(stay).await;
    let report = hotel.post(hotel.day(1)).await;

    assert_eq!(report, PostReport::default());
    assert_eq!(hotel.keys().await, vec![room_key(stay, hotel.day(0))]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn charges_follow_the_reservations_routing(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = hotel(opts, 1).await;
    let account = hotel.account(new_account("Acme")).await;
    let stay = hotel.checked_in(Some(account.id), vec![hotel.stay(&plans.bar, MealPlan::Bb, 0, 2)]).await.rooms[0].id;

    let report = hotel.post(hotel.day(0)).await;

    assert_eq!(report.lines, 2);
    let routed: Vec<(String, String)> =
        hotel.lines().await.into_iter().map(|(kind, category, ..)| (kind, category)).collect();
    assert_eq!(routed, vec![("account".into(), "room".into()), ("account".into(), "meal_plan".into())]);
    assert_eq!(hotel.keys().await, sorted(vec![room_key(stay, hotel.day(0)), meal_key(stay, hotel.day(0))]));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_inclusive_plan_posts_lines_totalling_the_night_price(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = hotel(opts, 1).await;
    let mut tx = hotel.tx().await;
    sqlx::query("update rate_plan set prices_include_tax = true where id = $1")
        .bind(plans.bar.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    hotel.checked_in(None, vec![hotel.stay(&plans.bar, MealPlan::Bb, 0, 2)]).await;

    hotel.post(hotel.day(0)).await;

    let nets: Vec<(String, i64, i64)> = sqlx::query_as("select category, net, total from folio_line order by id")
        .fetch_all(&mut *hotel.tx().await)
        .await
        .unwrap();
    assert_eq!(
        nets.iter().map(|(category, _, total)| (category.as_str(), *total)).collect::<Vec<_>>(),
        vec![("room", 10_000), ("meal_plan", 3_000)]
    );
    assert!(nets.iter().all(|(_, net, total)| net < total), "taxes are carved out of the price: {nets:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_without_a_rate_is_reported_and_the_others_post(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = hotel(opts, 2).await;
    let euro = hotel.priced_plan(hotel.rate_plan("EUR", "EUR", &[hotel.deluxe.id]), 0, 40, 9_000).await;
    let in_euros = hotel.checked_in(None, vec![hotel.stay(&euro, MealPlan::Ro, 0, 2)]).await.rooms[0].id;
    let in_dollars = hotel.checked_in(None, vec![hotel.stay(&plans.bar, MealPlan::Ro, 0, 2)]).await.rooms[0].id;
    let today = hotel.day(0);

    let report = hotel.post(today).await;

    let error = format!("No exchange rate for EUR on {today}");
    assert_eq!(
        report,
        PostReport { stays: 1, lines: 1, skipped_existing: 0, failures: vec![(in_euros, error.clone())] }
    );
    assert_eq!(hotel.keys().await, vec![room_key(in_dollars, today)]);
    let audited: Vec<serde_json::Value> =
        sqlx::query_scalar("select data from audit_log where action = 'folio.business_date_posted'")
            .fetch_all(&mut *hotel.tx().await)
            .await
            .unwrap();
    let failures = serde_json::json!([{ "reservation_room": in_euros, "error": error }]);
    assert_eq!(audited, vec![serde_json::json!({ "date": today, "stays": 1, "lines": 1, "failures": failures })]);

    // Once the rate exists, a rerun posts the stay it missed and skips the other.
    let mut tx = hotel.tx().await;
    sqlx::query(
        "insert into fx_rate (tenant_id, property_id, date, currency, rate_to_base, source, source_date)
         values ($1, $2, $3, 'EUR', 330, 'manual', $3)",
    )
    .bind(hotel.tenant.0)
    .bind(hotel.property)
    .bind(today)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let rerun = hotel.post(today).await;
    assert_eq!(rerun, PostReport { stays: 2, lines: 1, skipped_existing: 1, failures: vec![] });
    assert_eq!(hotel.keys().await, sorted(vec![room_key(in_dollars, today), room_key(in_euros, today)]));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_run_stopped_between_stays_resumes_cleanly(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = hotel(opts, 3).await;
    let room = || hotel.stay(&plans.bar, MealPlan::Bb, 0, 2);
    let mut stays: Vec<Uuid> =
        hotel.checked_in(None, vec![room(), room(), room()]).await.rooms.iter().map(|stay| stay.id).collect();
    stays.sort();
    let today = hotel.day(0);
    // What a run stopped after its first stay (in id order) leaves: that stay's night posted and committed.
    let mut tx = hotel.tx().await;
    let posted = folio::post_unposted_nights(&mut tx, hotel.tenant, None, stays[0], hotel.day(1)).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(posted, 2);

    let report = hotel.post(today).await;

    assert_eq!(report, PostReport { stays: 3, lines: 4, skipped_existing: 2, failures: vec![] });
    let expected = stays.iter().flat_map(|stay| [room_key(*stay, today), meal_key(*stay, today)]).collect();
    assert_eq!(hotel.keys().await, sorted(expected));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn check_out_posts_the_missing_nights_exactly_once(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = hotel(opts, 1).await;
    let stay = hotel.checked_in(None, vec![hotel.stay(&plans.bar, MealPlan::Bb, 0, 3)]).await.rooms[0].id;
    hotel.post(hotel.day(0)).await;
    // No audit ran on days 1 and 2; the guest leaves on the departure date.
    hotel.move_business_date(3).await;

    hotel.check_out(stay).await;
    let report = hotel.post(hotel.day(3)).await;

    let expected = (0..3).flat_map(|day| [room_key(stay, hotel.day(day)), meal_key(stay, hotel.day(day))]).collect();
    assert_eq!(hotel.keys().await, sorted(expected));
    assert_eq!(hotel.lines().await.len(), 6, "each night once");
    assert_eq!(report, PostReport::default());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_early_check_out_posts_the_nights_up_to_today(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = hotel(opts, 1).await;
    let stay = hotel.checked_in(None, vec![hotel.stay(&plans.bar, MealPlan::Ro, 0, 5)]).await.rooms[0].id;
    hotel.move_business_date(2).await;

    hotel.check_out(stay).await;

    // The stay shrinks to days 0 and 1; tonight and later were never stayed.
    assert_eq!(hotel.keys().await, sorted(vec![room_key(stay, hotel.day(0)), room_key(stay, hotel.day(1))]));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_check_out_on_the_arrival_day_posts_the_night_it_keeps(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = hotel(opts, 1).await;
    let stay = hotel.checked_in(None, vec![hotel.stay(&plans.bar, MealPlan::Ro, 0, 3)]).await.rooms[0].id;

    hotel.check_out(stay).await;

    assert_eq!(hotel.keys().await, vec![room_key(stay, hotel.day(0))]);
    // Posting its nights again adds nothing.
    let mut tx = hotel.tx().await;
    let posted = folio::post_unposted_nights(&mut tx, hotel.tenant, None, stay, hotel.day(30)).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(posted, 0);
}

/// A stay always has a night (`reservation_room_stay_check` refuses an empty stay), so a day use (arrival equal to
/// departure) is a cut-off at the arrival date: nothing before it to post.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn no_nights_before_the_cut_off_posts_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = hotel(opts, 1).await;
    let stay = hotel.checked_in(None, vec![hotel.stay(&plans.bar, MealPlan::Bb, 0, 2)]).await.rooms[0].id;

    let mut tx = hotel.tx().await;
    let posted = folio::post_unposted_nights(&mut tx, hotel.tenant, None, stay, hotel.day(0)).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(posted, 0);
    assert_eq!(hotel.lines().await, vec![]);
}

/// The total of the lines posted under `keys` and of their reversals.
async fn net_of(hotel: &Hotel, keys: &[String]) -> (usize, i64) {
    sqlx::query_as(
        "select count(*)::int8, coalesce(sum(total), 0)::int8 from folio_line
         where posting_key = any($1) or reverses in (select id from folio_line where posting_key = any($1))",
    )
    .bind(keys)
    .fetch_one(&mut *hotel.tx().await)
    .await
    .map(|(count, total): (i64, i64)| (usize::try_from(count).unwrap(), total))
    .unwrap()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_early_check_out_reverses_tonights_posted_charges(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = hotel(opts, 1).await;
    let stay = hotel.checked_in(None, vec![hotel.stay(&plans.bar, MealPlan::Bb, 0, 3)]).await.rooms[0].id;
    hotel.post(hotel.day(0)).await;
    hotel.move_business_date(1).await;
    // Tonight's charges are posted, then the guest leaves early today: tonight is released.
    hotel.post(hotel.day(1)).await;

    hotel.check_out(stay).await;

    let tonight = [room_key(stay, hotel.day(1)), meal_key(stay, hotel.day(1))];
    assert_eq!(net_of(&hotel, &tonight).await, (4, 0), "both of tonight's lines and their reversals net to zero");
    let reasons: Vec<Option<String>> =
        sqlx::query_scalar("select reason from folio_line where reverses is not null order by id")
            .fetch_all(&mut *hotel.tx().await)
            .await
            .unwrap();
    assert_eq!(reasons, vec![Some("Night released at early check-out".to_owned()); 2]);
    let last_night = [room_key(stay, hotel.day(0)), meal_key(stay, hotel.day(0))];
    let (count, total) = net_of(&hotel, &last_night).await;
    assert_eq!(count, 2, "the night stayed is not reversed");
    assert!(total > 0);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_released_night_on_an_invoiced_folio_refuses_the_check_out(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = hotel(opts, 1).await;
    let stay = hotel.checked_in(None, vec![hotel.stay(&plans.bar, MealPlan::Ro, 0, 3)]).await.rooms[0].id;
    hotel.post(hotel.day(0)).await;
    hotel.move_business_date(1).await;
    hotel.post(hotel.day(1)).await;
    let mut tx = hotel.tx().await;
    sqlx::query("update folio set status = 'invoiced' where reservation_room_id = $1")
        .bind(stay)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let version: i32 = sqlx::query_scalar("select version from reservation_room where id = $1")
        .bind(stay)
        .fetch_one(&mut *hotel.tx().await)
        .await
        .unwrap();
    let mut tx = hotel.tx().await;
    let refused = reservations::check_out(&mut tx, hotel.tenant, hotel.user, hotel.property, stay, version).await;

    assert!(matches!(refused, Err(reservations::ReservationsError::Conflict(_))), "{refused:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_longer_than_31_nights_is_posted_for_a_date_past_its_31st(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = hotel(opts, 1).await;
    let stay = hotel.checked_in(None, vec![hotel.stay(&plans.bar, MealPlan::Ro, 0, 35)]).await.rooms[0].id;
    // Arrived 33 days before the date: outside the short-stay arrival range, found by the long-stay branch.
    hotel.move_business_date(33).await;

    let report = hotel.post(hotel.day(33)).await;

    assert_eq!((report.stays, report.lines, report.failures.len()), (1, 1, 0));
    assert_eq!(hotel.keys().await, vec![room_key(stay, hotel.day(33))]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_early_check_out_reverses_a_released_night_routed_to_the_account(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = hotel(opts, 1).await;
    let account = hotel.account(new_account("Acme")).await;
    let booked = hotel.checked_in(Some(account.id), vec![hotel.stay(&plans.bar, MealPlan::Bb, 0, 3)]).await;
    let stay = booked.rooms[0].id;
    hotel.post(hotel.day(0)).await;
    hotel.move_business_date(1).await;
    hotel.post(hotel.day(1)).await;
    // The routing changes mid-stay: the stay's lines are now spread over folios the check-out did not post to.
    let mut tx = hotel.tx().await;
    folio::set_routing(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, booked.id, &[]).await.unwrap();
    tx.commit().await.unwrap();

    hotel.check_out(stay).await;

    let tonight = [room_key(stay, hotel.day(1)), meal_key(stay, hotel.day(1))];
    assert_eq!(net_of(&hotel, &tonight).await, (4, 0), "tonight's account lines and their reversals net to zero");
    let reversed_on: Vec<String> = sqlx::query_scalar(
        "select f.kind from folio_line l join folio f on f.id = l.folio_id where l.reverses is not null order by l.id",
    )
    .fetch_all(&mut *hotel.tx().await)
    .await
    .unwrap();
    assert_eq!(reversed_on, vec!["account".to_owned(); 2], "reversed where they were posted");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_checked_in_stay_is_invoiced_only_once_its_nights_are_posted(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = hotel(opts, 1).await;
    let stay = hotel.checked_in(None, vec![hotel.stay(&plans.bar, MealPlan::Ro, 0, 2)]).await.rooms[0].id;
    let mut tx = hotel.tx().await;
    let guest = folio::ensure_guest_folio(&mut tx, hotel.tenant, hotel.property, stay).await.unwrap();
    tx.commit().await.unwrap();
    let issue = || async {
        let mut tx = hotel.tx().await;
        let issued = folio::issue_invoice(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, guest).await;
        tx.commit().await.unwrap();
        issued
    };

    let refused = issue().await;
    assert!(
        matches!(&refused, Err(folio::FolioError::Conflict(message))
            if message == "Post the stay's nights first: post tonight's charges or check out"),
        "{refused:?}"
    );

    hotel.post(hotel.day(0)).await;
    let query = folio::FolioQuery::Folio { property: hotel.property, folio: guest };
    let view = folio::folios_for(&mut hotel.tx().await, query).await.unwrap().pop().unwrap();
    let mut tx = hotel.tx().await;
    for balance in view.balances {
        let method = folio::Method::Cash;
        folio::take_payment(
            &mut tx,
            hotel.tenant,
            None,
            hotel.property,
            guest,
            method,
            balance.amount,
            &balance.currency,
            None,
        )
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();

    let invoice = issue().await.unwrap();
    assert_eq!(invoice.folio_id, guest, "tonight posted, the stay so far is invoiced");
}
