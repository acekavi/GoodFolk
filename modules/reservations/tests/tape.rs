mod common;

use common::{Hotel, Plans, new_guest};
use reservations::{
    CreatedReservation, Guest, NeedsRoomReason, ReservationsError, TapeBlock, TapeStay, TapeWindow, UnassignedStay,
};
use rooms::{BlockKind, NewBlock, NewBlockReason, NewRoomType, Room};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::Date;
use uuid::Uuid;

impl Hotel {
    async fn window(&self, rooms: &[Uuid], from: i64, to: i64) -> Result<TapeWindow, ReservationsError> {
        let mut tx = self.tx().await;
        reservations::tape_window(&mut tx, self.property, rooms, self.day(from), self.day(to)).await
    }

    async fn unassigned_between(&self, from: i64, to: i64) -> Result<Vec<UnassignedStay>, ReservationsError> {
        let mut tx = self.tx().await;
        reservations::unassigned_stays(&mut tx, self.property, self.day(from), self.day(to)).await
    }

    /// The property's rooms in display order.
    async fn all_rooms(&self) -> Vec<Room> {
        rooms::list_rooms(&mut self.tx().await, self.property, None).await.unwrap()
    }

    /// Books one DLX room on BAR for `[business date + from, business date + to)`, auto-assigned.
    async fn dlx(&self, booker: &Guest, plans: &Plans, from: i64, to: i64) -> CreatedReservation {
        self.try_book(booker, vec![self.room(self.deluxe.id, &plans.bar, from, to)]).await.unwrap()
    }

    /// As `dlx`, left without a room.
    async fn dlx_unassigned(&self, booker: &Guest, plans: &Plans, from: i64, to: i64) -> CreatedReservation {
        self.try_book_unassigned(booker, vec![self.room(self.deluxe.id, &plans.bar, from, to)]).await.unwrap()
    }

