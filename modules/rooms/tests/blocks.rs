mod common;

use common::Hotel;
use rooms::{
    Block, BlockKind, BlockReasonChanges, NewBlock, NewBlockReason, Room, RoomChanges, RoomsError, WINDOW_DAYS,
};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

impl Hotel {
    /// Blocks `room` for `[business date + from, business date + to)`.
    async fn block(&self, room: &Room, from: i64, to: i64, kind: BlockKind) -> Result<Block, RoomsError> {
        let reason = self.reason("MAINTENANCE").await;
        let input = NewBlock {
            room_id: room.id,
            from: self.day(from),
            to: self.day(to),
            kind,
            reason_id: reason.id,
            note: "Leaking pipe".into(),
        };
        let mut tx = self.tx().await;
        let block = rooms::create_block(&mut tx, self.tenant, self.user, self.property, input).await?;
        tx.commit().await.unwrap();
        Ok(block)
    }

    async fn shorten(&self, block: &Block, to: i64) -> Result<Block, RoomsError> {
        let mut tx = self.tx().await;
        let shortened =
            rooms::shorten_block(&mut tx, self.tenant, self.user, self.property, block.id, block.version, self.day(to))
                .await?;
        tx.commit().await.unwrap();
        Ok(shortened)
    }

    /// The days (as offsets from the business date) on which `room_type` has rooms out of order.
    async fn out_of_order_days(&self, room_type: Uuid) -> Vec<i64> {
        let counters = self.counters(room_type).await;
        (0..WINDOW_DAYS).zip(counters).filter(|(_, day)| day.out_of_order > 0).map(|(offset, _)| offset).collect()
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_property_starts_with_the_default_block_reasons(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut tx = hotel.tx().await;

    let reasons = rooms::list_block_reasons(&mut tx, hotel.property).await.unwrap();

    let codes: Vec<(&str, BlockKind)> = reasons.iter().map(|r| (r.code.as_str(), r.default_kind)).collect();
    assert_eq!(
        codes,
        [
            ("CONSTRUCTION", BlockKind::OutOfOrder),
            ("DEEP_CLEAN", BlockKind::OutOfService),
            ("MAINTENANCE", BlockKind::OutOfOrder),
            ("OTHER", BlockKind::OutOfOrder),
            ("RENOVATION", BlockKind::OutOfOrder),
        ]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_out_of_order_block_reduces_availability_on_its_days_only(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(dlx.id, "101").await;
    hotel.room(dlx.id, "102").await;

    let block = hotel.block(&room, 2, 5, BlockKind::OutOfOrder).await.unwrap();

    assert_eq!((block.from, block.to, block.version, block.released), (hotel.day(2), hotel.day(5), 1, false));
    assert_eq!(hotel.out_of_order_days(dlx.id).await, vec![2, 3, 4]);
    let counters = hotel.counters(dlx.id).await;
    assert_eq!(counters[2].available(), 1);
    assert_eq!(counters[5].available(), 2);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_out_of_service_block_leaves_availability_alone(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(dlx.id, "101").await;

    hotel.block(&room, 0, 3, BlockKind::OutOfService).await.unwrap();

    assert_eq!(hotel.out_of_order_days(dlx.id).await, Vec::<i64>::new());
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn overlapping_blocks_conflict_and_name_the_block_in_the_way(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(dlx.id, "101").await;
    let first = hotel.block(&room, 2, 5, BlockKind::OutOfOrder).await.unwrap();

    let overlapping = hotel.block(&room, 4, 6, BlockKind::OutOfService).await;
    let adjacent = hotel.block(&room, 5, 7, BlockKind::OutOfOrder).await;

    let Err(RoomsError::Overlap(conflicts)) = overlapping else { panic!("expected an overlap, got {overlapping:?}") };
    assert_eq!(conflicts, vec![first]);
    assert!(adjacent.is_ok(), "[5, 7) starts the day [2, 5) ends: {adjacent:?}");
    assert_eq!(hotel.out_of_order_days(dlx.id).await, vec![2, 3, 4, 5, 6]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn blocks_start_on_or_after_the_business_date_and_end_after_they_start(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(dlx.id, "101").await;

    let in_the_past = hotel.block(&room, -1, 2, BlockKind::OutOfOrder).await;
    let empty = hotel.block(&room, 3, 3, BlockKind::OutOfOrder).await;

    assert!(matches!(in_the_past, Err(RoomsError::Invalid(_))), "{in_the_past:?}");
    assert!(matches!(empty, Err(RoomsError::Invalid(_))), "{empty:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn blocks_end_within_the_counter_window(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(dlx.id, "101").await;
    let other = hotel.room(dlx.id, "102").await;

    let too_long = hotel.block(&room, 0, WINDOW_DAYS + 1, BlockKind::OutOfOrder).await;
    let to_the_end = hotel.block(&other, WINDOW_DAYS - 1, WINDOW_DAYS, BlockKind::OutOfOrder).await;

    let Err(RoomsError::Invalid(message)) = too_long else { panic!("expected Invalid, got {too_long:?}") };
    assert_eq!(message, "a block can end at most 730 days after the business date");
    assert!(to_the_end.is_ok(), "{to_the_end:?}");
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn releasing_early_restores_the_remaining_days(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(dlx.id, "101").await;
    let block = hotel.block(&room, 0, 10, BlockKind::OutOfOrder).await.unwrap();

    let released = hotel.shorten(&block, 4).await.unwrap();
    let stale = hotel.shorten(&block, 2).await;
    let longer = hotel.shorten(&released, 6).await;

    assert_eq!(
        (released.from, released.to, released.version, released.released),
        (hotel.day(0), hotel.day(4), 2, false)
    );
    assert_eq!(hotel.out_of_order_days(dlx.id).await, vec![0, 1, 2, 3]);
    assert!(matches!(stale, Err(RoomsError::VersionMismatch("block"))), "{stale:?}");
    assert!(matches!(longer, Err(RoomsError::Invalid(_))), "{longer:?}");
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn cancelling_before_the_start_frees_every_day(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(dlx.id, "101").await;
    let block = hotel.block(&room, 5, 8, BlockKind::OutOfOrder).await.unwrap();

    let cancelled = hotel.shorten(&block, 1).await.unwrap();
    let again = hotel.block(&room, 5, 8, BlockKind::OutOfOrder).await;
    let mut tx = hotel.tx().await;
    let listed = rooms::list_blocks(&mut tx, hotel.property, hotel.day(0), hotel.day(30)).await.unwrap();

    assert!(cancelled.released);
    assert_eq!((cancelled.from, cancelled.to), (hotel.day(5), hotel.day(8)));
    let again = again.unwrap();
    assert_eq!(listed, vec![again]);
    assert_eq!(hotel.out_of_order_days(dlx.id).await, vec![5, 6, 7]);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_blocked_room_takes_its_blocks_along_when_retyped_deactivated_or_reactivated(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let hotel = Hotel::new(opts).await;
    let std = hotel.room_type("STD").await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(std.id, "101").await;
    hotel.block(&room, 1, 3, BlockKind::OutOfOrder).await.unwrap();

    let mut tx = hotel.tx().await;
    let retype = RoomChanges { room_type_id: Some(dlx.id), ..RoomChanges::default() };
    let retyped =
        rooms::update_room(&mut tx, hotel.tenant, hotel.user, hotel.property, room.id, 1, retype).await.unwrap();
    tx.commit().await.unwrap();
    let moved = (hotel.out_of_order_days(std.id).await, hotel.out_of_order_days(dlx.id).await);
    let mut tx = hotel.tx().await;
    let deactivate = RoomChanges { active: Some(false), ..RoomChanges::default() };
    let deactivated =
        rooms::update_room(&mut tx, hotel.tenant, hotel.user, hotel.property, room.id, retyped.version, deactivate)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    let while_inactive = hotel.out_of_order_days(dlx.id).await;
    let mut tx = hotel.tx().await;
    let reactivate = RoomChanges { active: Some(true), ..RoomChanges::default() };
    rooms::update_room(&mut tx, hotel.tenant, hotel.user, hotel.property, room.id, deactivated.version, reactivate)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(moved, (vec![], vec![1, 2]));
    assert_eq!(while_inactive, Vec::<i64>::new());
    assert_eq!(hotel.out_of_order_days(dlx.id).await, vec![1, 2]);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn inactive_rooms_and_unknown_reasons_cannot_be_blocked(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(dlx.id, "101").await;
    let mut tx = hotel.tx().await;
    let deactivate = RoomChanges { active: Some(false), ..RoomChanges::default() };
    let inactive =
        rooms::update_room(&mut tx, hotel.tenant, hotel.user, hotel.property, room.id, 1, deactivate).await.unwrap();
    tx.commit().await.unwrap();
    let active = hotel.room(dlx.id, "102").await;

    let on_inactive = hotel.block(&inactive, 0, 2, BlockKind::OutOfOrder).await;
    let unknown_reason = NewBlock {
        room_id: active.id,
        from: hotel.day(0),
        to: hotel.day(2),
        kind: BlockKind::OutOfOrder,
        reason_id: Uuid::now_v7(),
        note: String::new(),
    };
    let mut tx = hotel.tx().await;
    let unknown_reason = rooms::create_block(&mut tx, hotel.tenant, hotel.user, hotel.property, unknown_reason).await;

    assert!(matches!(on_inactive, Err(RoomsError::Invalid(_))), "{on_inactive:?}");
    assert!(matches!(unknown_reason, Err(RoomsError::Invalid(_))), "{unknown_reason:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_property_adds_and_retires_its_own_block_reasons(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(dlx.id, "101").await;
    let pest =
        NewBlockReason { code: "PEST".into(), label: "Pest control".into(), default_kind: BlockKind::OutOfOrder };

    let mut tx = hotel.tx().await;
    let created =
        rooms::create_block_reason(&mut tx, hotel.tenant, hotel.user, hotel.property, pest.clone()).await.unwrap();
    let retire =
        BlockReasonChanges { active: Some(false), label: Some("Pests".into()), ..BlockReasonChanges::default() };
    let retired = rooms::update_block_reason(&mut tx, hotel.tenant, hotel.user, hotel.property, created.id, 1, retire)
        .await
        .unwrap();
    let stale = rooms::update_block_reason(
        &mut tx,
        hotel.tenant,
        hotel.user,
        hotel.property,
        created.id,
        1,
        BlockReasonChanges::default(),
    )
    .await;
    tx.commit().await.unwrap();
    let duplicate =
        rooms::create_block_reason(&mut hotel.tx().await, hotel.tenant, hotel.user, hotel.property, pest).await;
    let use_retired = NewBlock {
        room_id: room.id,
        from: hotel.day(0),
        to: hotel.day(1),
        kind: BlockKind::OutOfOrder,
        reason_id: retired.id,
        note: String::new(),
    };
    let use_retired =
        rooms::create_block(&mut hotel.tx().await, hotel.tenant, hotel.user, hotel.property, use_retired).await;

    assert_eq!((retired.label.as_str(), retired.active, retired.version), ("Pests", false, 2));
    assert!(matches!(stale, Err(RoomsError::VersionMismatch("block reason"))), "{stale:?}");
    assert!(matches!(duplicate, Err(RoomsError::Conflict(_))), "{duplicate:?}");
    assert!(matches!(use_retired, Err(RoomsError::Invalid(_))), "{use_retired:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_is_not_blocked_on_the_nights_a_stay_is_assigned_to_it(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let (room, occupied) = (hotel.room(dlx.id, "101").await, hotel.room(dlx.id, "102").await);
    let confirmed = hotel.assigned_stay(&room, 2, 5, "confirmed").await;
    hotel.assigned_stay(&room, 6, 8, "cancelled").await;
    hotel.assigned_stay(&room, 8, 10, "no_show").await;
    let checked_in = hotel.assigned_stay(&occupied, -1, 2, "checked_in").await;

    let overlapping = hotel.block(&room, 4, 6, BlockKind::OutOfService).await;
    let before_arrival = hotel.block(&room, 0, 3, BlockKind::OutOfOrder).await;
    let in_house = hotel.block(&occupied, 0, 1, BlockKind::OutOfOrder).await;
    // From the day the stay leaves, over a cancelled stay and a no-show.
    let after = hotel.block(&room, 5, 10, BlockKind::OutOfOrder).await;

    for (refused, confirmation, number) in
        [(overlapping, &confirmed, "101"), (before_arrival, &confirmed, "101"), (in_house, &checked_in, "102")]
    {
        let Err(RoomsError::Conflict(message)) = refused else { panic!("expected a conflict, got {refused:?}") };
        assert_eq!(message, format!("room {number} is assigned to {confirmation} on those nights"));
    }
    assert!(after.is_ok(), "{after:?}");
    assert_eq!(hotel.out_of_order_days(dlx.id).await, vec![5, 6, 7, 8, 9]);
}
