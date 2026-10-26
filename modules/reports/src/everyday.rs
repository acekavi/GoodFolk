//! The everyday reports of a date range, from the statistics the night audit stored when each day closed, and the
//! daily lists of one day.
//!
//! Only closed days count. A day of the range before the property's business date with no stored row (before its
//! first close) is not closed either: such days are listed in [`Summary::not_closed`], never estimated.

use crate::ReportsError;
use audit::DailyStats;
use db::Tx;
use domain::RoomStatus;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use time::{Date, Duration, Month, Weekday};
use uuid::Uuid;

/// The most days a report range covers.
const MAX_RANGE_DAYS: i64 = 366;

/// What a range holds, from its ends: how its same period last year is found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeKind {
    /// A whole calendar month.
    Month,
    /// Monday to Sunday.
    Week,
    Custom,
}

impl RangeKind {
    /// The kind of the range `from` to `to`, both included.
    pub fn of(from: Date, to: Date) -> Self {
        if from.day() == 1 && to == last_day(from.year(), from.month()) {
            Self::Month
        } else if from.weekday() == Weekday::Monday && to == from + Duration::days(6) {
            Self::Week
        } else {
            Self::Custom
        }
    }
}

fn last_day(year: i32, month: Month) -> Date {
    Date::from_calendar_date(year, month, month.length(year)).expect("a month's last day is a date")
}

/// The range of the same period last year, both ends included: a whole month compares with the same month, a week
/// with the week 364 days earlier (the same weekdays line up), and any other range with the same dates. `None` when
/// the period has no counterpart: an end falling on 29 February.
pub fn last_year_range(kind: RangeKind, from: Date, to: Date) -> Option<(Date, Date)> {
    match kind {
        RangeKind::Month => Some((
            Date::from_calendar_date(from.year() - 1, from.month(), 1).ok()?,
            last_day(from.year() - 1, from.month()),
        )),
        RangeKind::Week => Some((from - Duration::days(364), to - Duration::days(364))),
        RangeKind::Custom => Some((a_year_back(from)?, a_year_back(to)?)),
    }
}

fn a_year_back(date: Date) -> Option<Date> {
    Date::from_calendar_date(date.year() - 1, date.month(), date.day()).ok()
}

/// The five headline figures of a set of closed days, each rounded once from sums, never averaged from daily ratios.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Headline {
    /// Rooms sold over rooms available, in basis points: 10000 is full.
    pub occupancy_bp: i32,
    /// Room revenue per room sold.
    pub adr: i64,
    /// Room revenue per room available.
    pub revpar: i64,
    pub room_nights: i64,
    pub total_revenue: i64,
}

/// One closed day's figures, as the night audit stored them. Money is in base-currency minor units, net of tax.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DayRow {
    pub business_date: Date,
    pub rooms_available: i32,
    pub rooms_sold: i32,
    pub occupancy_bp: i32,
    pub adults: i32,
    pub children: i32,
    pub arrivals: i32,
    pub departures: i32,
    pub room_revenue: i64,
    pub meal_plan_revenue: i64,
    pub pos_revenue: i64,
    pub fee_revenue: i64,
    pub other_revenue: i64,
    pub total_revenue: i64,
    pub adr: i64,
    pub revpar: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Dimension {
    Segment,
    Source,
    Residency,
    RoomType,
    Outlet,
}

impl Dimension {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "segment" => Some(Self::Segment),
            "source" => Some(Self::Source),
            "residency" => Some(Self::Residency),
            "room_type" => Some(Self::RoomType),
            "outlet" => Some(Self::Outlet),
            _ => None,
        }
    }
}

/// A dimension's key summed over the closed days of the range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BreakdownTotal {
    /// The dimension's value; `unattributed` for revenue no stay accounts for.
    pub key: String,
    pub room_nights: i64,
    pub room_revenue: i64,
    pub revenue: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Summary {
    /// The first and last closed day of the range; `None` when none is closed.
    pub closed_from: Option<Date>,
    pub closed_to: Option<Date>,
    /// The days of the range that are not closed yet, by date.
    pub not_closed: Vec<Date>,
    /// Over the closed days only.
    pub headline: Headline,
    /// The same period last year, when every day of it is closed.
    pub last_year: Option<Headline>,
    pub days: Vec<DayRow>,
    /// Each dimension's keys by key (byte order).
    pub breakdowns: BTreeMap<Dimension, Vec<BreakdownTotal>>,
}

