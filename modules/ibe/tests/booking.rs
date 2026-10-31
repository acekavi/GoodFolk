//! Booking on the site: a confirmed booking-site reservation with new guests, its manage token, its confirmation email
//! queued, and the checks before anything is written: the price, the rooms left, occupancy and residency.
mod common;

use common::{Hotel, new_plan};
use ibe::{BookingError, BookingRequest};
use rates::{NewRatePlan, Residency};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct ReservationRow {
    source: String,
    guarantee: String,
    created_by: Option<Uuid>,
    booker_guest_id: Uuid,
    notes: String,
}

#[derive(sqlx::FromRow)]
struct RoomRow {
    status: String,
    cancellation_terms: Option<serde_json::Value>,
    deposit_terms: Option<serde_json::Value>,
    first_name: String,
    last_name: String,
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn booking_creates_confirmed_ibe_reservation(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts, 3).await;
    let mut request = hotel.request(&hotel.bar, Residency::Resident, &[(2, 0), (2, 1)], "Kamala@Example.com").await;
    request.rooms[1].occupant_name = Some("  Nimal  de Silva ".into());
    request.booker.arrival_time = Some(time::macros::time!(14:30));

    let booked = hotel.book(&request).await.unwrap();
    let again =
        hotel.book(&hotel.request(&hotel.bar, Residency::Resident, &[(1, 0)], "kamala@example.com").await).await;

