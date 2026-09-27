//! Property-based check of the inventory counters: random sequences of room and block changes must leave
//! `inventory_day` equal to a recomputation from rooms, blocks and reservations (`rooms::find_drift`).

mod common;

use common::Hotel;
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use rooms::{Block, BlockKind, NewBlock, NewRoom, Room, RoomChanges, RoomType};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

/// Sequences tried per run. Each run draws new ones; a failure prints the sequence that broke the counters.
const SEQUENCES: usize = 25;

#[derive(Debug, Clone)]
enum Op {
    CreateRoom { room_type: usize },
    SetActive { room: usize, active: bool },
    Retype { room: usize, room_type: usize },
    Block { room: usize, start: i64, days: i64, out_of_order: bool },
    Shorten { block: usize, to: i64 },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0..3usize).prop_map(|room_type| Op::CreateRoom { room_type }),
        (0..8usize, any::<bool>()).prop_map(|(room, active)| Op::SetActive { room, active }),
        (0..8usize, 0..3usize).prop_map(|(room, room_type)| Op::Retype { room, room_type }),
        (0..8usize, 0..40i64, 1..20i64, any::<bool>()).prop_map(|(room, start, days, out_of_order)| Op::Block {
            room,
            start,
            days,
            out_of_order
        }),
        (0..8usize, 0..40i64).prop_map(|(block, to)| Op::Shorten { block, to }),
    ]
}

/// What one sequence has created so far, so operations can refer to rooms and blocks by index.
struct Sequence<'a> {
    hotel: &'a Hotel,
    types: &'a [RoomType],
    name: usize,
    rooms: Vec<Room>,
    blocks: Vec<Block>,
}

impl Sequence<'_> {
    /// Applies `op` in its own transaction. Operations the rules refuse (an overlapping block, a room that is
    /// inactive) are rolled back and skipped, as they would be for a user.
    async fn apply(&mut self, op: &Op) {
        let hotel = self.hotel;
        let mut tx = hotel.tx().await;
        let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);
        let applied = match *op {
            Op::CreateRoom { room_type } => {
                let number = format!("{}-{}", self.name, self.rooms.len());
                let input = NewRoom { room_type_id: self.types[room_type].id, number, floor: None, section_id: None };
                rooms::create_room(&mut tx, tenant, user, property, input)
                    .await
                    .map(|room| self.rooms.push(room))
                    .is_ok()
            }
            Op::SetActive { room, active } => {
                let Some(current) = self.rooms.get(room).cloned() else { return };
                let changes = RoomChanges { active: Some(active), ..RoomChanges::default() };
                rooms::update_room(&mut tx, tenant, user, property, current.id, current.version, changes)
                    .await
                    .map(|updated| self.rooms[room] = updated)
                    .is_ok()
            }
            Op::Retype { room, room_type } => {
                let Some(current) = self.rooms.get(room).cloned() else { return };
                let changes = RoomChanges { room_type_id: Some(self.types[room_type].id), ..RoomChanges::default() };
                rooms::update_room(&mut tx, tenant, user, property, current.id, current.version, changes)
                    .await
                    .map(|updated| self.rooms[room] = updated)
                    .is_ok()
            }
            Op::Block { room, start, days, out_of_order } => {
                let Some(current) = self.rooms.get(room) else { return };
                let reason = hotel.reason("OTHER").await;
                let input = NewBlock {
                    room_id: current.id,
                    from: hotel.day(start),
                    to: hotel.day(start + days),
                    kind: if out_of_order { BlockKind::OutOfOrder } else { BlockKind::OutOfService },
                    reason_id: reason.id,
                    note: String::new(),
                };
                rooms::create_block(&mut tx, tenant, user, property, input)
                    .await
                    .map(|block| self.blocks.push(block))
                    .is_ok()
            }
            Op::Shorten { block, to } => {
                let Some(current) = self.blocks.get(block).cloned() else { return };
                rooms::shorten_block(&mut tx, tenant, user, property, current.id, current.version, hotel.day(to))
                    .await
                    .map(|shortened| self.blocks[block] = shortened)
                    .is_ok()
            }
        };
        if applied {
            tx.commit().await.unwrap();
        } else {
            tx.rollback().await.unwrap();
        }
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn counters_match_a_recount_after_any_sequence_of_changes(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let types = [hotel.room_type("A").await, hotel.room_type("B").await, hotel.room_type("C").await];
    let sequences = proptest::collection::vec(op(), 1..30);
    let mut runner = TestRunner::default();

    for name in 0..SEQUENCES {
        let ops = sequences.new_tree(&mut runner).unwrap().current();
        let mut sequence = Sequence { hotel: &hotel, types: &types, name, rooms: Vec::new(), blocks: Vec::new() };
        for (step, op) in ops.iter().enumerate() {
            sequence.apply(op).await;
            let drift = hotel.drift().await;
            assert!(drift.is_empty(), "counters drifted after step {step} of {ops:?}: {drift:?}");
        }
    }
}

