//! Cancelling a reservation room: its unused nights go back on sale and it records what cancelling cost under
//! the terms the booking kept, whatever its plan's policy says now.

use crate::{ReservationsError, audit, business_date, notify, reservation_key, reservations_key};
use db::{TenantId, Tx, UserId};
use domain::{Action, RoomStatus};
use rates::{CancellationRule, Penalty, PenaltyKind};
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use time::Date;
use uuid::Uuid;

/// The cancellation policy a room was booked under, copied from its plan at booking
/// (`reservation_room.cancellation_terms`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CancellationTerms {
    pub rules: Vec<CancellationRule>,
    /// What not arriving costs; the no-show command applies it, cancelling does not.
    pub no_show: Penalty,
}

/// A cancelled room and what cancelling it cost, in its currency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct CancelledRoom {
    pub id: Uuid,
    pub reservation_id: Uuid,
    pub status: RoomStatus,
    pub version: i32,
    pub penalty: i64,
    pub currency: String,
}

/// What cancelling a stay arriving on `check_in` costs on `today`, in minor units of the stay's currency.
///
/// `nights` are the stay's nights in date order, each with its room and meal amounts. The days left are
/// `check_in - today`, 0 once the guest was due. The rule with the fewest `days_before_arrival` that is still
/// at or above the days left applies: `nights` costs the room amounts of the first that many nights (all of
/// them if the stay is shorter), `percent` costs that many basis points of the stay's total (room and meals),
/// rounded half up, and `amount` costs the amount. The penalty never exceeds the stay's total. Without terms,
/// or when no rule reaches the days left, cancelling is free.
pub fn cancellation_penalty(
    terms: Option<&CancellationTerms>,
    nights: &[(Date, i64, i64)],
    check_in: Date,
    today: Date,
) -> i64 {
    let Some(terms) = terms else { return 0 };
    let days_left = (check_in - today).whole_days().max(0);
    let rule = terms
        .rules
        .iter()
        .filter(|rule| i64::from(rule.days_before_arrival) >= days_left)
        .min_by_key(|rule| rule.days_before_arrival);
    let Some(rule) = rule else { return 0 };
    let total: i64 = nights.iter().map(|&(_, room, meal)| room + meal).sum();
    let penalty = match rule.penalty.kind {
        PenaltyKind::Nights => {
            let count = usize::try_from(rule.penalty.value).unwrap_or(0);
            nights.iter().take(count).map(|&(_, room, _)| room).sum()
        }
        PenaltyKind::Percent => {
            let exact = i128::from(total) * i128::from(rule.penalty.value);
            let rounded = ((exact + 5_000) / 10_000).min(i128::from(total));
            i64::try_from(rounded).expect("at most the stay's total")
        }
        PenaltyKind::Amount => rule.penalty.value,
    };
    penalty.min(total)
}

