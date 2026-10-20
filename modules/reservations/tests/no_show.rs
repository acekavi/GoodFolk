mod common;

use common::{Hotel, Plans, new_guest};
use rates::{Penalty, PenaltyKind};
use reservations::{CancellationTerms, ChangedStay, CreatedReservation, Guest, NoShowRoom, ReservationsError};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::Date;
use uuid::Uuid;

impl Hotel {
    async fn try_no_show(
        &self,
        actor: Option<db::UserId>,
        id: Uuid,
        version: i32,
    ) -> Result<NoShowRoom, ReservationsError> {
        let mut tx = self.tx().await;
        let marked = reservations::mark_no_show(&mut tx, self.tenant, actor, self.property, id, version).await?;
        tx.commit().await.unwrap();
        Ok(marked)
    }

    async fn try_delay(&self, id: Uuid, version: i32) -> Result<ChangedStay, ReservationsError> {
        let mut tx = self.tx().await;
        let delayed =
            reservations::delay_arrival(&mut tx, self.tenant, Some(self.user), self.property, id, version).await?;
        tx.commit().await.unwrap();
        Ok(delayed)
    }

    async fn try_extend(&self, id: Uuid, version: i32) -> Result<ChangedStay, ReservationsError> {
        let mut tx = self.tx().await;
        let extended =
            reservations::extend_stay(&mut tx, self.tenant, Some(self.user), self.property, id, version).await?;
        tx.commit().await.unwrap();
        Ok(extended)
    }

    /// Books one room of `plan` for `[business date + from, business date + to)`, room only.
    async fn stay(&self, booker: &Guest, plan: &rates::RatePlan, from: i64, to: i64) -> CreatedReservation {
        self.try_book(booker, vec![self.room(self.deluxe.id, plan, from, to)]).await.unwrap()
    }

    /// Checks `id` in at `version`, committed, and returns its new version.
    async fn checked_in(&self, id: Uuid, version: i32) -> i32 {
        let mut tx = self.tx().await;
        let checked_in =
            reservations::check_in(&mut tx, self.tenant, self.user, self.property, id, version, None).await.unwrap();
        tx.commit().await.unwrap();
        checked_in.version
    }

    /// Moves the business date to `business date + days` and the counter window with it, as the night audit will.
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

