mod common;

use common::{Hotel, Plans, new_guest};
use reservations::{AssignedRoom, CreatedReservation, FreeRoom, Guest, ReservationsError};
use rooms::{BlockKind, NewBlock, NewBlockReason, Room, RoomChanges};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

impl Hotel {
    /// Assigns room `room` to the stay `stay` at `version` in its own transaction, committed if it succeeds.
    async fn try_assign(&self, stay: Uuid, version: i32, room: Uuid) -> Result<AssignedRoom, ReservationsError> {
        let mut tx = self.tx().await;
        let assigned =
            reservations::assign_room(&mut tx, self.tenant, self.user, self.property, stay, version, room).await?;
        tx.commit().await.unwrap();
        Ok(assigned)
    }

    async fn try_unassign(&self, stay: Uuid, version: i32) -> Result<AssignedRoom, ReservationsError> {
        let mut tx = self.tx().await;
        let unassigned =
            reservations::unassign_room(&mut tx, self.tenant, self.user, self.property, stay, version).await?;
        tx.commit().await.unwrap();
        Ok(unassigned)
    }

    /// The property's room numbered `number`.
    async fn numbered(&self, number: &str) -> Room {
        let rooms = rooms::list_rooms(&mut self.tx().await, self.property, None).await.unwrap();
        rooms.into_iter().find(|room| room.number == number).expect("a room with that number")
    }

    /// Books one DLX room on BAR for `[business date + from, business date + to)`.
    async fn stay(&self, booker: &Guest, plans: &Plans, from: i64, to: i64) -> CreatedReservation {
        self.try_book(booker, vec![self.room(self.deluxe.id, &plans.bar, from, to)]).await.unwrap()
    }

    /// Blocks `room` for `[business date + from, business date + to)`.
    async fn block(&self, room: &Room, from: i64, to: i64, kind: BlockKind) {
        let mut tx = self.tx().await;
        let reasons = rooms::list_block_reasons(&mut tx, self.property).await.unwrap();
        let reason = match reasons.into_iter().find(|reason| reason.code == "LEAK") {
            Some(reason) => reason,
            None => {
                let leak = NewBlockReason { code: "LEAK".into(), label: "Leak".into(), default_kind: kind };
                rooms::create_block_reason(&mut tx, self.tenant, self.user, self.property, leak).await.unwrap()
            }
        };
        let block = NewBlock {
            room_id: room.id,
            from: self.day(from),
            to: self.day(to),
            kind,
            reason_id: reason.id,
            note: String::new(),
        };
        rooms::create_block(&mut tx, self.tenant, self.user, self.property, block).await.unwrap();
        tx.commit().await.unwrap();
    }

    async fn free_rooms(&self, room_type: Uuid, from: i64, to: i64) -> Vec<String> {
        let mut tx = self.tx().await;
        let free = reservations::free_rooms(&mut tx, self.property, room_type, self.day(from), self.day(to));
        free.await.unwrap().into_iter().map(|room| room.number).collect()
    }

    /// Every reservation room of the tenant as `(id, room_id, version)`.
    async fn assignments(&self) -> Vec<(Uuid, Option<Uuid>, i32)> {
        sqlx::query_as("select id, room_id, version from reservation_room order by id")
            .fetch_all(&mut *self.tx().await)
            .await
            .unwrap()
    }

    /// The audit entries about `stay`, oldest first, as `(action, data)`.
    async fn audit_of(&self, stay: Uuid) -> Vec<(String, serde_json::Value)> {
        sqlx::query_as("select action, data from audit_log where entity_id = $1 order by id")
            .bind(stay)
            .fetch_all(&mut *self.tx().await)
            .await
            .unwrap()
    }

    async fn reservation_version(&self, reservation: Uuid) -> i32 {
        sqlx::query_scalar("select version from reservation where id = $1")
            .bind(reservation)
            .fetch_one(&mut *self.tx().await)
            .await
            .unwrap()
    }
}

