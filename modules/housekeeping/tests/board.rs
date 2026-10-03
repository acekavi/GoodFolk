mod common;

use common::Hotel;
use db::UserId;
use housekeeping::{BoardRoom, Condition, Occupancy, Severity, board, my_rooms, report_issue};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

impl Hotel {
    async fn board(&self) -> Vec<BoardRoom> {
        board(&mut self.tx().await, self.property).await.unwrap()
    }

    async fn my_rooms(&self, user: UserId) -> Vec<BoardRoom> {
        my_rooms(&mut self.tx().await, self.property, user).await.unwrap()
    }

    /// `room`'s occupancy today.
    async fn occupancy(&self, room: Uuid) -> Occupancy {
        self.board().await.into_iter().find(|row| row.room_id == room).unwrap().occupancy
    }
}

fn numbers(rooms: &[BoardRoom]) -> Vec<&str> {
    rooms.iter().map(|room| room.number.as_str()).collect()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn occupancy_follows_the_stays_of_the_business_date(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let rooms = hotel
        .rooms(&["vacant", "stay", "out", "in", "turn", "early", "late", "same", "gone", "past", "next", "day", "here"])
        .await;
    let [vacant, stay, out, due_in, turnover, early, late, same, gone, past, next, day, here] = rooms[..] else {
        panic!()
    };

    hotel.stay(stay, -2, 2, "checked_in").await;
    hotel.stay(out, -1, 0, "checked_in").await;
    hotel.stay(due_in, 0, 3, "confirmed").await;
    hotel.stay(turnover, -2, 0, "checked_in").await;
    hotel.stay(turnover, 0, 2, "confirmed").await;
    // Checked out early: the stay was shortened to end before today, or today itself.
    hotel.stay(early, -3, -1, "checked_out").await;
    hotel.stay(late, -3, 0, "checked_out").await;
    // A stay that arrives and leaves today does not exist, so same-day use is two stays.
    hotel.stay(same, -1, 0, "confirmed").await;
    hotel.stay(same, 0, 1, "confirmed").await;
    hotel.stay(gone, -1, 2, "cancelled").await;
    hotel.stay(gone, 0, 2, "no_show").await;
    hotel.stay(past, -9, -4, "checked_out").await;
    hotel.stay(next, 1, 3, "confirmed").await;
    // Checked out on its arrival day, and arrived and checked in today.
    hotel.stay(day, 0, 1, "checked_out").await;
    hotel.stay(here, 0, 2, "checked_in").await;

    let expected = [
        (vacant, Occupancy::Vacant),
        (stay, Occupancy::StayOver),
        (out, Occupancy::DueOut),
        (due_in, Occupancy::DueIn),
        (turnover, Occupancy::Turnover),
        (early, Occupancy::Vacant),
        (late, Occupancy::Vacant),
        (same, Occupancy::Turnover),
        (gone, Occupancy::Vacant),
        (past, Occupancy::Vacant),
        (next, Occupancy::Vacant),
        (day, Occupancy::Vacant),
        (here, Occupancy::StayOver),
    ];
    let board = hotel.board().await;
    for (room, occupancy) in expected {
        let row = board.iter().find(|row| row.room_id == room).unwrap();
        assert_eq!(row.occupancy, occupancy, "room {}", row.number);
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_checked_out_stay_still_in_range_keeps_the_room_occupied(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let room = hotel.rooms(&["101"]).await[0];
    // Only reachable if a stay is checked out before its end is shrunk: it counts until the business date reaches it.
    hotel.stay(room, -1, 2, "checked_out").await;
    assert_eq!(hotel.occupancy(room).await, Occupancy::StayOver);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn long_stays_count_too(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let rooms = hotel.rooms(&["101", "102"]).await;
    hotel.stay(rooms[0], -100, 100, "checked_in").await;
    hotel.stay(rooms[1], -40, 0, "checked_in").await;
    assert_eq!(hotel.occupancy(rooms[0]).await, Occupancy::StayOver);
    assert_eq!(hotel.occupancy(rooms[1]).await, Occupancy::DueOut);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn stays_and_blocks_at_the_short_long_split_are_found(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let rooms = hotel.rooms(&["a31", "a32", "d31", "d32", "block31", "block32"]).await;
    // In house on the last night of a 31 and a 32 night stay, and due out when they end today.
    hotel.stay(rooms[0], -30, 1, "checked_in").await;
    hotel.stay(rooms[1], -31, 1, "checked_in").await;
    hotel.stay(rooms[2], -31, 0, "checked_in").await;
    hotel.stay(rooms[3], -32, 0, "checked_in").await;
    // A block covering today on the first day it exists, either side of the split.
    hotel.block(rooms[4], -30, 1, "out_of_order").await;
    hotel.block(rooms[5], -31, 1, "out_of_service").await;

    let board = hotel.board().await;
    let occupancy: Vec<_> = board.iter().map(|room| room.occupancy).collect();
    assert_eq!(
        occupancy,
        [
            Occupancy::StayOver,
            Occupancy::StayOver,
            Occupancy::DueOut,
            Occupancy::DueOut,
            Occupancy::Vacant,
            Occupancy::Vacant
        ]
    );
    assert_eq!(board[4].block_kind, Some(rooms::BlockKind::OutOfOrder));
    assert_eq!(board[5].block_kind, Some(rooms::BlockKind::OutOfService));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_board_has_each_rooms_condition_assignee_issues_and_block(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let rooms = hotel.rooms(&["101", "102", "103", "104"]).await;
    let ann = hotel.member(Some("housekeeping"), Some(hotel.property)).await;
    hotel.assign(rooms[0], ann).await;
    hotel.block(rooms[1], 0, 3, "out_of_order").await;
    hotel.block(rooms[2], 1, 3, "out_of_service").await;
    {
        let mut tx = hotel.tx().await;
        for (room, severity) in [(rooms[0], Severity::Low), (rooms[0], Severity::High)] {
            report_issue(&mut tx, hotel.tenant, hotel.user, hotel.property, room, severity, "Broken".into())
                .await
                .unwrap();
        }
        sqlx::query("update room_issue set status = 'resolved', resolved_at = now() where severity = 'low'")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("update room_condition set condition = 'dirty', version = 4 where room_id = $1")
            .bind(rooms[3])
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    let board = hotel.board().await;
    let [first, second, third, fourth] = &board[..] else { panic!("{} rooms", board.len()) };
    assert_eq!((first.open_issues, second.open_issues), (1, 0), "a resolved issue is not counted");
    let assignee = first.assignee.as_ref().unwrap();
    assert_eq!((assignee.user_id, assignee.display_name.as_str()), (ann.0, "U"));
    assert_eq!(second.assignee, None);
    assert_eq!(first.block_kind, None);
    assert_eq!(second.block_kind, Some(rooms::BlockKind::OutOfOrder));
    assert_eq!(third.block_kind, None, "a block starting tomorrow does not cover today");
    assert_eq!((first.condition, first.condition_version), (Condition::Clean, 1));
    assert_eq!((fourth.condition, fourth.condition_version), (Condition::Dirty, 4));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn only_active_rooms_appear_and_released_blocks_do_not_show(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let rooms = hotel.rooms(&["101", "102"]).await;
    hotel.block(rooms[0], 0, 3, "out_of_order").await;
    let mut tx = hotel.tx().await;
    sqlx::query("update room_block set released_at = now()").execute(&mut *tx).await.unwrap();
    sqlx::query("update room set active = false where id = $1").bind(rooms[1]).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();

    let board = hotel.board().await;
    assert_eq!(numbers(&board), ["101"]);
    assert_eq!(board[0].block_kind, None);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_without_a_condition_row_still_shows_as_clean(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let room = hotel.rooms(&["101"]).await[0];
    let mut tx = hotel.tx().await;
    sqlx::query("delete from room_condition where room_id = $1").bind(room).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();

    let board = hotel.board().await;
    assert_eq!((board[0].condition, board[0].condition_version), (Condition::Clean, 0));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_board_follows_the_rail_order(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let rooms = hotel.rooms(&["101", "A1", "99", "B2", "100", "7"]).await;
    assert_eq!(numbers(&hotel.board().await), ["101", "A1", "99", "B2", "100", "7"], "display order first");

    let mut tx = hotel.tx().await;
    sqlx::query("update room set sort_order = 0").execute(&mut *tx).await.unwrap();
    sqlx::query("update room set sort_order = -1 where id = $1").bind(rooms[3]).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        numbers(&hotel.board().await),
        ["B2", "7", "99", "100", "101", "A1"],
        "then digits-only numbers by value, then the rest as text"
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn my_rooms_are_the_users_own_in_cleaning_order(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let rooms = hotel.rooms(&["101", "102", "103", "104", "105", "106", "107"]).await;
    let ann = hotel.member(Some("housekeeping"), Some(hotel.property)).await;
    let bob = hotel.member(Some("housekeeping"), Some(hotel.property)).await;

    hotel.stay(rooms[1], -1, 0, "checked_in").await;
    hotel.stay(rooms[2], -1, 1, "checked_in").await;
    hotel.stay(rooms[3], 0, 1, "confirmed").await;
    hotel.stay(rooms[4], -1, 0, "checked_in").await;
    hotel.stay(rooms[4], 0, 2, "confirmed").await;
    hotel.stay(rooms[6], -1, 0, "checked_in").await;
    for room in &rooms[..6] {
        hotel.assign(*room, ann).await;
    }
    hotel.assign(rooms[6], bob).await;

    let mine = hotel.my_rooms(ann).await;
    assert_eq!(numbers(&mine), ["105", "102", "103", "104", "101", "106"]);
    assert_eq!(mine[0].occupancy, Occupancy::Turnover);
    assert_eq!(numbers(&hotel.my_rooms(bob).await), ["107"], "another user's assignment is not mine");
    assert!(hotel.my_rooms(hotel.user).await.is_empty());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn assignments_of_other_days_do_not_show(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let room = hotel.rooms(&["101"]).await[0];
    let ann = hotel.member(Some("housekeeping"), Some(hotel.property)).await;
    let mut tx = hotel.tx().await;
    sqlx::query(
        "insert into hk_assignment (tenant_id, property_id, business_date, room_id, user_id)
         values ($1, $2, $3 - 1, $4, $5)",
    )
    .bind(hotel.tenant.0)
    .bind(hotel.property)
    .bind(hotel.business_date)
    .bind(room)
    .bind(ann.0)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    assert!(hotel.my_rooms(ann).await.is_empty());
    assert_eq!(hotel.board().await[0].assignee, None);
}
