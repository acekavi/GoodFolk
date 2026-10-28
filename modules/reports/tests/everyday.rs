//! The everyday reports over stored statistics (`audit::store`) and the daily lists over stays seeded straight into
//! the reservation tables.

mod common;

use audit::BreakdownRow;
use common::{Hotel, date};
use reports::{BreakdownTotal, Dimension, Headline, ListKind, RangeKind, ReportsError};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn headline_is_ratio_of_sums(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.set_business_date(date(2027, 3, 1)).await;
    hotel.close_day(date(2027, 2, 1), 20, 10, 10 * 1_000, vec![]).await;
    hotel.close_day(date(2027, 2, 2), 20, 20, 20 * 3_000, vec![]).await;

    let summary =
        reports::summary(&mut hotel.tx().await, hotel.property, date(2027, 2, 1), date(2027, 2, 2)).await.unwrap();

    // 30 of 40 rooms sold: not the 5 000 and 10 000 of the days averaged.
    assert_eq!(
        summary.headline,
        Headline { occupancy_bp: 7_500, adr: 2_333, revpar: 1_750, room_nights: 30, total_revenue: 70_000 }
    );
    assert_eq!(summary.days.len(), 2);
    assert!(summary.not_closed.is_empty());
    assert_eq!((summary.closed_from, summary.closed_to), (Some(date(2027, 2, 1)), Some(date(2027, 2, 2))));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn open_days_are_listed_not_estimated(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.set_business_date(date(2027, 2, 3)).await;
    hotel.close_day(date(2027, 2, 1), 20, 10, 10_000, vec![]).await;
    hotel.close_day(date(2027, 2, 2), 20, 10, 10_000, vec![]).await;

    // 31 January is before the property's first close; 3 to 5 February are not closed yet.
    let summary =
        reports::summary(&mut hotel.tx().await, hotel.property, date(2027, 1, 31), date(2027, 2, 5)).await.unwrap();

    assert_eq!(summary.not_closed, [date(2027, 1, 31), date(2027, 2, 3), date(2027, 2, 4), date(2027, 2, 5)]);
    assert_eq!((summary.closed_from, summary.closed_to), (Some(date(2027, 2, 1)), Some(date(2027, 2, 2))));
    assert_eq!(summary.headline.room_nights, 20);
    assert_eq!(summary.headline.occupancy_bp, 5_000);
    assert_eq!(summary.days.len(), 2);

    let none =
        reports::summary(&mut hotel.tx().await, hotel.property, date(2027, 2, 3), date(2027, 2, 4)).await.unwrap();
    assert_eq!((none.closed_from, none.closed_to), (None, None));
    assert_eq!(none.headline, Headline { occupancy_bp: 0, adr: 0, revpar: 0, room_nights: 0, total_revenue: 0 });
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn last_year_only_when_fully_closed(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.set_business_date(date(2027, 6, 1)).await;
    hotel.close_days(date(2027, 3, 10), date(2027, 3, 12), 5, 5_000).await;
    hotel.close_days(date(2026, 3, 10), date(2026, 3, 11), 8, 8_000).await;
    let range = (date(2027, 3, 10), date(2027, 3, 12));

    let partial = reports::summary(&mut hotel.tx().await, hotel.property, range.0, range.1).await.unwrap();
    assert_eq!(partial.last_year, None);

    hotel.close_day(date(2026, 3, 12), 10, 8, 8_000, vec![]).await;
    let full = reports::summary(&mut hotel.tx().await, hotel.property, range.0, range.1).await.unwrap();
    assert_eq!(
        full.last_year,
        Some(Headline { occupancy_bp: 8_000, adr: 1_000, revpar: 800, room_nights: 24, total_revenue: 24_000 })
    );
}

#[test]
fn week_compares_364_days_back() {
    let (from, to) = (date(2027, 3, 1), date(2027, 3, 7));

    assert_eq!(RangeKind::of(from, to), RangeKind::Week);
    assert_eq!(reports::last_year_range(RangeKind::Week, from, to), Some((date(2026, 3, 2), date(2026, 3, 8))));
}

#[test]
fn month_compares_same_month() {
    let (from, to) = (date(2028, 2, 1), date(2028, 2, 29));

    assert_eq!(RangeKind::of(from, to), RangeKind::Month);
    assert_eq!(reports::last_year_range(RangeKind::Month, from, to), Some((date(2027, 2, 1), date(2027, 2, 28))));
    assert_eq!(
        reports::last_year_range(RangeKind::Month, date(2027, 3, 1), date(2027, 3, 31)),
        Some((date(2026, 3, 1), date(2026, 3, 31)))
    );
}

#[test]
fn feb29_custom_range_has_no_counterpart() {
    let from = date(2028, 2, 20);

    assert_eq!(RangeKind::of(from, date(2028, 3, 5)), RangeKind::Custom);
    assert_eq!(
        reports::last_year_range(RangeKind::Custom, from, date(2028, 3, 5)),
        Some((date(2027, 2, 20), date(2027, 3, 5)))
    );
    assert_eq!(reports::last_year_range(RangeKind::Custom, date(2028, 2, 29), date(2028, 3, 5)), None);
    assert_eq!(reports::last_year_range(RangeKind::Custom, from, date(2028, 2, 29)), None);
}

fn row(dimension: &str, key: &str, room_nights: i32, room_revenue: i64, revenue: i64) -> BreakdownRow {
    BreakdownRow { dimension: dimension.into(), key: key.into(), room_nights, room_revenue, revenue }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn breakdowns_sum_over_range(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.set_business_date(date(2027, 3, 1)).await;
    let day_one = vec![
        row("segment", "ibe", 2, 2_000, 2_500),
        row("segment", "ota", 1, 900, 900),
        row("outlet", "BAR", 0, 0, 300),
    ];
    let day_two = vec![row("segment", "ibe", 3, 3_000, 3_200), row("room_type", "DLX", 3, 3_000, 3_200)];
    hotel.close_day(date(2027, 2, 1), 10, 3, 2_900, day_one).await;
    hotel.close_day(date(2027, 2, 2), 10, 3, 3_000, day_two).await;
    // Outside the range.
    hotel.close_day(date(2027, 2, 3), 10, 1, 1_000, vec![row("segment", "ibe", 1, 1_000, 1_000)]).await;

    let summary =
        reports::summary(&mut hotel.tx().await, hotel.property, date(2027, 2, 1), date(2027, 2, 2)).await.unwrap();

    let total = |key: &str, room_nights, room_revenue, revenue| BreakdownTotal {
        key: key.into(),
        room_nights,
        room_revenue,
        revenue,
    };
    assert_eq!(summary.breakdowns[&Dimension::Segment], [total("ibe", 5, 5_000, 5_700), total("ota", 1, 900, 900)]);
    assert_eq!(summary.breakdowns[&Dimension::Outlet], [total("BAR", 0, 0, 300)]);
    assert_eq!(summary.breakdowns[&Dimension::RoomType], [total("DLX", 3, 3_000, 3_200)]);
    assert!(!summary.breakdowns.contains_key(&Dimension::Source));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn range_over_366_days_is_invalid(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut tx = hotel.tx().await;

    let ok = reports::summary(&mut tx, hotel.property, date(2027, 1, 1), date(2027, 12, 31)).await;
    assert!(ok.is_ok(), "{ok:?}");
    let too_long = reports::summary(&mut tx, hotel.property, date(2027, 1, 1), date(2028, 1, 2)).await.unwrap_err();
    assert!(matches!(too_long, ReportsError::Invalid(_)), "{too_long:?}");
    let backwards = reports::summary(&mut tx, hotel.property, date(2027, 1, 2), date(2027, 1, 1)).await.unwrap_err();
    assert!(matches!(backwards, ReportsError::Invalid(_)), "{backwards:?}");
    let other = reports::summary(&mut tx, uuid::Uuid::now_v7(), date(2027, 1, 1), date(2027, 1, 2)).await.unwrap_err();
    assert!(matches!(other, ReportsError::NotFound("property")), "{other:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn daily_lists_hold_the_stays_of_the_day(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let today = date(2027, 3, 10);
    hotel.set_business_date(today).await;
    let (yesterday, tomorrow) = (date(2027, 3, 9), date(2027, 3, 11));
    let arriving = hotel.stay("confirmed", today, tomorrow, Some("102"), "Ada", "Silva").await;
    let arrived_early = hotel.stay("checked_in", yesterday, date(2027, 3, 12), Some("101"), "Bo", "Perera").await;
    let leaving = hotel.stay("checked_in", date(2027, 3, 8), today, Some("103"), "Cy", "Fernando").await;
    let left_yesterday = hotel.stay("checked_out", date(2027, 3, 7), yesterday, Some("104"), "Di", "Jay").await;
    hotel.stay("cancelled", today, tomorrow, None, "Ed", "Cancelled").await;
    hotel.stay("confirmed", tomorrow, date(2027, 3, 12), None, "Flo", "Later").await;
    let list = |kind, on| {
        let hotel = &hotel;
        async move {
            let rows = reports::daily_list(&mut hotel.tx().await, hotel.property, on, kind).await.unwrap();
            rows.into_iter().map(|row| row.reservation_room).collect::<Vec<_>>()
        }
    };

    assert_eq!(list(ListKind::Arrivals, today).await, [arriving]);
    assert_eq!(list(ListKind::Arrivals, yesterday).await, [arrived_early]);
    assert_eq!(list(ListKind::Departures, today).await, [leaving]);
    assert_eq!(list(ListKind::Departures, yesterday).await, [left_yesterday]);
    // In house on the night of the 10th: ordered by room number; the departing guest has checked out of the night.
    assert_eq!(list(ListKind::InHouse, today).await, [arrived_early]);
    assert_eq!(list(ListKind::InHouse, yesterday).await, [arrived_early, leaving]);

    let rows = reports::daily_list(&mut hotel.tx().await, hotel.property, today, ListKind::Arrivals).await.unwrap();
    assert_eq!(rows[0].guest, "Ada Silva");
    assert_eq!(rows[0].room_number.as_deref(), Some("102"));
    assert_eq!((rows[0].arrival, rows[0].departure), (today, tomorrow));
    let other = reports::daily_list(&mut hotel.tx().await, uuid::Uuid::now_v7(), today, ListKind::InHouse).await;
    assert!(matches!(other, Err(ReportsError::NotFound("property"))));
}
