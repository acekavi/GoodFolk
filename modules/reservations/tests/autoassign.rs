mod common;

use common::{Hotel, new_guest};
use reservations::{CreatedReservation, CreatedRoom, NewReservation, ReservationsError, Source};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

impl Hotel {
    /// The property's room numbered `number`.
    async fn numbered(&self, number: &str) -> rooms::Room {
        let rooms = rooms::list_rooms(&mut self.tx().await, self.property, None).await.unwrap();
        rooms.into_iter().find(|room| room.number == number).expect("a room with that number")
    }

    /// Assigns `room` to `stay` only if auto-assignment did not already put it there, in its own transaction:
    /// lets a test pin a stay to a room without caring whether auto-assign already agreed.
    async fn force_into(&self, stay: &CreatedRoom, room: Uuid) {
        if stay.room_id == Some(room) {
            return;
        }
        let mut tx = self.tx().await;
        reservations::assign_room(&mut tx, self.tenant, self.user, self.property, stay.id, stay.version, room)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    /// The `LEAK` block reason, created if this property does not have one yet, committed.
    async fn block_reason(&self, kind: rooms::BlockKind) -> rooms::BlockReason {
        let mut tx = self.tx().await;
        let reasons = rooms::list_block_reasons(&mut tx, self.property).await.unwrap();
        let reason = match reasons.into_iter().find(|reason| reason.code == "LEAK") {
            Some(reason) => reason,
            None => {
                let leak = rooms::NewBlockReason { code: "LEAK".into(), label: "Leak".into(), default_kind: kind };
                rooms::create_block_reason(&mut tx, self.tenant, self.user, self.property, leak).await.unwrap()
            }
        };
        tx.commit().await.unwrap();
        reason
    }

    /// Blocks `room` for `[business date + from, business date + to)`, in its own transaction.
    async fn block(&self, room: &rooms::Room, from: i64, to: i64, kind: rooms::BlockKind) -> rooms::Block {
        let reason = self.block_reason(kind).await;
        let mut tx = self.tx().await;
        let new_block = rooms::NewBlock {
            room_id: room.id,
            from: self.day(from),
            to: self.day(to),
            kind,
            reason_id: reason.id,
            note: String::new(),
        };
        let block = rooms::create_block(&mut tx, self.tenant, self.user, self.property, new_block).await.unwrap();
        tx.commit().await.unwrap();
        block
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_new_booking_gets_a_room_of_its_type(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;

    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 3)]).await.unwrap();

    let stay = &booked.rooms[0];
    let room_id = stay.room_id.expect("a room fits");
    assert_eq!(stay.room_number.as_deref(), Some("101"));
    let assigned_type: Uuid = sqlx::query_scalar("select room_type_id from room where id = $1")
        .bind(room_id)
        .fetch_one(&mut *hotel.tx().await)
        .await
        .unwrap();
    assert_eq!(assigned_type, hotel.deluxe.id);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_tightest_fit_is_chosen(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 3).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let room103 = hotel.numbered("103").await;

