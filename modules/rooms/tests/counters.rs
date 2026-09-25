//! Property-based check of the inventory counters: random sequences of room and block changes must leave
//! `inventory_day` equal to a recomputation from rooms and blocks (`rooms::find_drift`).

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
        match *op {
            Op::CreateRoom { room_type } => {
                let number = format!("{}-{}", self.name, self.rooms.len());
                let input = NewRoom { room_type_id: self.types[room_type].id, number, floor: None, section_id: None };
                if let Ok(room) = rooms::create_room(&mut tx, tenant, user, property, input).await {
                    self.rooms.push(room);
                }
            }
            Op::SetActive { room, active } => {
                let Some(current) = self.rooms.get(room).cloned() else { return };
                let changes = RoomChanges { active: Some(active), ..RoomChanges::default() };
                if let Ok(updated) =
                    rooms::update_room(&mut tx, tenant, user, property, current.id, current.version, changes).await
                {
                    self.rooms[room] = updated;
                }
            }
            Op::Retype { room, room_type } => {
                let Some(current) = self.rooms.get(room).cloned() else { return };
                let changes = RoomChanges { room_type_id: Some(self.types[room_type].id), ..RoomChanges::default() };
                if let Ok(updated) =
                    rooms::update_room(&mut tx, tenant, user, property, current.id, current.version, changes).await
                {
                    self.rooms[room] = updated;
                }
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
                if let Ok(block) = rooms::create_block(&mut tx, tenant, user, property, input).await {
                    self.blocks.push(block);
                }
            }
            Op::Shorten { block, to } => {
                let Some(current) = self.blocks.get(block).cloned() else { return };
                if let Ok(shortened) =
                    rooms::shorten_block(&mut tx, tenant, user, property, current.id, current.version, hotel.day(to))
                        .await
                {
                    self.blocks[block] = shortened;
                }
            }
        }
        tx.commit().await.unwrap();
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