/// `numerator / denominator` rounded half away from zero; 0 for an empty denominator.
fn ratio(numerator: i64, denominator: i64) -> i64 {
    if denominator == 0 {
        return 0;
    }
    let (n, d) = (i128::from(numerator), i128::from(denominator));
    let rounded = (2 * n.abs() + d) / (2 * d) * n.signum();
    i64::try_from(rounded).expect("a ratio of i64s with an i64 denominator fits an i64")
}

fn total_revenue(day: &DailyStats) -> i64 {
    day.room_revenue + day.meal_plan_revenue + day.pos_revenue + day.fee_revenue + day.other_revenue
}

fn headline(days: &[DailyStats]) -> Headline {
    let sum = |value: fn(&DailyStats) -> i64| days.iter().map(value).sum::<i64>();
    let (available, sold) = (sum(|d| i64::from(d.rooms_available)), sum(|d| i64::from(d.rooms_sold)));
    let room_revenue = sum(|d| d.room_revenue);
    Headline {
        occupancy_bp: i32::try_from(ratio(sold * 10_000, available)).unwrap_or(i32::MAX),
        adr: ratio(room_revenue, sold),
        revpar: ratio(room_revenue, available),
        room_nights: sold,
        total_revenue: sum(total_revenue),
    }
}

fn day_row(day: DailyStats) -> DayRow {
    DayRow {
        business_date: day.business_date,
        rooms_available: day.rooms_available,
        rooms_sold: day.rooms_sold,
        occupancy_bp: day.occupancy_bp,
        adults: day.adults,
        children: day.children,
        arrivals: day.arrivals,
        departures: day.departures,
        room_revenue: day.room_revenue,
        meal_plan_revenue: day.meal_plan_revenue,
        pos_revenue: day.pos_revenue,
        fee_revenue: day.fee_revenue,
        other_revenue: day.other_revenue,
        total_revenue: total_revenue(&day),
        adr: day.adr,
        revpar: day.revpar,
    }
}

fn breakdowns(days: &[DailyStats]) -> Result<BTreeMap<Dimension, Vec<BreakdownTotal>>, ReportsError> {
    let mut totals: BTreeMap<Dimension, BTreeMap<&str, BreakdownTotal>> = BTreeMap::new();
    for row in days.iter().flat_map(|day| &day.breakdown) {
        let dimension = Dimension::parse(&row.dimension)
            .ok_or_else(|| ReportsError::Invalid(format!("unknown breakdown dimension {:?}", row.dimension)))?;
        let total = totals.entry(dimension).or_default().entry(&row.key).or_insert_with(|| BreakdownTotal {
            key: row.key.clone(),
            room_nights: 0,
            room_revenue: 0,
            revenue: 0,
        });
        total.room_nights += i64::from(row.room_nights);
        total.room_revenue += row.room_revenue;
        total.revenue += row.revenue;
    }
    Ok(totals.into_iter().map(|(dimension, keys)| (dimension, keys.into_values().collect())).collect())
}