/// Retype pairs run at once, each room moving to the other's type while it has an out-of-order block. Each
/// pair also shortens a block in one type and creates a block in the other, on rooms that stay put.
const OPPOSITE_RETYPES: usize = 30;

/// One command in [`opposite_retypes_and_block_changes_run_concurrently_without_deadlocking`].
enum Change {
    Retype(Room, uuid::Uuid),
    Shorten(Block, time::Date),
    Block(NewBlock),
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn opposite_retypes_and_block_changes_run_concurrently_without_deadlocking(
    _: PgPoolOptions,
    opts: PgConnectOptions,
) {
    let hotel = Hotel::new(opts.clone()).await;
    let (a, b) = (hotel.room_type("A").await, hotel.room_type("B").await);
    let reason = hotel.reason("OTHER").await;
    let out_of_order = |room: &Room, start: i64| NewBlock {
        room_id: room.id,
        from: hotel.day(start),
        to: hotel.day(start + 10),
        kind: BlockKind::OutOfOrder,
        reason_id: reason.id,
        note: String::new(),
    };
    let block = |input: NewBlock| async {
        let mut tx = hotel.tx().await;
        let block = rooms::create_block(&mut tx, hotel.tenant, hotel.user, hotel.property, input).await.unwrap();
        tx.commit().await.unwrap();
        block
    };
    let mut changes = Vec::new();
    for pair in 0..OPPOSITE_RETYPES {
        let start = i64::try_from(pair).unwrap();
        for (from, to, side) in [(a.id, b.id, "a"), (b.id, a.id, "b")] {
            let room = hotel.room(from, &format!("{side}{pair}")).await;
            block(out_of_order(&room, start)).await;
            changes.push(Change::Retype(room, to));
        }
        let shortened = hotel.room(a.id, &format!("c{pair}")).await;
        changes.push(Change::Shorten(block(out_of_order(&shortened, start)).await, hotel.day(start + 5)));
        let blocked = hotel.room(b.id, &format!("d{pair}")).await;
        changes.push(Change::Block(out_of_order(&blocked, start)));
    }

    let pool = db::testing::app_pool(opts, 16).await;
    let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);
    let total = changes.len();
    let mut tasks = tokio::task::JoinSet::new();
    for change in changes {
        let pool = pool.clone();
        tasks.spawn(async move {
            let mut tx = db::begin(&pool, db::Scope::tenant(tenant)).await.unwrap();
            match change {
                Change::Retype(room, to) => {
                    let changes = RoomChanges { room_type_id: Some(to), ..RoomChanges::default() };
                    rooms::update_room(&mut tx, tenant, user, property, room.id, room.version, changes).await?;
                }
                Change::Shorten(block, to) => {
                    rooms::shorten_block(&mut tx, tenant, user, property, block.id, block.version, to).await?;
                }
                Change::Block(input) => {
                    rooms::create_block(&mut tx, tenant, user, property, input).await?;
                }
            }
            tx.commit().await?;
            Ok::<_, rooms::RoomsError>(())
        });
    }
    let failures: Vec<String> =
        tasks.join_all().await.into_iter().filter_map(|result| result.err().map(|err| err.to_string())).collect();

    assert!(failures.is_empty(), "{} of {total} changes failed: {failures:?}", failures.len());
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn sold_rooms_without_a_reservation_are_drift(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let room_type = hotel.room_type("A").await;
    hotel.room(room_type.id, "101").await;
    let mut tx = hotel.tx().await;
    sqlx::query("update inventory_day set sold = 1 where room_type_id = $1 and date = $2")
        .bind(room_type.id)
        .bind(hotel.day(3))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let drift = hotel.drift().await;

    assert_eq!(
        drift,
        vec![rooms::InventoryDrift {
            room_type_id: room_type.id,
            date: hotel.day(3),
            expected_physical: Some(1),
            actual_physical: Some(1),
            expected_sold: Some(0),
            actual_sold: Some(1),
            expected_out_of_order: Some(0),
            actual_out_of_order: Some(0),
        }]
    );
}