    // Fill 103, which is last in rail order, with 0..2 and 5..8, leaving exactly 2..5 free on it; reassign by
    // hand if auto-assign put either booking elsewhere, so the gap this test relies on is guaranteed regardless.
    let first = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 2)]).await.unwrap();
    hotel.force_into(&first.rooms[0], room103.id).await;
    let second = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 5, 8)]).await.unwrap();
    hotel.force_into(&second.rooms[0], room103.id).await;

    let third = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 5)]).await.unwrap();

    assert_eq!(third.rooms[0].room_number.as_deref(), Some("103"), "not 101, which ties on rail order: {third:?}");
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_shorter_gap_after_wins_when_the_gap_before_is_equal(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 3).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let room102 = hotel.numbered("102").await;
    // Nothing comes before 2..4 on any room, so every gap_before is the horizon; only 102 has a stay soon
    // after (5..7: one free night), and 102 is not first in rail order.
    let later = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 5, 7)]).await.unwrap();
    hotel.force_into(&later.rooms[0], room102.id).await;

    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 4)]).await.unwrap();

    assert_eq!(booked.rooms[0].room_number.as_deref(), Some("102"), "{booked:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn ties_go_to_the_first_room_in_rail_order(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 3).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let all = rooms::list_rooms(&mut hotel.tx().await, hotel.property, None).await.unwrap();
    let room103 = all.iter().find(|room| room.number == "103").expect("103 exists").id;
    let rest: Vec<Uuid> = all.iter().map(|room| room.id).filter(|id| *id != room103).collect();
    let ids: Vec<Uuid> = std::iter::once(room103).chain(rest).collect();
    let mut tx = hotel.tx().await;
    rooms::reorder_rooms(&mut tx, hotel.tenant, hotel.property, &ids).await.unwrap();
    tx.commit().await.unwrap();

    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 3)]).await.unwrap();

    assert_eq!(booked.rooms[0].room_number.as_deref(), Some("103"), "{booked:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn ties_follow_the_rails_natural_number_order(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    hotel.rooms(hotel.deluxe.id, &["99"]).await;
    // Same sort order for 101 and 99, so only the number breaks the tie. As text, "101" < "99".
    let mut tx = hotel.tx().await;
    sqlx::query("update room set sort_order = 0 where property_id = $1 and room_type_id = $2")
        .bind(hotel.property)
        .bind(hotel.deluxe.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;

    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 3)]).await.unwrap();

    assert_eq!(booked.rooms[0].room_number.as_deref(), Some("99"), "{booked:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn hand_assigned_rooms_are_never_moved(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let room102 = hotel.numbered("102").await;

    let a = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 3)]).await.unwrap();
    assert_eq!(a.rooms[0].room_number.as_deref(), Some("101"), "{a:?}");
    let mut tx = hotel.tx().await;
    let a_reassigned = reservations::assign_room(
        &mut tx,
        hotel.tenant,
        hotel.user,
        hotel.property,
        a.rooms[0].id,
        a.rooms[0].version,
        room102.id,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let b = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 3)]).await.unwrap();

    assert_eq!(b.rooms[0].room_number.as_deref(), Some("101"), "B takes the room A vacated: {b:?}");
    let a_room: Option<Uuid> = sqlx::query_scalar("select room_id from reservation_room where id = $1")
        .bind(a.rooms[0].id)
        .fetch_one(&mut *hotel.tx().await)
        .await
        .unwrap();
    assert_eq!(a_room, Some(room102.id), "A's hand assignment must survive B's booking");
    assert_eq!(a_reassigned.room_id, Some(room102.id));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_multi_room_booking_gets_distinct_rooms(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 3).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;

    let booked = hotel
        .try_book(
            &booker,
            vec![
                hotel.room(hotel.deluxe.id, &plans.bar, 1, 3),
                hotel.room(hotel.deluxe.id, &plans.bar, 1, 3),
                hotel.room(hotel.deluxe.id, &plans.bar, 1, 3),
            ],
        )
        .await
        .unwrap();

    let assigned: Vec<Uuid> = booked.rooms.iter().map(|room| room.room_id.expect("every room fits")).collect();
    let mut unique = assigned.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), 3, "{assigned:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_multi_room_booking_beyond_the_free_rooms_leaves_the_rest_unassigned(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    hotel.set_overbooking(&hotel.deluxe, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;

    let booked = hotel
        .try_book(
            &booker,
            vec![
                hotel.room(hotel.deluxe.id, &plans.bar, 1, 3),
                hotel.room(hotel.deluxe.id, &plans.bar, 1, 3),
                hotel.room(hotel.deluxe.id, &plans.bar, 1, 3),
            ],
        )
        .await
        .unwrap();

    let assigned: Vec<Uuid> = booked.rooms.iter().filter_map(|room| room.room_id).collect();
    let mut unique = assigned.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), 2, "the two physical-plus-allowance rooms are distinct: {:?}", booked.rooms);
    let unassigned = booked.rooms.iter().filter(|room| room.room_id.is_none()).count();
    assert_eq!(unassigned, 1, "the overbooked third room has no room to go in: {:?}", booked.rooms);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn split_nights_leave_the_stay_unassigned(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let room101 = hotel.numbered("101").await;
    let room102 = hotel.numbered("102").await;

    let first = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 2)]).await.unwrap();
    hotel.force_into(&first.rooms[0], room101.id).await;
    let second = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 2, 4)]).await.unwrap();
    hotel.force_into(&second.rooms[0], room102.id).await;

    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 4)]).await.unwrap();

    assert_eq!(booked.rooms[0].room_id, None, "no single room is free the whole stay: {booked:?}");
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_blocked_room_is_not_chosen(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let room101 = hotel.numbered("101").await;
    hotel.block(&room101, 1, 2, rooms::BlockKind::OutOfOrder).await;

    let booked = hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 0, 3)]).await.unwrap();

    assert_eq!(booked.rooms[0].room_number.as_deref(), Some("102"), "{booked:?}");
}