fn conflict<T: std::fmt::Debug>(result: Result<T, ReservationsError>) -> String {
    match result {
        Err(ReservationsError::Conflict(message)) => message,
        other => panic!("expected Conflict, got {other:?}"),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_is_assigned_moved_and_unassigned_without_touching_the_counters(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let booked = hotel.stay(&booker, &plans, 1, 4).await;
    let stay = booked.rooms[0].id;
    let (r101, r102) = (hotel.numbered("101").await, hotel.numbered("102").await);
    let sold = hotel.sold(hotel.deluxe.id, 0, 5).await;

    let assigned = hotel.try_assign(stay, 1, r101.id).await.unwrap();
    let moved = hotel.try_assign(stay, 2, r102.id).await.unwrap();
    let unassigned = hotel.try_unassign(stay, 3).await.unwrap();

    let expected = |room: Option<&Room>, version| AssignedRoom {
        id: stay,
        reservation_id: booked.id,
        room_id: room.map(|room| room.id),
        room_number: room.map(|room| room.number.clone()),
        version,
    };
    assert_eq!(assigned, expected(Some(&r101), 2));
    assert_eq!(moved, expected(Some(&r102), 3));
    assert_eq!(unassigned, expected(None, 4));
    assert_eq!(hotel.assignments().await, vec![(stay, None, 4)]);
    assert_eq!(hotel.reservation_version(booked.id).await, 4, "the reservation's detail changed each time");
    assert_eq!(hotel.sold(hotel.deluxe.id, 0, 5).await, sold, "assigning never changes the counters");
    let audit = hotel.audit_of(stay).await;
    assert_eq!(
        audit,
        vec![
            (
                "reservation_room.assigned".into(),
                serde_json::json!({ "reservation_id": booked.id, "room_id": r101.id, "number": "101", "previous": null })
            ),
            (
                "reservation_room.assigned".into(),
                serde_json::json!({ "reservation_id": booked.id, "room_id": r102.id, "number": "102", "previous": "101" })
            ),
            (
                "reservation_room.unassigned".into(),
                serde_json::json!({ "reservation_id": booked.id, "room_id": r102.id, "number": "102" })
            ),
        ]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn only_an_active_unblocked_room_of_the_booked_type_is_assigned(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 4).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let stay = hotel.stay(&booker, &plans, 1, 4).await.rooms[0].id;
    let r102 = hotel.numbered("102").await;
    let r103 = hotel.numbered("103").await;
    let r104 = hotel.numbered("104").await;
    let mut tx = hotel.tx().await;
    let inactive = RoomChanges { active: Some(false), ..RoomChanges::default() };
    rooms::update_room(&mut tx, hotel.tenant, hotel.user, hotel.property, r102.id, r102.version, inactive)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    hotel.block(&r103, 3, 6, BlockKind::OutOfService).await;
    hotel.block(&r104, 0, 2, BlockKind::OutOfOrder).await;
    let r101 = hotel.numbered("101").await;
    // A block from the day the stay leaves is no obstacle.
    hotel.block(&r101, 4, 6, BlockKind::OutOfOrder).await;

    let wrong_type = conflict(hotel.try_assign(stay, 1, hotel.numbered("201").await.id).await);
    let deactivated = conflict(hotel.try_assign(stay, 1, r102.id).await);
    let out_of_service = conflict(hotel.try_assign(stay, 1, r103.id).await);
    let out_of_order = conflict(hotel.try_assign(stay, 1, r104.id).await);

    assert_eq!(wrong_type, "room 201 is a STD, this booking is for DLX");
    assert_eq!(deactivated, "room 102 is inactive");
    assert_eq!(out_of_service, format!("room 103 is blocked from {} to {}", hotel.day(3), hotel.day(6)));
    assert_eq!(out_of_order, format!("room 104 is blocked from {} to {}", hotel.day(0), hotel.day(2)));
    assert_eq!(hotel.assignments().await, vec![(stay, None, 1)]);
    assert!(hotel.try_assign(stay, 1, r101.id).await.is_ok());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_taken_on_any_of_the_nights_names_the_booking_that_has_it(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let first = hotel.stay(&booker, &plans, 1, 4).await;
    let second = hotel.stay(&booker, &plans, 3, 5).await;
    let r101 = hotel.numbered("101").await;
    hotel.try_assign(first.rooms[0].id, 1, r101.id).await.unwrap();

    let taken = conflict(hotel.try_assign(second.rooms[0].id, 1, r101.id).await);

    assert_eq!(taken, format!("room 101 is taken by {} on those nights", first.confirmation_no));
    assert_eq!(hotel.assignments().await, vec![(first.rooms[0].id, Some(r101.id), 2), (second.rooms[0].id, None, 1)]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn back_to_back_stays_share_a_room(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let r101 = hotel.numbered("101").await;
    let before = hotel.stay(&booker, &plans, 1, 3).await.rooms[0].id;
    let after = hotel.stay(&booker, &plans, 3, 5).await.rooms[0].id;

    hotel.try_assign(before, 1, r101.id).await.unwrap();
    hotel.try_assign(after, 1, r101.id).await.unwrap();

    assert_eq!(hotel.assignments().await, vec![(before, Some(r101.id), 2), (after, Some(r101.id), 2)]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_cancelled_booking_frees_its_room(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let r101 = hotel.numbered("101").await;
    let cancelled = hotel.stay(&booker, &plans, 1, 4).await.rooms[0].id;
    hotel.try_assign(cancelled, 1, r101.id).await.unwrap();
    let mut tx = hotel.tx().await;
    reservations::cancel_room(&mut tx, hotel.tenant, hotel.user, hotel.property, cancelled, 2).await.unwrap();
    tx.commit().await.unwrap();
    let stay = hotel.stay(&booker, &plans, 1, 4).await.rooms[0].id;

    let assigned = hotel.try_assign(stay, 1, r101.id).await;
    let reassigned = conflict(hotel.try_assign(cancelled, 3, hotel.numbered("102").await.id).await);
    let unassigned = conflict(hotel.try_unassign(cancelled, 3).await);

    assert_eq!(assigned.unwrap().room_id, Some(r101.id));
    assert_eq!(reassigned, "only a confirmed stay can be assigned a room; this one is cancelled");
    assert_eq!(unassigned, "only a confirmed stay can have its room unassigned; this one is cancelled");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn unassigning_a_stay_without_a_room_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let stay = hotel.stay(&booker, &plans, 1, 4).await.rooms[0].id;
    let r101 = hotel.numbered("101").await;
    hotel.try_assign(stay, 1, r101.id).await.unwrap();

    let again = conflict(hotel.try_assign(stay, 2, r101.id).await);
    hotel.try_unassign(stay, 2).await.unwrap();
    let nothing = conflict(hotel.try_unassign(stay, 3).await);

    assert_eq!(again, "the stay is already in room 101");
    assert_eq!(nothing, "the stay has no room assigned");
    assert_eq!(hotel.assignments().await, vec![(stay, None, 3)]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stale_version_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let stay = hotel.stay(&booker, &plans, 1, 4).await.rooms[0].id;
    let r101 = hotel.numbered("101").await;

    let stale_assign = hotel.try_assign(stay, 2, r101.id).await;
    hotel.try_assign(stay, 1, r101.id).await.unwrap();
    let stale_unassign = hotel.try_unassign(stay, 1).await;

    assert!(matches!(stale_assign, Err(ReservationsError::VersionMismatch("reservation room"))), "{stale_assign:?}");
    assert!(
        matches!(stale_unassign, Err(ReservationsError::VersionMismatch("reservation room"))),
        "{stale_unassign:?}"
    );
    assert_eq!(hotel.assignments().await, vec![(stay, Some(r101.id), 2)]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_or_stay_of_another_property_is_not_found(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let stay = hotel.stay(&booker, &plans, 1, 4).await.rooms[0].id;
    let mut tx = hotel.tx().await;
    let kandy = property::NewProperty {
        code: "KDY".into(),
        name: "Kandy".into(),
        timezone: "Asia/Colombo".into(),
        base_currency: "LKR".into(),
    };
    let kandy = property::create_property(&mut tx, hotel.tenant, hotel.user, kandy).await.unwrap();
    let dlx = deluxe_type();
    let dlx = rooms::create_room_type(&mut tx, hotel.tenant, hotel.user, kandy.id, dlx).await.unwrap();
    let elsewhere = rooms::NewRoom { room_type_id: dlx.id, number: "101".into(), floor: None, section_id: None };
    let elsewhere = rooms::create_room(&mut tx, hotel.tenant, hotel.user, kandy.id, elsewhere).await.unwrap();
    tx.commit().await.unwrap();

    let other_room = hotel.try_assign(stay, 1, elsewhere.id).await;
    let mut tx = hotel.tx().await;
    let other_stay =
        reservations::assign_room(&mut tx, hotel.tenant, hotel.user, kandy.id, stay, 1, elsewhere.id).await;

    assert!(matches!(other_room, Err(ReservationsError::NotFound("room"))), "{other_room:?}");
    assert!(matches!(other_stay, Err(ReservationsError::NotFound("reservation room"))), "{other_stay:?}");
}

/// A room type like the test hotel's DLX, for another property.
fn deluxe_type() -> rooms::NewRoomType {
    rooms::NewRoomType {
        code: "DLX".into(),
        name: "Deluxe".into(),
        base_occupancy: 2,
        max_adults: 2,
        max_children: 1,
        max_occupancy: 3,
        bed_config: vec![],
        amenities: vec![],
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn parallel_assignments_of_one_room_to_overlapping_stays_give_it_to_one(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts.clone(), 2).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let stays = [hotel.stay(&booker, &plans, 1, 4).await, hotel.stay(&booker, &plans, 2, 5).await];
    let r101 = hotel.numbered("101").await;

    let pool = db::testing::app_pool(opts, 2).await;
    let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);
    // Both transactions are open before either assigns, so the assignments really overlap.
    let start = std::sync::Arc::new(tokio::sync::Barrier::new(stays.len()));
    let mut tasks = tokio::task::JoinSet::new();
    for booked in &stays {
        let (pool, start, stay) = (pool.clone(), start.clone(), booked.rooms[0].id);
        tasks.spawn(async move {
            let mut tx = db::begin(&pool, db::Scope::tenant(tenant)).await.unwrap();
            start.wait().await;
            let assigned = reservations::assign_room(&mut tx, tenant, user, property, stay, 1, r101.id).await?;
            tx.commit().await?;
            Ok::<_, ReservationsError>(assigned)
        });
    }
    let results = tasks.join_all().await;

    let winners: Vec<&AssignedRoom> = results.iter().filter_map(|result| result.as_ref().ok()).collect();
    assert_eq!(winners.len(), 1, "{results:?}");
    let winner = stays.iter().find(|booked| booked.rooms[0].id == winners[0].id).unwrap();
    let conflicts: Vec<String> =
        results.into_iter().filter_map(|result| result.err()).map(|err| conflict::<()>(Err(err))).collect();
    assert_eq!(conflicts, [format!("room 101 is taken by {} on those nights", winner.confirmation_no)]);
    let assigned: Vec<Option<Uuid>> = hotel.assignments().await.into_iter().map(|(_, room, _)| room).collect();
    assert_eq!(assigned.iter().filter(|room| room.is_some()).count(), 1);
}

/// Rounds of an assignment and an out-of-order block of the same room on the same nights, run at once. Each round
/// books a room and may block another, so the hotel has two rooms per round.
const ASSIGN_OR_BLOCK_ROUNDS: usize = 10;

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_assignment_and_a_block_of_the_same_room_at_once_let_one_through(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts.clone(), 2 * ASSIGN_OR_BLOCK_ROUNDS).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let reason = {
        let mut tx = hotel.tx().await;
        let leak = NewBlockReason { code: "LEAK".into(), label: "Leak".into(), default_kind: BlockKind::OutOfOrder };
        let reason = rooms::create_block_reason(&mut tx, hotel.tenant, hotel.user, hotel.property, leak).await;
        tx.commit().await.unwrap();
        reason.unwrap()
    };
    let pool = db::testing::app_pool(opts, 2).await;
    let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);

    for round in 0..ASSIGN_OR_BLOCK_ROUNDS {
        let room = hotel.numbered(&format!("{}", 101 + round)).await;
        let stay = hotel.stay(&booker, &plans, 1, 4).await.rooms[0].id;
        let block = NewBlock {
            room_id: room.id,
            from: hotel.day(2),
            to: hotel.day(3),
            kind: BlockKind::OutOfOrder,
            reason_id: reason.id,
            note: String::new(),
        };
        let start = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let (assigning, blocking) = (
            {
                let (pool, start) = (pool.clone(), start.clone());
                tokio::spawn(async move {
                    let mut tx = db::begin(&pool, db::Scope::tenant(tenant)).await.unwrap();
                    start.wait().await;
                    let assigned = reservations::assign_room(&mut tx, tenant, user, property, stay, 1, room.id).await;
                    if assigned.is_ok() {
                        tx.commit().await.unwrap();
                    }
                    assigned.map(|_| ()).map_err(|err| err.to_string())
                })
            },
            {
                let (pool, start) = (pool.clone(), start.clone());
                tokio::spawn(async move {
                    let mut tx = db::begin(&pool, db::Scope::tenant(tenant)).await.unwrap();
                    start.wait().await;
                    let blocked = rooms::create_block(&mut tx, tenant, user, property, block).await;
                    if blocked.is_ok() {
                        tx.commit().await.unwrap();
                    }
                    blocked.map(|_| ()).map_err(|err| err.to_string())
                })
            },
        );
        let (assigned, blocked) = (assigning.await.unwrap(), blocking.await.unwrap());

        let confirmation = hotel.confirmation_numbers().await.pop().unwrap();
        let refusals = [
            format!("room {} is blocked from {} to {}", room.number, hotel.day(2), hotel.day(3)),
            format!("room {} is assigned to {confirmation} on those nights", room.number),
        ];
        match (&assigned, &blocked) {
            (Ok(()), Err(message)) | (Err(message), Ok(())) => {
                assert!(refusals.contains(message), "round {round}: {message}")
            }
            _ => panic!("round {round}: exactly one should go through: {assigned:?}, {blocked:?}"),
        }
    }
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_retyped_while_a_stay_is_being_assigned_to_it_is_a_wrong_type_conflict(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts.clone(), 1).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let stay = hotel.stay(&booker, &plans, 1, 4).await.rooms[0].id;
    let r101 = hotel.numbered("101").await;
    let standard = hotel.standard.id;
    let pool = db::testing::app_pool(opts, 2).await;
    let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);

    // Retypes room 101 to STD and holds the transaction open, so its row stays locked while the assignment
    // below is blocked waiting to lock it too.
    let mut retyping = db::begin(&pool, db::Scope::tenant(tenant)).await.unwrap();
    let retype = RoomChanges { room_type_id: Some(standard), ..RoomChanges::default() };
    rooms::update_room(&mut retyping, tenant, user, property, r101.id, r101.version, retype).await.unwrap();

    let (pool2, room_id) = (pool.clone(), r101.id);
    let assigning = tokio::spawn(async move {
        let mut tx = db::begin(&pool2, db::Scope::tenant(tenant)).await.unwrap();
        reservations::assign_room(&mut tx, tenant, user, property, stay, 1, room_id).await
    });
    // Gives the spawned task time to reach its `select ... for update` and start waiting on the row lock
    // `retyping` still holds, so the retype has genuinely committed while the assignment was in flight.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    retyping.commit().await.unwrap();

    let result = assigning.await.unwrap();

    assert_eq!(conflict(result), "room 101 is a STD, this booking is for DLX");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn free_rooms_leave_out_assigned_blocked_and_inactive_rooms(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 4).await;
    let booker = hotel.guest(new_guest("Ada", "Silva")).await;
    let (r101, r102, r104) = (hotel.numbered("101").await, hotel.numbered("102").await, hotel.numbered("104").await);
    let stay = hotel.stay(&booker, &plans, 1, 4).await.rooms[0].id;
    hotel.try_assign(stay, 1, r101.id).await.unwrap();
    // A cancelled stay's room is free again, though the cancelled stay keeps it on record.
    let cancelled = hotel.stay(&booker, &plans, 1, 4).await.rooms[0].id;
    hotel.try_assign(cancelled, 1, r102.id).await.unwrap();
    let mut tx = hotel.tx().await;
    reservations::cancel_room(&mut tx, hotel.tenant, hotel.user, hotel.property, cancelled, 2).await.unwrap();
    tx.commit().await.unwrap();
    hotel.block(&hotel.numbered("103").await, 3, 5, BlockKind::OutOfService).await;
    let mut tx = hotel.tx().await;
    let east = rooms::create_section(&mut tx, hotel.tenant, hotel.user, hotel.property, "East").await.unwrap();
    let changes = RoomChanges { section_id: Some(Some(east.id)), ..RoomChanges::default() };
    rooms::update_room(&mut tx, hotel.tenant, hotel.user, hotel.property, r104.id, r104.version, changes)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let r105 = hotel.rooms(hotel.deluxe.id, &["105"]).await.remove(0);
    let mut tx = hotel.tx().await;
    let inactive = RoomChanges { active: Some(false), ..RoomChanges::default() };
    rooms::update_room(&mut tx, hotel.tenant, hotel.user, hotel.property, r105.id, r105.version, inactive)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = hotel.tx().await;
    let during =
        reservations::free_rooms(&mut tx, hotel.property, hotel.deluxe.id, hotel.day(1), hotel.day(4)).await.unwrap();

    let r102 = hotel.numbered("102").await;
    assert_eq!(
        during,
        vec![
            FreeRoom { id: r102.id, number: "102".into(), section: None },
            FreeRoom { id: r104.id, number: "104".into(), section: Some("East".into()) },
        ]
    );
    assert_eq!(hotel.free_rooms(hotel.deluxe.id, 4, 6).await, ["101", "102", "104"], "103 is blocked on 4");
    assert_eq!(hotel.free_rooms(hotel.deluxe.id, 5, 7).await, ["101", "102", "103", "104"]);
    assert_eq!(hotel.free_rooms(hotel.standard.id, 1, 4).await, ["201"]);
    let backwards = reservations::free_rooms(&mut tx, hotel.property, hotel.deluxe.id, hotel.day(4), hotel.day(4));
    assert!(matches!(backwards.await, Err(ReservationsError::Invalid(_))));
}