/// The everyday reports of `property` from `from` to `to`, both included (at most 366 days, `Invalid` otherwise),
/// from the closed days among them. `NotFound` for a property not in the tenant.
pub async fn summary(tx: &mut Tx, property: Uuid, from: Date, to: Date) -> Result<Summary, ReportsError> {
    let stored = audit::daily_stats(tx, property, from, to).await?;
    let closed: HashSet<Date> = stored.iter().map(|day| day.business_date).collect();
    let not_closed = from.iter_to(to).filter(|date| !closed.contains(date)).collect();

    let last_year = match last_year_range(RangeKind::of(from, to), from, to) {
        // A leap year can make last year's range one day longer than the longest range read at once.
        Some((last_from, last_to)) if (last_to - last_from).whole_days() < MAX_RANGE_DAYS => {
            let days = audit::daily_stats(tx, property, last_from, last_to).await?;
            (days.len() as i64 == (last_to - last_from).whole_days() + 1).then(|| headline(&days))
        }
        _ => None,
    };

    Ok(Summary {
        closed_from: stored.first().map(|day| day.business_date),
        closed_to: stored.last().map(|day| day.business_date),
        not_closed,
        headline: headline(&stored),
        last_year,
        breakdowns: breakdowns(&stored)?,
        days: stored.into_iter().map(day_row).collect(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    /// Stays arriving on the day.
    Arrivals,
    /// Checked-in or checked-out stays leaving on the day.
    Departures,
    /// Stays checked in and not yet due out on the day.
    InHouse,
}

/// One stay of a daily list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListRow {
    pub reservation_room: Uuid,
    pub reservation_id: Uuid,
    pub confirmation: String,
    pub guest: String,
    pub room_number: Option<String>,
    pub arrival: Date,
    pub departure: Date,
    pub status: RoomStatus,
    pub adults: i32,
    pub children: i32,
}

#[derive(sqlx::FromRow)]
struct StayRow {
    id: Uuid,
    reservation_id: Uuid,
    confirmation_no: String,
    first_name: String,
    last_name: String,
    number: Option<String>,
    arrival: Date,
    departure: Date,
    status: String,
    adults: i32,
    children: i32,
}

/// The stays of the property (`$1`) on the date (`$2`), which `$3` narrows to a kind: `arrivals`, `departures` or
/// `in_house`. Unlike the night audit's lists of what is still unresolved, these hold the stays that did or will
/// arrive and leave on the day, so a closed day's list is the same after the close. `arrival` is bounded as
/// `folio::in_house_stays` bounds it, so the `reservation_room_arrival_idx` can serve the in-house kind.
const STAYS_SQL: &str = "select rr.id, rr.reservation_id, r.confirmation_no, g.first_name, g.last_name, room.number,
        rr.arrival, rr.arrival + rr.nights as departure, rr.status, rr.adults, rr.children
 from reservation_room rr
 join reservation r on r.id = rr.reservation_id
 join guest g on g.id = rr.primary_guest_id
 left join room on room.id = rr.room_id
 where rr.property_id = $1
   and case $3
         when 'arrivals' then rr.arrival = $2 and rr.status <> 'cancelled'
         when 'departures' then rr.arrival <= $2 and rr.arrival + rr.nights = $2
                                and rr.status in ('checked_in', 'checked_out')
         else ((rr.arrival >= $2::date - 31 and rr.arrival <= $2) or rr.nights > 31)
              and rr.arrival <= $2 and $2 < rr.arrival + rr.nights and rr.status in ('checked_in', 'checked_out')
       end
 order by room.number nulls last, g.last_name, g.first_name, rr.id";

/// The stays arriving, leaving or in house on `date` (a closed day or the business date), by room number, then guest.
/// `NotFound` for a property not in the tenant.
pub async fn daily_list(tx: &mut Tx, property: Uuid, date: Date, kind: ListKind) -> Result<Vec<ListRow>, ReportsError> {
    let found: bool = sqlx::query_scalar("select exists (select 1 from property where id = $1)")
        .bind(property)
        .fetch_one(&mut **tx)
        .await?;
    if !found {
        return Err(ReportsError::NotFound("property"));
    }
    let kind = match kind {
        ListKind::Arrivals => "arrivals",
        ListKind::Departures => "departures",
        ListKind::InHouse => "in_house",
    };
    let rows: Vec<StayRow> =
        sqlx::query_as(STAYS_SQL).bind(property).bind(date).bind(kind).fetch_all(&mut **tx).await?;
    rows.into_iter()
        .map(|row| {
            let status = RoomStatus::parse(&row.status).ok_or_else(|| sqlx::Error::ColumnDecode {
                index: "status".into(),
                source: format!("unknown value {:?}", row.status).into(),
            })?;
            let guest =
                if row.first_name.is_empty() { row.last_name } else { format!("{} {}", row.first_name, row.last_name) };
            Ok(ListRow {
                reservation_room: row.id,
                reservation_id: row.reservation_id,
                confirmation: row.confirmation_no,
                guest,
                room_number: row.number,
                arrival: row.arrival,
                departure: row.departure,
                status,
                adults: row.adults,
                children: row.children,
            })
        })
        .collect()
}
