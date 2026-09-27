mod common;

use common::{Hotel, new_guest};
use rates::{MealPlan, QuoteRequest, Residency, RestrictionChange};
use reservations::{
    CreatedReservation, NewGuest, NewReservation, NewReservationRoom, ReservationsError, Source, Total,
};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

/// Parallel bookings racing for the last free room.
const RACERS: usize = 20;

fn invalid(result: Result<CreatedReservation, ReservationsError>) -> String {
    match result {
        Err(ReservationsError::Invalid(message)) => message,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_booking_takes_the_next_confirmation_number_fixes_its_prices_and_sells_its_nights(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let breakfast = NewReservationRoom { meal_plan: MealPlan::Bb, ..hotel.room(hotel.deluxe.id, &plans.bar, 2, 5) };

    let first = hotel.try_book(&booker, vec![breakfast.clone()]).await.unwrap();
    let second = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.rack, 3, 4)]).await.unwrap();

    assert_eq!(first.confirmation_no, "GAL-000001");
    assert_eq!(second.confirmation_no, "GAL-000002");
    assert_eq!(first.version, 1);
    assert_eq!(first.rooms.len(), 1);
    let room = &first.rooms[0];
    assert_eq!(
        (room.room_type_id, room.rate_plan_id, room.meal_plan, room.check_in, room.check_out),
        (hotel.deluxe.id, plans.bar.id, MealPlan::Bb, hotel.day(2), hotel.day(5))
    );
    assert_eq!((room.adults, room.children, room.total, room.currency.as_str()), (2, 0, 3 * 13_000, "USD"));
    assert_eq!(first.totals, vec![Total { currency: "USD".into(), amount: 39_000 }]);

    let request = QuoteRequest {
        room_type_id: hotel.deluxe.id,
        rate_plan_id: plans.bar.id,
        meal_plan: MealPlan::Bb,
        check_in: hotel.day(2),
        check_out: hotel.day(5),
        adults: 2,
        children: 0,
        residency: Residency::NonResident,
    };
    let quote = rates::load_quote(&mut hotel.tx().await, hotel.property, &request).await.unwrap();
    let nights: Vec<(time::Date, i64, i64, String)> = sqlx::query_as(
        "select date, room_amount, meal_amount, currency::text from reservation_night
         where reservation_room_id = $1 order by date",
    )
    .bind(room.id)
    .fetch_all(&mut *hotel.tx().await)
    .await
    .unwrap();
    let quoted: Vec<_> = quote.nights.iter().map(|night| (night.date, night.room, night.meal, "USD".into())).collect();
    assert_eq!(nights, quoted, "the nights are the quote's");

    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 7).await, [0, 0, 1, 2, 1, 0, 0], "only the booked nights are sold");
    assert_eq!(hotel.sold(hotel.standard.id, 0, 7).await, [0; 7]);
    assert_eq!(hotel.drift().await, vec![]);

    let terms: Vec<(Uuid, Option<serde_json::Value>, String)> = sqlx::query_as(
        "select reservation_id, cancellation_terms, status from reservation_room order by reservation_id",
    )
    .fetch_all(&mut *hotel.tx().await)
    .await
    .unwrap();
    let flexible = serde_json::json!({
        "rules": [{ "days_before_arrival": 2, "penalty": { "kind": "nights", "value": 1 } }],
        "no_show": { "kind": "percent", "value": 10000 },
    });
    assert_eq!(
        terms,
        vec![(first.id, Some(flexible), "confirmed".into()), (second.id, None, "confirmed".into())],
        "the plan's policy is copied; RACK has none"
    );
    let audited: Vec<(String, Uuid)> =
        sqlx::query_as("select action, entity_id from audit_log where entity = 'reservation' order by entity_id")
            .fetch_all(&mut *hotel.tx().await)
            .await
            .unwrap();
    assert_eq!(audited, vec![("reservation.created".into(), first.id), ("reservation.created".into(), second.id)]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_booking_needing_more_rooms_than_are_free_writes_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let deluxe = hotel.room(hotel.deluxe.id, &plans.bar, 2, 4);
    let standard = hotel.room(hotel.standard.id, &plans.bar, 2, 4);

    let refused = hotel.try_book(&booker, vec![standard.clone(), deluxe.clone(), deluxe.clone()]).await;

    match refused {
        Err(ReservationsError::Conflict(message)) => {
            assert_eq!(message, format!("no DLX rooms left on {}", hotel.day(2)));
        }
        other => panic!("expected Conflict, got {other:?}"),
    }
    assert_eq!(hotel.confirmation_numbers().await, Vec::<String>::new());
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 5).await, [0; 5]);
    assert_eq!(hotel.sold(hotel.standard.id, 0, 5).await, [0; 5]);

    let booked = hotel.try_book(&booker, vec![standard, deluxe]).await.unwrap();

    assert_eq!(booked.confirmation_no, "GAL-000001", "the refused booking did not use a number");
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 5).await, [0, 0, 1, 1, 0]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_the_plan_restricts_is_refused_with_every_reason(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let mut tx = hotel.tx().await;
    let restrict = |from: i64, closed: Option<bool>, min_stay: Option<Option<i32>>| RestrictionChange {
        from: hotel.day(from),
        to: hotel.day(from + 1),
        weekdays: vec![],
        room_type_ids: vec![],
        closed,
        min_stay,
        max_stay: None,
        closed_to_arrival: None,
        closed_to_departure: None,
    };
    for change in [restrict(2, None, Some(Some(5))), restrict(3, Some(true), None)] {
        rates::set_restrictions(&mut tx, hotel.tenant, hotel.user, hotel.property, plans.bar.id, &change)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();

    let message = invalid(hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await);

    assert_eq!(
        message,
        format!("stays over {} are at least 5 nights; BAR is closed on {}", hotel.day(2), hotel.day(3))
    );
    assert_eq!(hotel.confirmation_numbers().await, Vec::<String>::new());
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 5).await, [0; 5]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_is_priced_for_its_primary_guest_s_residency(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let visitor = hotel.guest(new_guest("Ada", "Silva")).await;
    let local = hotel.guest(NewGuest { residency: Residency::Resident, ..new_guest("Nimal", "Perera") }).await;
    let fit_f = hotel.room(hotel.deluxe.id, &plans.fit_f, 2, 4);

    let for_local = invalid(hotel.try_book(&local, vec![fit_f.clone()]).await);
    let local_guest = NewReservationRoom { primary_guest_id: Some(local.id), ..fit_f.clone() };
    let local_in_visitor_s_booking = invalid(hotel.try_book(&visitor, vec![local_guest]).await);
    let visitor_in_local_booking = NewReservationRoom { primary_guest_id: Some(visitor.id), ..fit_f };
    let booked = hotel.try_book(&local, vec![visitor_in_local_booking]).await.unwrap();

    assert_eq!(for_local, "FIT-F is sold to non-residents only");
    assert_eq!(local_in_visitor_s_booking, "FIT-F is sold to non-residents only");
    assert_eq!(booked.confirmation_no, "GAL-000001");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn malformed_bookings_are_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let room = hotel.room(hotel.deluxe.id, &plans.bar, 2, 4);
    let book = |input: NewReservation| async {
        let mut tx = hotel.tx().await;
        reservations::create_reservation(&mut tx, hotel.tenant, hotel.user, hotel.property, input).await
    };
    let valid = NewReservation {
        booker_guest_id: booker.id,
        source: Source::FrontDesk,
        notes: String::new(),
        rooms: vec![room.clone()],
    };
    let (first, last) = (hotel.day(0), hotel.day(730));
    let window = format!("stays must arrive on or after {first} and leave by {last}");

    let cases = [
        (NewReservation { rooms: vec![], ..valid.clone() }, "a reservation has 1 to 10 rooms".to_string()),
        (NewReservation { rooms: vec![room.clone(); 11], ..valid.clone() }, "a reservation has 1 to 10 rooms".into()),
        (
            NewReservation { source: Source::Ibe, ..valid.clone() },
            "bookings made here come from the front desk, phone or email".into(),
        ),
        (NewReservation { notes: "x".repeat(2001), ..valid.clone() }, "notes are at most 2000 characters".into()),
        (NewReservation { booker_guest_id: Uuid::now_v7(), ..valid.clone() }, "no such guest".into()),
        (
            NewReservation {
                rooms: vec![NewReservationRoom { primary_guest_id: Some(Uuid::now_v7()), ..room.clone() }],
                ..valid.clone()
            },
            "no such guest".into(),
        ),
        (
            NewReservation { rooms: vec![hotel.room(hotel.deluxe.id, &plans.bar, -1, 2)], ..valid.clone() },
            window.clone(),
        ),
        (NewReservation { rooms: vec![hotel.room(hotel.deluxe.id, &plans.bar, 729, 731)], ..valid.clone() }, window),
        (
            NewReservation { rooms: vec![hotel.room(hotel.deluxe.id, &plans.bar, 3, 3)], ..valid.clone() },
            "check-out is after check-in".into(),
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(invalid(book(input.clone()).await), expected, "{input:?}");
    }
    let unsold = NewReservation { rooms: vec![hotel.room(hotel.standard.id, &plans.rack, 2, 4)], ..valid.clone() };
    let (night_2, night_3) = (hotel.day(2), hotel.day(3));
    assert_eq!(
        invalid(book(unsold).await),
        format!("RACK does not sell STD; no price for 2 adults on {night_2}; no price for 2 adults on {night_3}")
    );
    assert!(book(valid).await.is_ok(), "the unchanged request is fine");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn parallel_bookings_for_the_last_room_sell_it_once(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts.clone(), 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();

    let pool = db::testing::app_pool(opts, u32::try_from(RACERS).unwrap()).await;
    let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);
    // Every racer holds an open transaction before any of them books, so the bookings really overlap.
    let start = std::sync::Arc::new(tokio::sync::Barrier::new(RACERS));
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..RACERS {
        let (pool, start) = (pool.clone(), start.clone());
        let input = NewReservation {
            booker_guest_id: booker.id,
            source: Source::FrontDesk,
            notes: String::new(),
            rooms: vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 4)],
        };
        tasks.spawn(async move {
            let mut tx = db::begin(&pool, db::Scope::tenant(tenant)).await.unwrap();
            start.wait().await;
            let created = reservations::create_reservation(&mut tx, tenant, user, property, input).await?;
            tx.commit().await?;
            Ok::<_, ReservationsError>(created)
        });
    }
    let results = tasks.join_all().await;

    let booked: Vec<&CreatedReservation> = results.iter().filter_map(|result| result.as_ref().ok()).collect();
    let conflicts: Vec<String> = results
        .iter()
        .filter_map(|result| match result {
            Err(ReservationsError::Conflict(message)) => Some(message.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(booked.len(), 1, "{results:?}");
    assert_eq!(conflicts.len(), RACERS - 1, "{results:?}");
    assert!(conflicts.iter().all(|message| message == &format!("no DLX rooms left on {}", hotel.day(2))));
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 6).await, [0, 1, 2, 2, 1, 0], "both rooms sold on nights 2 and 3");
    assert_eq!(hotel.drift().await, vec![]);
    assert_eq!(hotel.confirmation_numbers().await, ["GAL-000001", "GAL-000002"]);
}