/// How many times each race test below repeats its race, as the brief asks.
const RACE_ITERATIONS: i64 = 20;

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn parallel_bookings_racing_for_the_last_room_never_double_book(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts.clone(), 1).await;
    hotel.set_overbooking(&hotel.deluxe, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;

    let pool = db::testing::app_pool(opts, 2).await;
    let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);
    // Each iteration uses its own nights, so the one physical room is free again every time without rebuilding
    // the property from scratch.
    for i in 0..RACE_ITERATIONS {
        let from = i * 2;
        let to = from + 2;
        let start = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..2 {
            let (pool, start) = (pool.clone(), start.clone());
            let input = NewReservation {
                booker_guest_id: booker.id,
                source: Source::Phone,
                notes: String::new(),
                account_id: None,
                rooms: vec![hotel.room(hotel.deluxe.id, &plans.bar, from, to)],
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
        assert_eq!(booked.len(), 2, "the allowance lets both bookings through, one just unassigned: {results:?}");
        let assigned: Vec<Uuid> = booked.iter().filter_map(|created| created.rooms[0].room_id).collect();
        assert_eq!(assigned.len(), 1, "iteration {i}: exactly one gets the room: {results:?}");
    }
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_booking_racing_a_block_never_ends_up_on_the_blocked_room(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts.clone(), 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let room101 = hotel.numbered("101").await;
    // Out of service, not out of order: it must never touch the sellable count, or the booking side of the
    // race could be refused outright instead of landing unassigned.
    let reason = hotel.block_reason(rooms::BlockKind::OutOfService).await;

    let pool = db::testing::app_pool(opts, 2).await;
    let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);
    for i in 0..RACE_ITERATIONS {
        let from = i * 2;
        let to = from + 2;
        let start = std::sync::Arc::new(tokio::sync::Barrier::new(2));

        let booking = {
            let (pool, start) = (pool.clone(), start.clone());
            let input = NewReservation {
                booker_guest_id: booker.id,
                source: Source::Phone,
                notes: String::new(),
                account_id: None,
                rooms: vec![hotel.room(hotel.deluxe.id, &plans.bar, from, to)],
            };
            tokio::spawn(async move {
                let mut tx = db::begin(&pool, db::Scope::tenant(tenant)).await.unwrap();
                start.wait().await;
                let created = reservations::create_reservation(&mut tx, tenant, user, property, input).await?;
                tx.commit().await?;
                Ok::<_, ReservationsError>(created)
            })
        };
        let block = {
            let (pool, start) = (pool.clone(), start.clone());
            let new_block = rooms::NewBlock {
                room_id: room101.id,
                from: hotel.day(from),
                to: hotel.day(to),
                kind: rooms::BlockKind::OutOfService,
                reason_id: reason.id,
                note: String::new(),
            };
            tokio::spawn(async move {
                let mut tx = db::begin(&pool, db::Scope::tenant(tenant)).await.unwrap();
                start.wait().await;
                let block = rooms::create_block(&mut tx, tenant, user, property, new_block).await?;
                tx.commit().await?;
                Ok::<_, rooms::RoomsError>(block)
            })
        };
        let (booking, block) = tokio::join!(booking, block);
        let booking = booking.unwrap();
        let block = block.unwrap();

        match block {
            Ok(_) => {
                let created = booking.expect("the booking always succeeds, assigned or not");
                assert_eq!(created.rooms[0].room_id, None, "iteration {i}: the block got 101 first: {created:?}");
            }
            Err(rooms::RoomsError::Conflict(_)) => {
                let created = booking.expect("the booking always succeeds, assigned or not");
                assert_eq!(
                    created.rooms[0].room_id,
                    Some(room101.id),
                    "iteration {i}: the booking got 101 first: {created:?}"
                );
            }
            Err(other) => panic!("iteration {i}: unexpected block error: {other:?}"),
        }
    }
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_audit_names_the_assigned_room(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    hotel.set_overbooking(&hotel.deluxe, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;

    let booked = hotel
        .try_book(
            &booker,
            vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 3), hotel.room(hotel.deluxe.id, &plans.bar, 1, 3)],
        )
        .await
        .unwrap();
    assert!(
        booked.rooms.iter().any(|room| room.room_id.is_none()),
        "the allowance leaves one of the two unassigned: {:?}",
        booked.rooms
    );

    let data: serde_json::Value =
        sqlx::query_scalar("select data from audit_log where entity_id = $1 and action = 'reservation.created'")
            .bind(booked.id)
            .fetch_one(&mut *hotel.tx().await)
            .await
            .unwrap();

    let assigned = data["assigned"].as_object().expect("assigned is an object");
    assert_eq!(assigned.len(), 2, "{data}");
    for room in &booked.rooms {
        let expected = match room.room_id {
            Some(id) => serde_json::json!(id),
            None => serde_json::Value::Null,
        };
        assert_eq!(assigned[&room.id.to_string()], expected, "{data}");
    }
}

/// How long a booking may take when it must not wait on a held room lock.
const NO_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_locked_by_another_transaction_is_skipped_not_waited_for(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let room101 = hotel.numbered("101").await;
    // Another command holds 101 uncommitted, as a block or retype in flight would.
    let mut holder = hotel.tx().await;
    sqlx::query("select 1 from room where id = $1 for update").bind(room101.id).execute(&mut *holder).await.unwrap();

    let booked =
        tokio::time::timeout(NO_WAIT, hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 3)]))
            .await
            .expect("booking must not wait on the held room")
            .unwrap();
    holder.commit().await.unwrap();

    assert_eq!(booked.rooms[0].room_number.as_deref(), Some("102"), "101 would rank first: {booked:?}");
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_only_free_room_being_locked_leaves_the_booking_unassigned_not_waiting(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let room101 = hotel.numbered("101").await;
    let mut holder = hotel.tx().await;
    sqlx::query("select 1 from room where id = $1 for update").bind(room101.id).execute(&mut *holder).await.unwrap();

    let booked =
        tokio::time::timeout(NO_WAIT, hotel.try_book(&booker, vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 3)]))
            .await
            .expect("booking must not wait on the held room")
            .unwrap();
    holder.commit().await.unwrap();

    assert_eq!(booked.rooms[0].room_id, None, "the one room was held: {booked:?}");
    assert_eq!(hotel.drift().await, vec![]);
}