    let mut tx = hotel.tx().await;
    let row: ReservationRow =
        sqlx::query_as("select source, guarantee, created_by, booker_guest_id, notes from reservation where id = $1")
            .bind(booked.reservation_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!((row.source.as_str(), row.guarantee.as_str(), row.created_by), ("ibe", "none", None));
    assert_eq!(row.notes, "Arrival time: 14:30\nA quiet room, please");
    assert!(booked.confirmation_no.starts_with("GAL-"), "{}", booked.confirmation_no);
    let rooms: Vec<RoomRow> = sqlx::query_as(
        "select rr.status, rr.cancellation_terms, rr.deposit_terms, g.first_name, g.last_name
         from reservation_room rr join guest g on g.id = rr.primary_guest_id
         where rr.reservation_id = $1 order by rr.adults desc, rr.children",
    )
    .bind(booked.reservation_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(rooms.len(), 2);
    assert!(rooms.iter().all(|room| room.status == "confirmed"));
    assert_eq!(
        rooms[0].cancellation_terms,
        Some(serde_json::json!({
            "rules": [{"days_before_arrival": 3, "penalty": {"kind": "nights", "value": 1}}],
            "no_show": {"kind": "nights", "value": 1},
        })),
        "the terms are snapshotted"
    );
    assert!(rooms[0].deposit_terms.is_some(), "the deposit rule is kept");
    let name = |room: &RoomRow| (room.first_name.clone(), room.last_name.clone());
    assert_eq!(name(&rooms[0]), ("Kamala".into(), "Perera".into()), "the booker stays in the first");
    assert_eq!(name(&rooms[1]), ("Nimal  de".into(), "Silva".into()), "the occupant has a profile");

    let booker: (String, String, String, String, String) =
        sqlx::query_as("select email, phone, country::text, residency, first_name from guest where id = $1")
            .bind(row.booker_guest_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(
        booker,
        ("kamala@example.com".into(), "+94 77 123 4567".into(), "LK".into(), "resident".into(), "Kamala".into())
    );
    let again = again.unwrap();
    let bookers: Vec<Uuid> = sqlx::query_scalar("select id from guest where email = 'kamala@example.com' order by id")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(bookers.len(), 2, "a second booking with the same email is a second guest");
    assert_ne!(again.reservation_id, booked.reservation_id);

    let (hash, expires_at): (Vec<u8>, OffsetDateTime) =
        sqlx::query_as("select sha256, expires_at from reservation_access_token where reservation_id = $1")
            .bind(booked.reservation_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(hash, ibe::token_hash(&booked.token));
    assert_eq!(booked.token.len(), 43, "32 bytes, base64url");
    let colombo = time::UtcOffset::from_hms(5, 30, 0).unwrap();
    assert_eq!(expires_at, hotel.day(3 + 31).midnight().assume_offset(colombo), "the end of check-out + 30 days");

    let jobs: Vec<serde_json::Value> =
        sqlx::query_scalar("select payload from outbox where topic = 'mail-send' order by id")
            .fetch_all(&hotel.owner)
            .await
            .unwrap();
    let job: mail::MailJob = serde_json::from_value(jobs[0].clone()).unwrap();
    assert_eq!(
        job,
        mail::MailJob::IbeConfirmation {
            tenant_id: hotel.tenant.0,
            reservation_id: booked.reservation_id,
            token: booked.token.clone()
        }
    );

    let audits: Vec<(Option<Uuid>, String, serde_json::Value)> = sqlx::query_as(
        "select actor_user_id, action, data from audit_log
         where entity_id = $1 and action in ('ibe.booking_created', 'reservation.created') order by action",
    )
    .bind(booked.reservation_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(audits[0].1, "ibe.booking_created");
    assert_eq!(audits[0].2, serde_json::json!({"actor": "ibe", "rooms": 2}), "no email or phone");
    assert_eq!(
        (audits[1].0, audits[1].1.as_str(), &audits[1].2["actor"]),
        (None, "reservation.created", &"ibe".into())
    );

    let confirmation = ibe::confirmation(&mut tx, booked.reservation_id, &booked.token).await.unwrap().unwrap();
    assert_eq!(confirmation.totals, request.expected_totals);
    assert_eq!((confirmation.code.as_str(), confirmation.booker_email.as_str()), ("GAL", "kamala@example.com"));
    assert_eq!(confirmation.contact_email.as_deref(), Some("desk@gallefort.example"));
    assert_eq!(confirmation.deposits.len(), 1, "the advisory deposit");
    assert!(ibe::confirmation(&mut tx, booked.reservation_id, "not-the-token").await.unwrap().is_none());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn price_changed_needs_reconfirm(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts, 3).await;
    let mut request = hotel.request(&hotel.bar, Residency::Resident, &[(2, 0)], "kamala@example.com").await;
    let quoted = request.expected_totals.clone();
    request.expected_totals.insert("LKR".into(), quoted["LKR"] - 1_000);

    let changed = hotel.book(&request).await;

    match changed {
        Err(BookingError::PriceChanged { totals }) => assert_eq!(totals, quoted),
        other => panic!("{other:?}"),
    }
    assert_eq!(hotel.reservations().await, 0);
    request.expected_totals = quoted;
    hotel.book(&request).await.expect("booked once the guest confirms the new totals");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn sold_out_race(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts, 1).await;
    let first = hotel.request(&hotel.bar, Residency::Resident, &[(2, 0)], "kamala@example.com").await;
    let second = hotel.request(&hotel.bar, Residency::Resident, &[(2, 0)], "nimal@example.com").await;
    let held = hotel.hold_inventory().await;

    // Both bookings price the stay and find the last room free, then queue at the inventory lock: the loser's
    // `SoldOut` can then only come from the reservation command's no-room `Conflict`, not from its search.
    let release = async {
        hotel.wait_for_lock_waiters(2).await;
        held.rollback().await.unwrap();
    };
    let (a, b, ()) = tokio::join!(hotel.book(&first), hotel.book(&second), release);

    let results = [a, b];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1, "{results:?}");
    assert_eq!(results.iter().filter(|result| matches!(result, Err(BookingError::SoldOut))).count(), 1, "{results:?}");
    assert_eq!(hotel.reservations().await, 1);
    let late = hotel.book(&second).await;
    assert!(matches!(late, Err(BookingError::SoldOut)), "a search that no longer offers it: {late:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_rate_edit_before_the_lock_is_seen(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts, 3).await;
    let request = hotel.request(&hotel.bar, Residency::Resident, &[(2, 0)], "kamala@example.com").await;
    let quoted = request.expected_totals["LKR"];
    let held = hotel.hold_inventory().await;

    // The booking prices the stay at the old rate and waits at the inventory lock while the rate goes up.
    let edit = async {
        hotel.wait_for_lock_waiters(1).await;
        let mut tx = hotel.tx().await;
        let prices: Vec<rates::Price> = (1..3)
            .map(|day| rates::Price {
                room_type_id: hotel.deluxe.id,
                date: hotel.day(day),
                occupancy: 1,
                amount: 12_000,
            })
            .collect();
        rates::set_prices(&mut tx, hotel.tenant, hotel.user, hotel.property, hotel.bar.id, &prices).await.unwrap();
        tx.commit().await.unwrap();
        held.rollback().await.unwrap();
    };
    let (booked, ()) = tokio::join!(hotel.book(&request), edit);

    match booked {
        Err(BookingError::PriceChanged { totals }) => assert!(totals["LKR"] > quoted, "{totals:?} after {quoted}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(hotel.reservations().await, 0, "the booking rolled back");
    let new_totals = hotel.totals(&request).await;
    hotel.book(&BookingRequest { expected_totals: new_totals, ..request }).await.expect("booked at the new rate");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn occupancy_over_max_is_invalid(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts, 3).await;
    let mut request = hotel.request(&hotel.bar, Residency::Resident, &[(2, 0)], "kamala@example.com").await;
    request.rooms[0].adults = 3;

    let over = hotel.book(&request).await;
    request.rooms[0].adults = 2;
    request.agreed = false;
    let not_agreed = hotel.book(&request).await;

    assert!(matches!(over, Err(BookingError::Invalid(ref message)) if message.contains("holds 2 adults")), "{over:?}");
    assert!(matches!(not_agreed, Err(BookingError::NotAgreed)), "{not_agreed:?}");
    assert_eq!(hotel.reservations().await, 0);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn residency_mismatch_is_invalid(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts, 3).await;
    let residents =
        hotel.plan(NewRatePlan { residency: Some(Residency::Resident), ..new_plan("FITL", hotel.deluxe.id) }).await;
    let mut request = hotel.request(&residents, Residency::Resident, &[(2, 0)], "kamala@example.com").await;
    request.residency = Residency::NonResident;

    let mismatch = hotel.book(&request).await;

    assert!(
        matches!(mismatch, Err(BookingError::Invalid(ref message)) if message.contains("only for residents")),
        "{mismatch:?}"
    );
    assert_eq!(hotel.reservations().await, 0);
}
