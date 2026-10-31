//! The manage link: viewing a booking by its token, and cancelling it online under the terms it kept.

mod common;

use common::Hotel;
use ibe::{Booked, ManageError};
use rates::Residency;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::collections::BTreeMap;
use time::OffsetDateTime;
use uuid::Uuid;

impl Hotel {
    /// A booking of `rooms` for `[D + 1, D + 3)`: one night's penalty (11 000 a room, as booked) if cancelled now, the rule of
    /// its policy.
    async fn booked(&self, rooms: &[(i32, i32)]) -> Booked {
        let request = self.request(&self.bar, Residency::Resident, rooms, "kamala@example.com").await;
        self.book(&request).await.unwrap()
    }

    async fn manage(&self, token: &str) -> Result<ibe::ManageView, ManageError> {
        ibe::manage(&mut self.tx().await, self.property, token).await
    }

    /// Cancels in a transaction that commits only when the cancellation worked.
    async fn cancel(&self, token: &str) -> Result<ibe::ManageView, ManageError> {
        let mut tx = self.tx().await;
        let view = ibe::cancel(&mut tx, self.tenant, self.property, token).await?;
        tx.commit().await.unwrap();
        Ok(view)
    }

    async fn statuses(&self, reservation: Uuid) -> Vec<String> {
        sqlx::query_scalar("select status from reservation_room where reservation_id = $1 order by id")
            .bind(reservation)
            .fetch_all(&mut *self.tx().await)
            .await
            .unwrap()
    }