    async fn assign(&self, stay: &reservations::CreatedRoom, room: &Room) {
        let mut tx = self.tx().await;
        reservations::assign_room(&mut tx, self.tenant, self.user, self.property, stay.id, stay.version, room.id)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    async fn cancel(&self, stay: &reservations::CreatedRoom) {
        let mut tx = self.tx().await;
        reservations::cancel_room(&mut tx, self.tenant, self.user, self.property, stay.id, stay.version).await.unwrap();
        tx.commit().await.unwrap();
    }

    /// Blocks `room` for `[business date + from, business date + to)` and returns the block's id.
    async fn block(&self, room: &Room, from: i64, to: i64) -> Uuid {
        let mut tx = self.tx().await;
        let reasons = rooms::list_block_reasons(&mut tx, self.property).await.unwrap();
        let reason = match reasons.into_iter().find(|reason| reason.code == "LEAK") {
            Some(reason) => reason,
            None => {
                let leak =
                    NewBlockReason { code: "LEAK".into(), label: "Leak".into(), default_kind: BlockKind::OutOfOrder };
                rooms::create_block_reason(&mut tx, self.tenant, self.user, self.property, leak).await.unwrap()
            }
        };
        let block = NewBlock {
            room_id: room.id,
            from: self.day(from),
            to: self.day(to),
            kind: BlockKind::OutOfOrder,
            reason_id: reason.id,
            note: String::new(),
        };
        let block = rooms::create_block(&mut tx, self.tenant, self.user, self.property, block).await.unwrap();
        tx.commit().await.unwrap();
        block.id
    }

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
}

fn spans(stays: &[TapeStay]) -> Vec<(Date, Date)> {
    stays.iter().map(|stay| (stay.start, stay.end)).collect()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn stays_overlapping_the_window_are_returned_and_stays_touching_it_are_not(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 3).await;
    let silva = hotel.guest(new_guest("Anula", "Silva")).await;
    let room = hotel.all_rooms().await.into_iter().find(|room| room.number == "101").unwrap();
    // Window [10, 20): one stay per overlap shape, plus two that only touch it. All in room 101, one after the
    // other.
    let ends_at_from = hotel.dlx(&silva, &plans, 6, 10).await;
    let left_edge = hotel.dlx(&silva, &plans, 10, 12).await;
    let inside = hotel.dlx(&silva, &plans, 13, 15).await;
    let right_edge = hotel.dlx(&silva, &plans, 18, 22).await;
    let starts_at_to = hotel.dlx(&silva, &plans, 22, 24).await;
    for created in [&ends_at_from, &left_edge, &inside, &right_edge, &starts_at_to] {
        assert_eq!(created.rooms[0].room_id, Some(room.id), "tightest fit keeps these in one room");
    }

    let window = hotel.window(&[room.id], 10, 20).await.unwrap();

    assert_eq!(
        spans(&window.stays),
        vec![(hotel.day(10), hotel.day(12)), (hotel.day(13), hotel.day(15)), (hotel.day(18), hotel.day(22))]
    );
    assert_eq!(window.stays[0].id, left_edge.rooms[0].id);
    assert_eq!(window.stays[0].reservation_id, left_edge.id);
    assert_eq!(window.stays[0].room_type_id, hotel.deluxe.id);
    assert_eq!(window.stays[0].status, domain::RoomStatus::Confirmed);
    assert_eq!(window.stays[0].version, left_edge.rooms[0].version);
    assert!(window.blocks.is_empty());

    let spanning = hotel.window(&[room.id], 14, 15).await.unwrap();
    assert_eq!(spans(&spanning.stays), vec![(hotel.day(13), hotel.day(15))]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn only_the_named_rooms_stays_are_returned(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 3).await;
    let silva = hotel.guest(new_guest("Anula", "Silva")).await;
    let rooms = hotel.all_rooms().await;
    let first = hotel.dlx(&silva, &plans, 1, 3).await;
    let second = hotel.dlx(&silva, &plans, 1, 3).await;
    let named = first.rooms[0].room_id.unwrap();
    assert_ne!(named, second.rooms[0].room_id.unwrap());
    assert_eq!(rooms.len(), 4);

    let window = hotel.window(&[named], 0, 7).await.unwrap();

    assert_eq!(window.stays.iter().map(|stay| stay.id).collect::<Vec<_>>(), vec![first.rooms[0].id]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn cancelled_stays_and_released_blocks_are_left_out_and_live_blocks_carry_their_reason(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 3).await;
    let silva = hotel.guest(new_guest("Anula", "Silva")).await;
    let rooms = hotel.all_rooms().await;
    let r101 = rooms.iter().find(|room| room.number == "101").unwrap();
    let r102 = rooms.iter().find(|room| room.number == "102").unwrap();
    let cancelled = hotel.dlx(&silva, &plans, 2, 4).await;
    hotel.cancel(&cancelled.rooms[0]).await;
    let kept = hotel.dlx(&silva, &plans, 5, 6).await;
    let block = hotel.block(r102, 3, 6).await;
    let released = hotel.block(r101, 8, 9).await;
    let mut tx = hotel.tx().await;
    sqlx::query("update room_block set released_at = now() where id = $1")
        .bind(released)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let window = hotel.window(&[r101.id, r102.id], 0, 14).await.unwrap();

    assert_eq!(window.stays.iter().map(|stay| stay.id).collect::<Vec<_>>(), vec![kept.rooms[0].id]);
    assert_eq!(
        window.blocks,
        vec![TapeBlock { id: block, room_id: r102.id, start: hotel.day(3), end: hotel.day(6), reason: "Leak".into() }]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_unusable_room_list_or_window_is_refused(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, _) = Hotel::for_booking(opts, 12).await;
    let rooms: Vec<Uuid> = hotel.all_rooms().await.into_iter().map(|room| room.id).collect();
    assert_eq!(rooms.len(), 13);
    let other = {
        let mut tx = hotel.tx().await;
        let property = property::NewProperty {
            code: "COL".into(),
            name: "Colombo".into(),
            timezone: "Asia/Colombo".into(),
            base_currency: "LKR".into(),
        };
        let property = property::create_property(&mut tx, hotel.tenant, hotel.user, property).await.unwrap();
        let kind = NewRoomType {
            code: "DLX".into(),
            name: "Deluxe".into(),
            base_occupancy: 2,
            max_adults: 2,
            max_children: 1,
            max_occupancy: 3,
            overbooking: 0,
            bed_config: vec![],
            amenities: vec![],
        };
        let kind = rooms::create_room_type(&mut tx, hotel.tenant, hotel.user, property.id, kind).await.unwrap();
        let room = rooms::NewRoom { room_type_id: kind.id, number: "1".into(), floor: None, section_id: None };
        let room = rooms::create_room(&mut tx, hotel.tenant, hotel.user, property.id, room).await.unwrap();
        tx.commit().await.unwrap();
        room.id
    };

    assert!(hotel.window(&rooms[..10], 0, 7).await.is_ok());
    for refused in [
        hotel.window(&rooms[..11], 0, 7).await,
        hotel.window(&[], 0, 7).await,
        hotel.window(&[rooms[0], other], 0, 7).await,
        hotel.window(&[Uuid::now_v7()], 0, 7).await,
        hotel.window(&rooms[..1], 0, 43).await,
        hotel.window(&rooms[..1], 5, 5).await,
        hotel.window(&rooms[..1], 5, 4).await,
    ] {
        assert!(matches!(refused, Err(ReservationsError::Invalid(_))), "{refused:?}");
    }
    assert!(hotel.window(&rooms[..1], 0, 42).await.is_ok());
    assert!(matches!(hotel.unassigned_between(0, 43).await, Err(ReservationsError::Invalid(_))));
    assert!(matches!(hotel.unassigned_between(3, 3).await, Err(ReservationsError::Invalid(_))));
    // A repeated id counts once, so it names a single room of the property.
    assert!(hotel.window(&[rooms[0], rooms[0]], 0, 7).await.is_ok());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn guest_names_read_last_name_comma_first_initial_and_accounts_are_named(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 3).await;
    let silva = hotel.guest(new_guest("Anula", "Silva")).await;
    let cher = hotel.guest(new_guest("", "Cher")).await;
    let account = hotel.account(common::new_account("Acme Travel")).await;
    let named = hotel
        .try_book_for_account(&silva, Some(account.id), vec![hotel.room(hotel.deluxe.id, &plans.bar, 1, 3)])
        .await
        .unwrap();
    let single = hotel.dlx(&cher, &plans, 1, 3).await;
    let rooms: Vec<Uuid> = hotel.all_rooms().await.into_iter().map(|room| room.id).collect();

    let window = hotel.window(&rooms, 0, 7).await.unwrap();

    let by_id = |id: Uuid| window.stays.iter().find(|stay| stay.id == id).unwrap();
    assert_eq!(by_id(named.rooms[0].id).guest_name, "Silva, A.");
    assert_eq!(by_id(named.rooms[0].id).account_name.as_deref(), Some("Acme Travel"));
    assert_eq!(by_id(single.rooms[0].id).guest_name, "Cher");
    assert_eq!(by_id(single.rooms[0].id).account_name, None);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_sold_beyond_the_physical_rooms_needs_a_room_as_overbooked(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 1).await;
    hotel.set_overbooking(&hotel.deluxe, 1).await;
    let silva = hotel.guest(new_guest("Anula", "Silva")).await;
    let seated = hotel.dlx(&silva, &plans, 2, 5).await;
    let over = hotel.dlx(&silva, &plans, 3, 6).await;
    assert!(seated.rooms[0].room_id.is_some());
    assert!(over.rooms[0].room_id.is_none(), "one DLX room is taken on nights 3 and 4");

    let found = hotel.unassigned_between(0, 14).await.unwrap();

    assert_eq!(
        found,
        vec![UnassignedStay {
            id: over.rooms[0].id,
            reservation_id: over.id,
            room_type_id: hotel.deluxe.id,
            start: hotel.day(3),
            end: hotel.day(6),
            guest_name: "Silva, A.".into(),
            reason: NeedsRoomReason::Overbooked,
            version: over.rooms[0].version,
        }]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_no_single_room_can_hold_needs_a_room_as_no_single_room(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let silva = hotel.guest(new_guest("Anula", "Silva")).await;
    let rooms = hotel.all_rooms().await;
    let (r101, r102) = (&rooms[0], &rooms[1]);
    // Each room is free for half of the long stay, so the type has rooms enough but no one room fits.
    let early = hotel.dlx_unassigned(&silva, &plans, 0, 2).await;
    let late = hotel.dlx_unassigned(&silva, &plans, 2, 4).await;
    let long = hotel.dlx_unassigned(&silva, &plans, 0, 4).await;
    hotel.assign(&early.rooms[0], r101).await;
    hotel.assign(&late.rooms[0], r102).await;

    let found = hotel.unassigned_between(0, 14).await.unwrap();

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, long.rooms[0].id);
    assert_eq!(found[0].reason, NeedsRoomReason::NoSingleRoom);
    assert_eq!(found[0].version, long.rooms[0].version);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn only_confirmed_stays_in_the_window_without_a_room_are_listed_by_arrival(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 3).await;
    let silva = hotel.guest(new_guest("Anula", "Silva")).await;
    let later = hotel.dlx_unassigned(&silva, &plans, 6, 8).await;
    let sooner = hotel.dlx_unassigned(&silva, &plans, 4, 5).await;
    let outside = hotel.dlx_unassigned(&silva, &plans, 20, 22).await;
    let assigned = hotel.dlx(&silva, &plans, 5, 7).await;
    let cancelled = hotel.dlx_unassigned(&silva, &plans, 5, 7).await;
    hotel.cancel(&cancelled.rooms[0]).await;
    assert!(assigned.rooms[0].room_id.is_some());

    let found = hotel.unassigned_between(0, 14).await.unwrap();

    assert_eq!(found.iter().map(|stay| stay.id).collect::<Vec<_>>(), vec![sooner.rooms[0].id, later.rooms[0].id]);
    assert!(found.iter().all(|stay| stay.id != outside.rooms[0].id));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_past_arrival_unassigned_stay_reports_no_single_room_without_error(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, plans) = Hotel::for_booking(opts, 2).await;
    let silva = hotel.guest(new_guest("Anula", "Silva")).await;
    let stay = hotel.dlx_unassigned(&silva, &plans, 0, 3).await;
    hotel.move_business_date(1).await;

    let found = hotel.unassigned_between(0, 14).await.unwrap();

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, stay.rooms[0].id);
    assert_eq!(found[0].start, hotel.day(0));
    assert_eq!(found[0].reason, NeedsRoomReason::NoSingleRoom);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn long_and_short_stays_that_arrived_before_the_window_are_found_exactly_once(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let (hotel, plans) = Hotel::for_booking(opts, 3).await;
    let silva = hotel.guest(new_guest("Anula", "Silva")).await;
    // Three overlapping stays land in three different rooms.
    let long = hotel.dlx(&silva, &plans, 0, 40).await;
    let edge = hotel.dlx(&silva, &plans, 0, 31).await;
    let short = hotel.dlx(&silva, &plans, 0, 2).await;
    let rooms: Vec<Uuid> = hotel.all_rooms().await.into_iter().map(|room| room.id).collect();
    let ids = |window: &TapeWindow| {
        let mut ids: Vec<Uuid> = window.stays.iter().map(|stay| stay.id).collect();
        ids.sort();
        ids
    };
    let sorted = |mut ids: Vec<Uuid>| {
        ids.sort();
        ids
    };

    // A 40-night stay that arrived 35 days before the window is found through the long-stay branch.
    let window = hotel.window(&rooms, 35, 49).await.unwrap();
    assert_eq!(ids(&window), vec![long.rooms[0].id]);

    // A 31-night stay that arrived 30 days before the window and overlaps it is the short branch's edge; the
    // 40-night stay is in the window too, and each appears once.
    let window = hotel.window(&rooms, 30, 44).await.unwrap();
    assert_eq!(ids(&window), sorted(vec![long.rooms[0].id, edge.rooms[0].id]));

    // A 2-night stay that arrived 32 days before the window is not in it.
    let window = hotel.window(&rooms, 32, 46).await.unwrap();
    assert!(!ids(&window).contains(&short.rooms[0].id));
    assert_eq!(ids(&window), vec![long.rooms[0].id]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn blocks_that_started_before_the_window_are_found_by_start_and_length(_: PgPoolOptions, opts: PgConnectOptions) {
    let (hotel, _) = Hotel::for_booking(opts, 4).await;
    let rooms = hotel.all_rooms().await;
    let (long_room, edge_room, old_room, released_room) = (&rooms[0], &rooms[1], &rooms[2], &rooms[3]);
    // Window [35, 49): a 40-day block from day 0 overlaps it; so does a released block, and a 2-day block from
    // day 0 is long over. The 31-day edge block is added below.
    let long = hotel.block(long_room, 0, 40).await;
    let old = hotel.block(old_room, 0, 2).await;
    let released = hotel.block(released_room, 36, 40).await;
    let mut tx = hotel.tx().await;
    sqlx::query("update room_block set released_at = now() where id = $1")
        .bind(released)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let all: Vec<Uuid> = rooms.iter().map(|room| room.id).collect();

    let window = hotel.window(&all, 35, 49).await.unwrap();
    assert_eq!(window.blocks.iter().map(|block| block.id).collect::<Vec<_>>(), vec![long]);

    // A 31-day block starting 30 days before the window and overlapping it by one day: the short branch's edge.
    let edge = hotel.block(edge_room, 0, 31).await;
    let window = hotel.window(&all, 30, 44).await.unwrap();
    let mut found: Vec<Uuid> = window.blocks.iter().map(|block| block.id).collect();
    found.sort();
    let mut expected = vec![long, edge];
    expected.sort();
    assert_eq!(found, expected);
    assert!(!found.contains(&old) && !found.contains(&released));
}
