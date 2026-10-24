//! The advisory deposit: what a reservation's rooms ask for in advance under the rule each kept at booking
//! (`reservation_room.deposit_terms`), and how much of it its deposit folio has taken. Nothing enforces it: booking,
//! check-in and the night audit's close never read it.

use crate::ReservationsError;
use crate::cancellation::penalty_amount;
use db::Tx;
use rates::{DepositDue, DepositKind, DepositRule, Penalty, PenaltyKind};
use serde::Serialize;
use sqlx::types::Json;
use time::{Date, Duration};
use uuid::Uuid;

/// A reservation's deposit in one currency, in minor units.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct DepositDueLine {
    pub currency: String,
    /// The sum of its rooms' deposits in this currency, each at most the room's total.
    pub amount: i64,
    /// The earliest of those rooms' due dates.
    pub due_date: Date,
    /// The deposit folio's payments in this currency, net of refunds, without transfers.
    pub paid: i64,
    /// `max(0, amount - paid)`.
    pub outstanding: i64,
}

/// A room counted towards its reservation's deposit: neither cancelled nor a no-show, with the rule it was booked
/// under and its nights `(date, room_amount, meal_amount)` in date order.
#[derive(Debug, Clone)]
pub struct DepositRoom {
    pub currency: String,
    pub arrival: Date,
    pub rule: DepositRule,
    pub nights: Vec<(Date, i64, i64)>,
}

/// What `room` asks for and by when, for a reservation booked on the business date `booked`: priced as a penalty of the
/// same kind and value is ([`penalty_amount`]), so at most the room's total; due on `booked` or `days` before arrival.
pub fn room_deposit(room: &DepositRoom, booked: Date) -> (i64, Date) {
    let kind = match room.rule.kind {
        DepositKind::Percent => PenaltyKind::Percent,
        DepositKind::Nights => PenaltyKind::Nights,
        DepositKind::Amount => PenaltyKind::Amount,
    };
    let amount = penalty_amount(Penalty { kind, value: room.rule.value }, &room.nights);
    let due_date = match room.rule.due {
        DepositDue::AtBooking => booked,
        DepositDue::DaysBeforeArrival { days } => room.arrival - Duration::days(i64::from(days)),
    };
    (amount, due_date)
}

/// The deposit of a reservation booked on `booked` with `rooms`, per currency in the order the rooms first use them,
/// against `paid` (net payments by currency; a currency not listed has none).
pub fn deposit_lines(booked: Date, rooms: &[DepositRoom], paid: &[(String, i64)]) -> Vec<DepositDueLine> {
    let mut lines: Vec<DepositDueLine> = Vec::new();
    for room in rooms {
        let (amount, due_date) = room_deposit(room, booked);
        match lines.iter_mut().find(|line| line.currency == room.currency) {
            Some(line) => {
                line.amount += amount;
                line.due_date = line.due_date.min(due_date);
            }
            None => lines.push(DepositDueLine {
                currency: room.currency.clone(),
                amount,
                due_date,
                paid: 0,
                outstanding: 0,
            }),
        }
    }
    for line in &mut lines {
        line.paid = paid.iter().filter(|(currency, _)| *currency == line.currency).map(|(_, amount)| amount).sum();
        line.outstanding = (line.amount - line.paid).max(0);
    }
    lines
}

#[derive(sqlx::FromRow)]
struct Room {
    currency: String,
    arrival: Date,
    deposit_terms: Json<DepositRule>,
    dates: Vec<Date>,
    rooms: Vec<i64>,
    meals: Vec<i64>,
}

/// The reservation's deposit per currency ([`deposit_lines`]): its rooms with booked deposit terms that are neither
/// cancelled nor no-shows, against its deposit folio's payments. Empty when no such room has terms. `NotFound` for a
/// reservation not in the tenant.
pub async fn deposit_due(tx: &mut Tx, reservation: Uuid) -> Result<Vec<DepositDueLine>, ReservationsError> {
    let booked: Date = sqlx::query_scalar("select booked_business_date from reservation where id = $1")
        .bind(reservation)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ReservationsError::NotFound("reservation"))?;
    let rows: Vec<Room> = sqlx::query_as(
        "select rr.currency::text as currency, rr.arrival, rr.deposit_terms,
                coalesce(n.dates, '{}') as dates, coalesce(n.rooms, '{}') as rooms, coalesce(n.meals, '{}') as meals
         from reservation_room rr
         left join lateral (select array_agg(date order by date) as dates,
                                   array_agg(room_amount order by date) as rooms,
                                   array_agg(meal_amount order by date) as meals
                            from reservation_night where reservation_room_id = rr.id) n on true
         where rr.reservation_id = $1 and rr.deposit_terms is not null and rr.status not in ('cancelled', 'no_show')
         order by rr.arrival, rr.id",
    )
    .bind(reservation)
    .fetch_all(&mut **tx)
    .await?;
    let paid: Vec<(String, i64)> = sqlx::query_as(
        "select p.currency::text, sum(p.amount)::bigint
         from payment p join folio f on f.id = p.folio_id
         where f.reservation_id = $1 and f.kind = 'deposit' and p.method <> 'transfer'
         group by p.currency",
    )
    .bind(reservation)
    .fetch_all(&mut **tx)
    .await?;
    let rooms: Vec<DepositRoom> = rows
        .into_iter()
        .map(|row| DepositRoom {
            currency: row.currency,
            arrival: row.arrival,
            rule: row.deposit_terms.0,
            nights: row
                .dates
                .into_iter()
                .zip(row.rooms)
                .zip(row.meals)
                .map(|((d, room), meal)| (d, room, meal))
                .collect(),
        })
        .collect();
    Ok(deposit_lines(booked, &rooms, &paid))
}