    /// The `mail-send` jobs in the outbox, oldest first.
    async fn mail(&self) -> Vec<serde_json::Value> {
        sqlx::query_scalar("select payload from outbox where topic = 'mail-send' order by id")
            .fetch_all(&self.owner)
            .await
            .unwrap()
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn token_valid_until_30_days_after_checkout(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts, 3).await;
    let booked = hotel.booked(&[(2, 0)]).await;

    let expires_at: OffsetDateTime =
        sqlx::query_scalar("select expires_at from reservation_access_token where reservation_id = $1")
            .bind(booked.reservation_id)
            .fetch_one(&hotel.owner)
            .await
            .unwrap();
    // Check-out is D + 3: the 30th day after it is D + 33, and the link works to the end of that day in Colombo.
    let colombo = time::UtcOffset::from_hms(5, 30, 0).unwrap();
    assert_eq!(expires_at, hotel.day(34).midnight().assume_offset(colombo));
    assert!(hotel.manage(&booked.token).await.is_ok());

    let set_expiry = |seconds: i32| {
        let (owner, reservation) = (hotel.owner.clone(), booked.reservation_id);
        async move {
            sqlx::query(
                "update reservation_access_token set expires_at = now() + make_interval(secs => $2)
                 where reservation_id = $1",
            )
            .bind(reservation)
            .bind(f64::from(seconds))
            .execute(&owner)
            .await
            .unwrap();
        }
    };
    set_expiry(60).await;
    assert!(hotel.manage(&booked.token).await.is_ok(), "just before the end");
    set_expiry(-1).await;
    assert!(matches!(hotel.manage(&booked.token).await, Err(ManageError::Gone)), "just after the end");
    assert!(matches!(hotel.cancel(&booked.token).await, Err(ManageError::Gone)));
    assert_eq!(hotel.statuses(booked.reservation_id).await, ["confirmed"]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn wrong_or_expired_token_is_gone(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts, 3).await;
    let booked = hotel.booked(&[(2, 0)]).await;
    sqlx::query("update reservation_access_token set expires_at = now() - interval '1 day'")
        .execute(&hotel.owner)
        .await
        .unwrap();
    let live = hotel.booked(&[(2, 0)]).await;
    let mut tx = hotel.tx().await;
    let elsewhere = property::NewProperty {
        code: "COL".into(),
        name: "Colombo Hotel".into(),
        timezone: "Asia/Colombo".into(),
        base_currency: "LKR".into(),
    };
    let elsewhere = property::create_property(&mut tx, hotel.tenant, hotel.user, elsewhere).await.unwrap().id;
    tx.commit().await.unwrap();

    let gone = |result: Result<ibe::ManageView, ManageError>| match result {
        Err(err @ ManageError::Gone) => err.to_string(),
        other => panic!("not gone: {other:?}"),
    };
    let wrong = gone(hotel.manage("not-a-token-at-all").await);
    let expired = gone(hotel.manage(&booked.token).await);
    let other_property = gone(ibe::manage(&mut hotel.tx().await, elsewhere, &live.token).await);
    let cancel_wrong = gone(hotel.cancel("not-a-token-at-all").await);
    let cancel_expired = gone(hotel.cancel(&booked.token).await);

    assert_eq!(wrong, "This link has expired or is not valid");
    assert!([&expired, &other_property, &cancel_wrong, &cancel_expired].iter().all(|body| **body == wrong));
    assert!(hotel.manage(&live.token).await.is_ok(), "the live token still works on its own property");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn manage_shows_the_guest_the_booking_and_nothing_internal(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts, 3).await;
    let booked = hotel.booked(&[(2, 0), (2, 1)]).await;

    let view = hotel.manage(&booked.token).await.unwrap();

    assert_eq!(view.property_name, "Galle Fort Hotel");
    assert_eq!(view.confirmation_no, booked.confirmation_no);
    assert_eq!((view.check_in, view.check_out), (hotel.day(1), hotel.day(3)));
    assert_eq!(view.status, "confirmed");
    assert_eq!(view.contact_email.as_deref(), Some("desk@gallefort.example"));
    assert_eq!(view.rooms.len(), 2);
    assert_eq!((view.rooms[0].room_type.as_str(), view.rooms[0].adults), ("Deluxe", 2));
    assert_eq!(view.rooms[1].children + view.rooms[0].children, 1);
    assert_eq!(view.totals.len(), 1);
    assert_eq!(view.cancellation.len(), 1, "one wording for both rooms: {:?}", view.cancellation);
    assert!(view.cancellable);
    assert_eq!(
        view.penalty_if_cancelled,
        BTreeMap::from([("LKR".to_owned(), 22_000)]),
        "a night per room, meals and tax in"
    );
    assert!(view.cancellation_penalty.is_empty());

    let shown = serde_json::to_string(&view).unwrap();
    for internal in ["quiet room", "created_by", "guest_id", "notes", "kamala@example.com", "Perera"] {
        assert!(!shown.contains(internal), "{internal} in {shown}");
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn cancel_applies_snapshotted_penalty(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts, 3).await;
    let booked = hotel.booked(&[(2, 0), (2, 1)]).await;
    // The plan's live policy is free from now on; the booking keeps the terms it was made under.
    sqlx::query("update cancellation_policy set rules = '[]'").execute(&hotel.owner).await.unwrap();

    let view = hotel.cancel(&booked.token).await.unwrap();

    assert_eq!(view.status, "cancelled");
    assert!(!view.cancellable && view.penalty_if_cancelled.is_empty());
    assert_eq!(view.cancellation_penalty, BTreeMap::from([("LKR".to_owned(), 22_000)]));
    assert_eq!(hotel.statuses(booked.reservation_id).await, ["cancelled", "cancelled"]);
    let mut tx = hotel.tx().await;
    let fees: Vec<(i64,)> = sqlx::query_as(
        "select fl.net from folio_line fl join reservation_room rr on fl.posting_key = 'cancellation:' || rr.id
         where rr.reservation_id = $1 order by fl.net",
    )
    .bind(booked.reservation_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(fees, [(11_000,), (11_000,)], "the penalty is billed, never waived");
    let rooms: Vec<(Option<Uuid>, Option<i64>)> =
        sqlx::query_as("select cancelled_by, cancellation_penalty from reservation_room where reservation_id = $1")
            .bind(booked.reservation_id)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert!(rooms.iter().all(|room| *room == (None, Some(11_000))), "no staff member: {rooms:?}");
    let sold: i64 = sqlx::query_scalar("select coalesce(sum(sold), 0) from inventory_day where room_type_id = $1")
        .bind(hotel.deluxe.id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(sold, 0, "the nights went back on sale");
    let audits: Vec<(Option<Uuid>, String, serde_json::Value)> = sqlx::query_as(
        "select a.actor_user_id, a.action, a.data from audit_log a
         where a.action in ('reservation_room.cancelled', 'ibe.booking_cancelled')
           and (a.entity_id = $1 or a.entity_id in (select id from reservation_room where reservation_id = $1))
         order by a.action, a.id",
    )
    .bind(booked.reservation_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(audits.len(), 3);
    assert_eq!(audits[0].1, "ibe.booking_cancelled");
    assert_eq!(audits[0].2, serde_json::json!({"actor": "ibe", "rooms": 2, "reason": "Cancelled by the guest online"}));
    assert!(audits.iter().all(|audit| audit.0.is_none()));
    assert!(audits[1..].iter().all(|audit| audit.2["reason"] == "Cancelled by the guest online"));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn checked_in_booking_not_cancellable(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts, 3).await;
    let booked = hotel.booked(&[(2, 0)]).await;
    sqlx::query(
        "update reservation_room set status = 'checked_in', checked_in_at = now(), checked_in_business_date = $2
         where reservation_id = $1",
    )
    .bind(booked.reservation_id)
    .bind(hotel.business_date)
    .execute(&hotel.owner)
    .await
    .unwrap();

    let view = hotel.manage(&booked.token).await.unwrap();
    let cancelled = hotel.cancel(&booked.token).await;

    assert_eq!((view.status.as_str(), view.cancellable), ("checked_in", false), "read-only");
    assert!(matches!(cancelled, Err(ManageError::NotCancellable(_))), "{cancelled:?}");
    assert_eq!(hotel.statuses(booked.reservation_id).await, ["checked_in"]);
    assert_eq!(hotel.mail().await.len(), 1, "only the confirmation");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_arriving_today_or_partly_cancelled_is_not_cancellable_and_nothing_changes(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let hotel = Hotel::new(opts, 3).await;
    let mut today = hotel.request(&hotel.bar, Residency::Resident, &[(2, 0)], "kamala@example.com").await;
    today.check_in = hotel.day(0);
    today.expected_totals = hotel.totals(&today).await;
    let today = hotel.book(&today).await.unwrap();
    let two = hotel.booked(&[(2, 0), (2, 0)]).await;
    let mut tx = hotel.tx().await;
    let first: (Uuid, i32) =
        sqlx::query_as("select id, version from reservation_room where reservation_id = $1 order by id limit 1")
            .bind(two.reservation_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    reservations::cancel_room(&mut tx, hotel.tenant, Some(hotel.user), hotel.property, first.0, first.1, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mail_before = hotel.mail().await.len();

    let arriving = hotel.cancel(&today.token).await;
    let partly = hotel.cancel(&two.token).await;

    assert!(matches!(arriving, Err(ManageError::NotCancellable(_))), "{arriving:?}");
    assert!(matches!(partly, Err(ManageError::NotCancellable(_))), "{partly:?}");
    assert_eq!(hotel.statuses(today.reservation_id).await, ["confirmed"]);
    assert_eq!(hotel.statuses(two.reservation_id).await.iter().filter(|status| *status == "confirmed").count(), 1);
    assert_eq!(hotel.mail().await.len(), mail_before);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn second_cancel_reports_already_cancelled(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts, 3).await;
    let booked = hotel.booked(&[(2, 0)]).await;
    hotel.cancel(&booked.token).await.unwrap();

    let again = hotel.cancel(&booked.token).await;

    assert!(matches!(again, Err(ManageError::AlreadyCancelled)), "{again:?}");
    assert_eq!(hotel.manage(&booked.token).await.unwrap().status, "cancelled", "the page still shows it");
    assert_eq!(hotel.mail().await.len(), 2, "the confirmation and one cancellation");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn two_simultaneous_cancels_give_one_success_and_one_already_cancelled(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts, 3).await;
    let booked = hotel.booked(&[(2, 0), (2, 1)]).await;

    let (first, second) = tokio::join!(hotel.cancel(&booked.token), hotel.cancel(&booked.token));

    let outcomes = [first, second];
    assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1, "{outcomes:?}");
    assert_eq!(
        outcomes.iter().filter(|outcome| matches!(outcome, Err(ManageError::AlreadyCancelled))).count(),
        1,
        "{outcomes:?}"
    );
    assert_eq!(hotel.mail().await.len(), 2, "the confirmation and one cancellation");
    let fees: i64 = sqlx::query_scalar("select count(*) from folio_line where posting_key like 'cancellation:%'")
        .fetch_one(&hotel.owner)
        .await
        .unwrap();
    assert_eq!(fees, 2, "each room's penalty once");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn cancellation_email_sent(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts, 3).await;
    let booked = hotel.booked(&[(2, 0)]).await;

    hotel.cancel(&booked.token).await.unwrap();

    let jobs = hotel.mail().await;
    assert_eq!(jobs.len(), 2);
    let job: mail::MailJob = serde_json::from_value(jobs[1].clone()).unwrap();
    assert_eq!(
        job,
        mail::MailJob::IbeCancellation { tenant_id: hotel.tenant.0, reservation_id: booked.reservation_id }
    );
    assert!(!jobs[1].to_string().contains(&booked.token), "the cancellation carries no token");
    let notice = ibe::cancellation(&mut hotel.tx().await, booked.reservation_id).await.unwrap().unwrap();
    assert_eq!(notice.booker_email, "kamala@example.com");
    assert_eq!(notice.confirmation_no, booked.confirmation_no);
    assert_eq!(notice.penalty, BTreeMap::from([("LKR".to_owned(), 11_000)]));
    let live = hotel.booked(&[(2, 0)]).await;
    assert!(
        ibe::cancellation(&mut hotel.tx().await, live.reservation_id).await.unwrap().is_none(),
        "a booking not cancelled has no cancellation to tell"
    );
}