/// Cancels the room `id` of the property, which must be at `expected_version` (`VersionMismatch`) and
/// tentative or confirmed (`Conflict`). Its nights from the business date on go back on sale, and the penalty
/// its booked terms set for cancelling on the business date is recorded. The reservation's version moves
/// too: its derived status may change, so a client holding it must refetch.
pub async fn cancel_room(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
) -> Result<CancelledRoom, ReservationsError> {
    let row: Option<StayRow> = sqlx::query_as(
        "select reservation_id, room_type_id, status, lower(stay) as check_in, upper(stay) as check_out, currency,
                cancellation_terms, version
         from reservation_room where id = $1 and property_id = $2
         for update",
    )
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?;
    let stay = row.ok_or(ReservationsError::NotFound("reservation room"))?;
    if stay.version != expected_version {
        return Err(ReservationsError::VersionMismatch("reservation room"));
    }
    let current = RoomStatus::parse(&stay.status).ok_or_else(|| sqlx::Error::ColumnDecode {
        index: "status".into(),
        source: format!("unknown {:?}", stay.status).into(),
    })?;
    let status =
        domain::transition(current, Action::Cancel).map_err(|invalid| ReservationsError::Conflict(invalid.message))?;
    let today = business_date(tx, property).await?;

    // Counters before the business date are history, outside the counter window: only the nights from the
    // business date on go back on sale.
    let from = stay.check_in.max(today);
    if from < stay.check_out {
        rooms::lock_days(tx, property, &[stay.room_type_id], from, stay.check_out).await?;
        sqlx::query(
            "update inventory_day set sold = sold - 1
             where property_id = $1 and room_type_id = $2 and date >= $3 and date < $4",
        )
        .bind(property)
        .bind(stay.room_type_id)
        .bind(from)
        .bind(stay.check_out)
        .execute(&mut **tx)
        .await?;
    }

    let nights: Vec<(Date, i64, i64)> = sqlx::query_as(
        "select date, room_amount, meal_amount from reservation_night where reservation_room_id = $1 order by date",
    )
    .bind(id)
    .fetch_all(&mut **tx)
    .await?;
    let terms = stay.cancellation_terms.as_ref().map(|terms| &terms.0);
    let penalty = cancellation_penalty(terms, &nights, stay.check_in, today);
    let version: i32 = sqlx::query_scalar(
        "update reservation_room
         set status = $2, cancelled_at = now(), cancelled_by = $3, cancellation_penalty = $4, version = version + 1
         where id = $1
         returning version",
    )
    .bind(id)
    .bind(status.as_str())
    .bind(actor.0)
    .bind(penalty)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("update reservation set version = version + 1 where id = $1")
        .bind(stay.reservation_id)
        .execute(&mut **tx)
        .await?;

    let data = serde_json::json!({
        "reservation_id": stay.reservation_id,
        "penalty": penalty,
        "currency": stay.currency,
    });
    audit(tx, tenant, actor, "reservation_room.cancelled", "reservation_room", id, data).await?;
    // The tape key covers the stay's whole range, not just the nights `from` released: the chart shows a
    // cancelled stay's past nights too.
    let keys = [reservations_key(property), reservation_key(stay.reservation_id)]
        .into_iter()
        .chain(rooms::month_keys(property, from, stay.check_out))
        .chain(rooms::tape_keys(property, stay.check_in, stay.check_out))
        .collect();
    notify(tx, tenant, property, keys).await?;

    Ok(CancelledRoom { id, reservation_id: stay.reservation_id, status, version, penalty, currency: stay.currency })
}

/// The parts of a `reservation_room` that cancelling reads.
#[derive(sqlx::FromRow)]
struct StayRow {
    reservation_id: Uuid,
    room_type_id: Uuid,
    status: String,
    check_in: Date,
    check_out: Date,
    currency: String,
    cancellation_terms: Option<Json<CancellationTerms>>,
    version: i32,
}

#[cfg(test)]
mod tests {
    use super::{CancellationTerms, cancellation_penalty};
    use rates::{CancellationRule, Penalty, PenaltyKind};
    use time::macros::date;
    use time::{Date, Duration};

    const ARRIVAL: Date = date!(2026 - 10 - 10);

    /// Three nights from `ARRIVAL`: rooms 10000, 12000 and 14000, meals 1000 each (39000 in all).
    fn nights() -> Vec<(Date, i64, i64)> {
        (0..3).map(|n| (ARRIVAL + Duration::days(n), 10_000 + 2_000 * n, 1_000)).collect()
    }

    fn rule(days_before_arrival: i32, kind: PenaltyKind, value: i64) -> CancellationRule {
        CancellationRule { days_before_arrival, penalty: Penalty { kind, value } }
    }

    /// Terms with `rules`; a no-show costs everything.
    fn terms(rules: Vec<CancellationRule>) -> CancellationTerms {
        CancellationTerms { rules, no_show: Penalty { kind: PenaltyKind::Percent, value: 10_000 } }
    }