    /// Runs `sql` with `$1` = `id` and `$2` = `value`, committed.
    async fn set<T>(&self, sql: &'static str, id: Uuid, value: T)
    where
        T: for<'q> sqlx::Encode<'q, sqlx::Postgres> + sqlx::Type<sqlx::Postgres> + Send,
    {
        let mut tx = self.tx().await;
        sqlx::query(sql).bind(id).bind(value).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    }

    /// `room`'s status, stay and `cancelled_business_date`.
    async fn row(&self, room: Uuid) -> (String, Date, Date, Option<Date>, Option<Uuid>) {
        sqlx::query_as(
            "select status, lower(stay), upper(stay), cancelled_business_date, room_id
             from reservation_room where id = $1",
        )
        .bind(room)
        .fetch_one(&mut *self.tx().await)
        .await
        .unwrap()
    }

    /// `room`'s nights as `(date, room_amount, meal_amount)`, oldest first.
    async fn nights(&self, room: Uuid) -> Vec<(Date, i64, i64)> {
        sqlx::query_as(
            "select date, room_amount, meal_amount from reservation_night where reservation_room_id = $1 order by date",
        )
        .bind(room)
        .fetch_all(&mut *self.tx().await)
        .await
        .unwrap()
    }

    /// The actor and data of every `action` entry, oldest first.
    async fn audited(&self, action: &str) -> Vec<(Option<Uuid>, serde_json::Value)> {
        sqlx::query_as("select actor_user_id, data from audit_log where action = $1 order by id")
            .bind(action)
            .fetch_all(&mut *self.tx().await)
            .await
            .unwrap()
    }

    /// `reservation`'s deposit folio.
    async fn deposit(&self, reservation: Uuid) -> folio::FolioView {
        let mut tx = self.tx().await;
        let deposit = folio::ensure_deposit_folio(&mut tx, self.tenant, self.property, reservation).await.unwrap();
        let query = folio::FolioQuery::Folio { property: self.property, folio: deposit };
        folio::folios_for(&mut tx, query).await.unwrap().pop().expect("the deposit folio")
    }
}

fn conflict<T: std::fmt::Debug>(result: Result<T, ReservationsError>) -> String {
    match result {
        Err(ReservationsError::Conflict(message)) => message,
        other => panic!("expected Conflict, got {other:?}"),
    }
}

/// Terms whose no-show costs `penalty`.
fn no_show_terms(kind: PenaltyKind, value: i64) -> serde_json::Value {
    serde_json::to_value(CancellationTerms { rules: vec![], no_show: Penalty { kind, value } }).unwrap()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_no_show_releases_its_nights_and_posts_its_fee_beside_the_deposit(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.stay(&booker, &plans.bar, 0, 3).await;
    let room = booked.rooms[0].id;
    let mut tx = hotel.tx().await;
    let deposit = folio::ensure_deposit_folio(&mut tx, hotel.tenant, hotel.property, booked.id).await.unwrap();
    folio::take_payment(
        &mut tx,
        hotel.tenant,
        Some(hotel.user),
        hotel.property,
        deposit,
        folio::Method::Cash,
        5_000,
        "LKR",
        None,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let marked = hotel.try_no_show(Some(hotel.user), room, 1).await.unwrap();

    let expected =
        NoShowRoom { id: room, reservation_id: booked.id, version: 2, penalty: 30_000, currency: "USD".into() };
    assert_eq!(marked, expected, "BAR's no-show costs the whole stay");
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 4).await, [0; 4]);
    assert_eq!(hotel.drift().await, vec![]);
    let (status, .., cancelled_on, _) = hotel.row(room).await;
    assert_eq!((status.as_str(), cancelled_on), ("no_show", Some(hotel.day(0))));
    let view = hotel.deposit(booked.id).await;
    let [fee] = &view.lines[..] else { panic!("one line: {:?}", view.lines) };
    assert_eq!((fee.line.source, fee.line.category.as_str()), (folio::Source::NoShowFee, "fee"));
    assert_eq!(fee.line.posting_key, Some(format!("no_show:{room}")));
    assert_eq!((fee.line.net, fee.line.currency.as_str()), (30_000, "USD"), "exclusive: the plan's prices are");
    assert!(!fee.taxes.is_empty(), "a penalty is taxed");
    assert_eq!(view.payments.len(), 1, "the deposit stays on the deposit folio");
    assert_eq!(
        hotel.audited("reservation_room.no_show").await,
        vec![(
            Some(hotel.user.0),
            serde_json::json!({ "reservation_id": booked.id, "penalty": 30_000, "currency": "USD", "mode": "manual" })
        )]
    );

    let mut tx = hotel.tx().await;
    folio::post_no_show_fee(&mut tx, hotel.tenant, None, hotel.property, room, 30_000, "USD", false).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(hotel.deposit(booked.id).await.lines.len(), 1, "a replay posts nothing more");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_close_s_no_show_of_a_tax_inclusive_plan_totals_exactly_the_penalty(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    hotel.set("update rate_plan set prices_include_tax = $2 where id = $1", plans.bar.id, true).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.stay(&booker, &plans.bar, 0, 2).await;

    assert_eq!(hotel.try_no_show(None, booked.rooms[0].id, 1).await.unwrap().penalty, 20_000);

    let lines = hotel.deposit(booked.id).await.lines;
    let [fee] = &lines[..] else { panic!("one line: {lines:?}") };
    assert_eq!(fee.line.total, 20_000, "the penalty already includes its taxes");
    assert!(fee.line.net < 20_000 && !fee.taxes.is_empty(), "{fee:?}");
    let [(actor, data)] = &hotel.audited("reservation_room.no_show").await[..] else { panic!("one entry") };
    assert_eq!((actor, &data["mode"]), (&None, &serde_json::json!("auto")));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_no_show_costs_what_the_booked_terms_say(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 3).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let one_night = hotel.stay(&booker, &plans.bar, 0, 3).await.rooms[0].id;
    let half = hotel.stay(&booker, &plans.bar, 0, 3).await.rooms[0].id;
    let free = hotel.stay(&booker, &plans.rack, 0, 3).await;
    let sql = "update reservation_room set cancellation_terms = $2 where id = $1";
    hotel.set(sql, one_night, no_show_terms(PenaltyKind::Nights, 1)).await;
    hotel.set(sql, half, no_show_terms(PenaltyKind::Percent, 5_000)).await;

    assert_eq!(hotel.try_no_show(Some(hotel.user), one_night, 1).await.unwrap().penalty, 10_000, "the first night");
    assert_eq!(hotel.try_no_show(Some(hotel.user), half, 1).await.unwrap().penalty, 15_000, "half of 30000");
    assert_eq!(hotel.try_no_show(Some(hotel.user), free.rooms[0].id, 1).await.unwrap().penalty, 0, "RACK has no terms");
    assert!(hotel.deposit(free.id).await.lines.is_empty(), "nothing to post");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn only_a_confirmed_room_due_by_the_business_date_is_a_no_show(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let in_house = hotel.stay(&booker, &plans.bar, 0, 2).await.rooms[0].id;
    let version = hotel.checked_in(in_house, 1).await;
    let tomorrow = hotel.stay(&booker, &plans.bar, 1, 3).await.rooms[0].id;

    assert_eq!(
        conflict(hotel.try_no_show(Some(hotel.user), in_house, version).await),
        "a checked-in room can't be marked a no-show"
    );
    let early = conflict(hotel.try_no_show(Some(hotel.user), tomorrow, 1).await);
    assert!(early.contains(&hotel.day(1).to_string()), "{early}");
    let stale = hotel.try_no_show(Some(hotel.user), tomorrow, 2).await;
    assert!(matches!(stale, Err(ReservationsError::VersionMismatch("reservation room"))), "{stale:?}");
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 3).await, [1, 2, 1]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_delayed_arrival_drops_only_the_business_date_s_night(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.stay(&booker, &plans.bar, 0, 3).await;
    let room = booked.rooms[0].id;
    let before = hotel.row(room).await;

    let delayed = hotel.try_delay(room, 1).await.unwrap();

    let expected = ChangedStay {
        id: room,
        reservation_id: booked.id,
        version: 2,
        check_in: hotel.day(1),
        check_out: hotel.day(3),
        total: 20_000,
        currency: "USD".into(),
    };
    assert_eq!(delayed, expected);
    let nights: Vec<Date> = hotel.nights(room).await.into_iter().map(|(date, ..)| date).collect();
    assert_eq!(nights, [hotel.day(1), hotel.day(2)]);
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 4).await, [0, 1, 1, 0]);
    assert_eq!(hotel.drift().await, vec![]);
    let after = hotel.row(room).await;
    assert_eq!((after.1, after.4), (hotel.day(1), before.4), "the room stays assigned");
    assert_eq!(
        hotel.audited("reservation_room.arrival_delayed").await,
        vec![(
            Some(hotel.user.0),
            serde_json::json!({ "reservation_id": booked.id, "from": hotel.day(0), "to": hotel.day(1), "mode": "manual" })
        )]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_one_night_stay_is_not_delayed_but_a_tentative_one_is(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let one_night = hotel.stay(&booker, &plans.bar, 0, 1).await.rooms[0].id;
    let tentative = hotel.stay(&booker, &plans.bar, 0, 2).await.rooms[0].id;
    hotel.set("update reservation_room set status = $2 where id = $1", tentative, "tentative").await;

    assert_eq!(conflict(hotel.try_delay(one_night, 1).await), "Change the dates with Modify");
    let delayed = hotel.try_delay(tentative, 1).await.unwrap();

    assert_eq!((delayed.check_in, delayed.check_out), (hotel.day(1), hotel.day(2)));
    assert_eq!(hotel.row(tentative).await.0, "tentative");
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 2).await, [1, 1]);
}

/// Books `[business date, business date + 1)` in room 101, checks it in, and moves the business date to its
/// departure: a departure still in house at the close. Returns the reservation and the stay's version.
async fn due_out(hotel: &Hotel, plans: &Plans) -> (CreatedReservation, i32) {
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.stay(&booker, &plans.bar, 0, 1).await;
    assert_eq!(booked.rooms[0].room_number.as_deref(), Some("101"));
    let version = hotel.checked_in(booked.rooms[0].id, 1).await;
    hotel.move_business_date(1).await;
    (booked, version)
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_extension_adds_tonight_at_the_last_night_s_amounts(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let (booked, version) = due_out(&hotel, &plans).await;
    let room = booked.rooms[0].id;
    let mut tx = hotel.tx().await;
    sqlx::query("update reservation_night set room_amount = 12_345, meal_amount = 678 where reservation_room_id = $1")
        .bind(room)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let extended = hotel.try_extend(room, version).await.unwrap();

    let expected = ChangedStay {
        id: room,
        reservation_id: booked.id,
        version: version + 1,
        check_in: hotel.day(0),
        check_out: hotel.day(2),
        total: 2 * (12_345 + 678),
        currency: "USD".into(),
    };
    assert_eq!(extended, expected);
    assert_eq!(hotel.nights(room).await, [(hotel.day(0), 12_345, 678), (hotel.day(1), 12_345, 678)]);
    assert_eq!(hotel.sold(hotel.deluxe.id, 1, 3).await, [1, 0]);
    assert_eq!(hotel.drift().await, vec![]);
    let [(_, data)] = &hotel.audited("reservation_room.extended").await[..] else { panic!("one entry") };
    assert_eq!(data["mode"], "manual");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn no_extension_onto_a_full_night(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Bo", "Perera")).await;
    hotel.stay(&booker, &plans.bar, 1, 2).await;
    let (booked, version) = due_out(&hotel, &plans).await;

    let full = conflict(hotel.try_extend(booked.rooms[0].id, version).await);

    assert_eq!(full, format!("No room of this type is free on {}", hotel.day(1)));
    assert_eq!(hotel.sold(hotel.deluxe.id, 1, 2).await, [1]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn no_extension_into_a_room_taken_or_blocked_tonight(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let (booked, version) = due_out(&hotel, &plans).await;
    let stay = booked.rooms[0].id;
    let room = booked.rooms[0].room_id.unwrap();
    let booker = hotel.guest(new_guest("Bo", "Perera")).await;
    // Arriving tonight in room 101, on a plan without terms: cancelling it later posts no fee.
    let next = hotel.try_book_unassigned(&booker, vec![hotel.room(hotel.deluxe.id, &plans.rack, 1, 2)]).await.unwrap();
    let next_room = &next.rooms[0];
    let mut tx = hotel.tx().await;
    let assigned = reservations::assign_room(
        &mut tx,
        hotel.tenant,
        hotel.user,
        hotel.property,
        next_room.id,
        next_room.version,
        room,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let taken = conflict(hotel.try_extend(stay, version).await);
    assert_eq!(taken, format!("room 101 is taken by {} on those nights", next.confirmation_no));

    let mut tx = hotel.tx().await;
    reservations::cancel_room(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, next_room.id, assigned.version)
        .await
        .unwrap();
    let leak =
        rooms::NewBlockReason { code: "LEAK".into(), label: "Leak".into(), default_kind: rooms::BlockKind::OutOfOrder };
    let reason = rooms::create_block_reason(&mut tx, hotel.tenant, hotel.user, hotel.property, leak).await.unwrap();
    // Straight into the table: the block commands refuse a room a stay holds, which this one is about to.
    sqlx::query(
        "insert into room_block (id, tenant_id, property_id, room_id, period, kind, reason_id, note, created_by)
         values ($1, $2, $3, $4, daterange($5, $6), 'out_of_order', $7, '', $8)",
    )
    .bind(Uuid::now_v7())
    .bind(hotel.tenant.0)
    .bind(hotel.property)
    .bind(room)
    .bind(hotel.day(1))
    .bind(hotel.day(3))
    .bind(reason.id)
    .bind(hotel.user.0)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let blocked = conflict(hotel.try_extend(stay, version).await);
    assert_eq!(blocked, format!("room 101 is blocked from {} to {}", hotel.day(1), hotel.day(3)));
    assert_eq!(hotel.row(stay).await.2, hotel.day(1), "still leaving today");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn only_a_stay_due_out_today_is_extended(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let room = hotel.stay(&booker, &plans.bar, 0, 2).await.rooms[0].id;
    let version = hotel.checked_in(room, 1).await;

    let early = conflict(hotel.try_extend(room, version).await);

    assert!(early.contains(&hotel.day(2).to_string()), "{early}");
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 3).await, [1, 1, 0]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn bookings_and_cancellations_keep_the_business_date_they_happened_on(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    // RACK has no terms: cancelling posts no fee, which would need a rate on the new business date.
    let first = hotel.stay(&booker, &plans.rack, 2, 4).await;
    hotel.move_business_date(1).await;
    let second = hotel.stay(&booker, &plans.rack, 2, 4).await;
    let mut tx = hotel.tx().await;
    reservations::cancel_room(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, first.rooms[0].id, 1)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let booked_on: Vec<Date> =
        sqlx::query_scalar("select booked_business_date from reservation where id = any($1) order by confirmation_no")
            .bind(vec![first.id, second.id])
            .fetch_all(&mut *hotel.tx().await)
            .await
            .unwrap();
    assert_eq!(booked_on, [hotel.day(0), hotel.day(1)]);
    assert_eq!(hotel.row(first.rooms[0].id).await.3, Some(hotel.day(1)));
    assert_eq!(hotel.row(second.rooms[0].id).await.3, None);
}