#[cfg(test)]
mod tests {
    use super::{DepositRoom, deposit_lines, room_deposit};
    use rates::{DepositDue, DepositKind, DepositRule};
    use time::macros::date;
    use time::{Date, Duration};

    const BOOKED: Date = date!(2026 - 10 - 01);
    const ARRIVAL: Date = date!(2026 - 10 - 10);

    /// Three nights from `ARRIVAL` in `currency`: rooms 10000, 12000 and 14000, meals 1000 each (39000 in all).
    fn room(currency: &str, kind: DepositKind, value: i64, due: DepositDue) -> DepositRoom {
        DepositRoom {
            currency: currency.into(),
            arrival: ARRIVAL,
            rule: DepositRule { kind, value, due },
            nights: (0..3).map(|n| (ARRIVAL + Duration::days(n), 10_000 + 2_000 * n, 1_000)).collect(),
        }
    }

    fn amount(kind: DepositKind, value: i64) -> i64 {
        room_deposit(&room("LKR", kind, value, DepositDue::AtBooking), BOOKED).0
    }

    #[test]
    fn a_percent_deposit_is_basis_points_of_the_total_rounded_half_up_and_capped() {
        assert_eq!(amount(DepositKind::Percent, 3_000), 11_700);
        assert_eq!(amount(DepositKind::Percent, 3), 12, "11.7 rounds up");
        assert_eq!(amount(DepositKind::Percent, 10_000), 39_000);
    }

    #[test]
    fn a_nights_deposit_is_the_first_nights_room_amounts_capped_at_the_stay() {
        assert_eq!(amount(DepositKind::Nights, 1), 10_000);
        assert_eq!(amount(DepositKind::Nights, 2), 22_000, "no meals");
        assert_eq!(amount(DepositKind::Nights, 30), 36_000, "all three nights");
    }

    #[test]
    fn an_amount_deposit_is_capped_at_the_room_s_total() {
        assert_eq!(amount(DepositKind::Amount, 25_000), 25_000);
        assert_eq!(amount(DepositKind::Amount, 100_000), 39_000);
    }

    #[test]
    fn a_deposit_is_due_at_booking_or_days_before_arrival() {
        let at_booking = room("LKR", DepositKind::Nights, 1, DepositDue::AtBooking);
        assert_eq!(room_deposit(&at_booking, BOOKED).1, BOOKED);
        let before = room("LKR", DepositKind::Nights, 1, DepositDue::DaysBeforeArrival { days: 7 });
        assert_eq!(room_deposit(&before, BOOKED).1, date!(2026 - 10 - 03));
        let on_the_day = room("LKR", DepositKind::Nights, 1, DepositDue::DaysBeforeArrival { days: 0 });
        assert_eq!(room_deposit(&on_the_day, BOOKED).1, ARRIVAL);
    }

    #[test]
    fn rooms_sum_per_currency_with_the_earliest_due_date_and_their_own_payments() {
        let rooms = [
            room("LKR", DepositKind::Nights, 1, DepositDue::DaysBeforeArrival { days: 3 }),
            room("USD", DepositKind::Amount, 5_000, DepositDue::AtBooking),
            room("LKR", DepositKind::Percent, 5_000, DepositDue::DaysBeforeArrival { days: 5 }),
        ];
        let paid = [("LKR".to_owned(), 40_000), ("USD".to_owned(), 2_000)];
        let lines = deposit_lines(BOOKED, &rooms, &paid);
        assert_eq!(lines.len(), 2);
        assert_eq!(
            (lines[0].currency.as_str(), lines[0].amount, lines[0].due_date, lines[0].paid, lines[0].outstanding),
            ("LKR", 29_500, date!(2026 - 10 - 05), 40_000, 0),
            "overpaid owes nothing"
        );
        assert_eq!(
            (lines[1].currency.as_str(), lines[1].amount, lines[1].due_date, lines[1].paid, lines[1].outstanding),
            ("USD", 5_000, BOOKED, 2_000, 3_000)
        );
    }

    #[test]
    fn no_rooms_owe_nothing() {
        assert!(deposit_lines(BOOKED, &[], &[("LKR".to_owned(), 1_000)]).is_empty());
    }
}