    /// A 1-night rule 7 days out and a 50% rule 2 days out.
    fn standard() -> CancellationTerms {
        terms(vec![rule(7, PenaltyKind::Nights, 1), rule(2, PenaltyKind::Percent, 5_000)])
    }

    /// The penalty of cancelling `days_left` days before `ARRIVAL`.
    fn penalty(terms: Option<&CancellationTerms>, days_left: i64) -> i64 {
        cancellation_penalty(terms, &nights(), ARRIVAL, ARRIVAL - Duration::days(days_left))
    }

    #[test]
    fn without_terms_cancelling_is_free() {
        assert_eq!(penalty(None, 0), 0);
    }

    #[test]
    fn before_every_rule_cancelling_is_free() {
        assert_eq!(penalty(Some(&standard()), 8), 0);
    }

    #[test]
    fn the_rule_with_the_fewest_days_still_reached_applies_its_boundary_included() {
        let terms = standard();
        assert_eq!(penalty(Some(&terms), 7), 10_000, "exactly 7 days out: the first night");
        assert_eq!(penalty(Some(&terms), 3), 10_000);
        assert_eq!(penalty(Some(&terms), 2), 19_500, "exactly 2 days out: half of 39000");
        assert_eq!(penalty(Some(&terms), 0), 19_500);
    }

    #[test]
    fn after_arrival_the_closest_rule_applies_as_on_the_day() {
        let terms = standard();
        assert_eq!(penalty(Some(&terms), -1), 19_500);
        assert_eq!(penalty(Some(&terms), -5), penalty(Some(&terms), 0));
        assert_eq!(penalty(Some(&self::terms(vec![rule(0, PenaltyKind::Nights, 1)])), -2), 10_000);
    }

    #[test]
    fn a_nights_penalty_costs_those_nights_room_amounts_up_to_the_whole_stay() {
        assert_eq!(penalty(Some(&terms(vec![rule(5, PenaltyKind::Nights, 2)])), 1), 22_000, "no meals");
        assert_eq!(penalty(Some(&terms(vec![rule(5, PenaltyKind::Nights, 30)])), 1), 36_000, "all 3 nights");
    }

    #[test]
    fn a_percent_penalty_rounds_half_up() {
        // 39000 × 1 bp = 3.9 → 4; × 3 bp = 11.7 → 12.
        assert_eq!(penalty(Some(&terms(vec![rule(5, PenaltyKind::Percent, 1)])), 1), 4);
        assert_eq!(penalty(Some(&terms(vec![rule(5, PenaltyKind::Percent, 3)])), 1), 12);
        let small = [(ARRIVAL, 5, 0)];
        let half = terms(vec![rule(5, PenaltyKind::Percent, 1_000)]);
        assert_eq!(cancellation_penalty(Some(&half), &small, ARRIVAL, ARRIVAL), 1, "0.5 rounds up");
        let less = terms(vec![rule(5, PenaltyKind::Percent, 999)]);
        assert_eq!(cancellation_penalty(Some(&less), &small, ARRIVAL, ARRIVAL), 0, "0.4995 rounds down");
        assert_eq!(penalty(Some(&terms(vec![rule(5, PenaltyKind::Percent, 10_000)])), 1), 39_000);
    }

    #[test]
    fn an_amount_penalty_is_capped_at_the_stay_s_total() {
        assert_eq!(penalty(Some(&terms(vec![rule(5, PenaltyKind::Amount, 25_000)])), 1), 25_000);
        assert_eq!(penalty(Some(&terms(vec![rule(5, PenaltyKind::Amount, 100_000)])), 1), 39_000);
    }

    #[test]
    fn the_no_show_penalty_is_not_a_cancellation_rule() {
        let only_no_show = terms(vec![]);
        assert_eq!(penalty(Some(&only_no_show), 0), 0);
        assert_eq!(penalty(Some(&only_no_show), -1), 0);
    }
}
