//! The reservation room state machine (Phase 3): which [`Action`]s move a [`RoomStatus`] to which other one,
//! and the reservation-level status derived from its rooms. Pure and dependency-free (no DB, no async) so the
//! rules are exhaustively unit-tested here and shared, rather than re-derived, by every caller: the
//! reservations module, the night audit and the tape chart.
//!
//! The `reservation` table has no status column; `reservation_room` does, and [`reservation_status`] derives
//! the parent reservation's status from its rooms' when one is needed (a detail view, a list row).

use std::fmt;

/// A `reservation_room`'s status, exactly per the diagram in `docs/specs/phase-3-reservations.md` § State
/// machine.
///
/// Convention: `modules/rates`' `text_enum!` macro (per-variant `#[serde(rename = "...")]`, `as_str`/`parse`)
/// is private to that crate, and `domain` must not depend on `rates` (it stays pure). Every variant name here
/// already lowercases to its database text with `snake_case`, so this instead follows the plainer convention
/// already used by `identity::Role`: `#[serde(rename_all = "snake_case")]` plus hand-written `as_str`/`parse`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoomStatus {
    Tentative,
    Confirmed,
    CheckedIn,
    CheckedOut,
    Cancelled,
    NoShow,
}

impl RoomStatus {
    /// The `text` value stored in `reservation_room.status`.
    pub fn as_str(self) -> &'static str {
        match self {
            RoomStatus::Tentative => "tentative",
            RoomStatus::Confirmed => "confirmed",
            RoomStatus::CheckedIn => "checked_in",
            RoomStatus::CheckedOut => "checked_out",
            RoomStatus::Cancelled => "cancelled",
            RoomStatus::NoShow => "no_show",
        }
    }

    pub fn parse(value: &str) -> Option<RoomStatus> {
        [
            RoomStatus::Tentative,
            RoomStatus::Confirmed,
            RoomStatus::CheckedIn,
            RoomStatus::CheckedOut,
            RoomStatus::Cancelled,
            RoomStatus::NoShow,
        ]
        .into_iter()
        .find(|status| status.as_str() == value)
    }
}

impl fmt::Display for RoomStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A command applied to one `reservation_room`. Each REST action endpoint
/// (`reservation-rooms/{id}/{confirm|check-in|undo-check-in|check-out|cancel}`, and the night audit for
/// `NoShow`) picks its action directly, so this is never itself serialized on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Confirm,
    CheckIn,
    UndoCheckIn,
    CheckOut,
    Cancel,
    NoShow,
}

/// `action` does not apply to a room in status `from`. Reaching this is a caller bug (the UI and the API
/// should only ever offer actions valid for the room's current status), so `message` is written for logs and
/// error responses alike, e.g. "a checked-out room can't be cancelled".
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct InvalidTransition {
    pub from: RoomStatus,
    pub action: Action,
    pub message: String,
}

/// A human description of `from`, for [`InvalidTransition`] messages.
fn subject(from: RoomStatus) -> &'static str {
    match from {
        RoomStatus::Tentative => "a tentative room",
        RoomStatus::Confirmed => "a confirmed room",
        RoomStatus::CheckedIn => "a checked-in room",
        RoomStatus::CheckedOut => "a checked-out room",
        RoomStatus::Cancelled => "a cancelled room",
        RoomStatus::NoShow => "a no-show room",
    }
}

fn invalid_transition(from: RoomStatus, action: Action) -> InvalidTransition {
    let subject = subject(from);
    let message = match action {
        Action::Confirm => format!("{subject} can't be confirmed"),
        Action::CheckIn => format!("{subject} can't be checked in"),
        Action::UndoCheckIn => format!("{subject}'s check-in can't be undone"),
        Action::CheckOut => format!("{subject} can't be checked out"),
        Action::Cancel => format!("{subject} can't be cancelled"),
        Action::NoShow => format!("{subject} can't be marked a no-show"),
    };
    InvalidTransition { from, action, message }
}

/// Applies `action` to a room in status `from`, exactly per the diagram in
/// `docs/specs/phase-3-reservations.md` § State machine: `confirm` (tentative → confirmed), `check_in`
/// (confirmed → checked_in), `undo_check_in` (checked_in → confirmed), `check_out` (checked_in →
/// checked_out), `cancel` (tentative or confirmed → cancelled), `no_show` (confirmed → no_show only). Every
/// other pair is refused.
///
/// `undo_check_in`'s "same business date only" rule is a guard the 3b command checks before calling this
/// (this crate has no notion of a business date); this function only knows the shape of the graph.
pub fn transition(from: RoomStatus, action: Action) -> Result<RoomStatus, InvalidTransition> {
    match (from, action) {
        (RoomStatus::Tentative, Action::Confirm) => Ok(RoomStatus::Confirmed),
        (RoomStatus::Tentative, Action::Cancel) => Ok(RoomStatus::Cancelled),
        (RoomStatus::Confirmed, Action::CheckIn) => Ok(RoomStatus::CheckedIn),
        (RoomStatus::Confirmed, Action::Cancel) => Ok(RoomStatus::Cancelled),
        (RoomStatus::Confirmed, Action::NoShow) => Ok(RoomStatus::NoShow),
        (RoomStatus::CheckedIn, Action::UndoCheckIn) => Ok(RoomStatus::Confirmed),
        (RoomStatus::CheckedIn, Action::CheckOut) => Ok(RoomStatus::CheckedOut),
        _ => Err(invalid_transition(from, action)),
    }
}

/// The reservation's overall status, derived from its rooms' statuses (the `reservation` table has no status
/// column of its own): every room cancelled means cancelled; else any room checked in means checked in; else
/// every room checked out or cancelled, with at least one checked out, means checked out; else every room a
/// no-show or cancelled, with at least one no-show, means no-show; else any room confirmed means confirmed;
/// else tentative.
///
/// `None` for an empty slice: every real reservation has at least one room, so an empty slice only reaches
/// here through a caller bug.
pub fn reservation_status(rooms: &[RoomStatus]) -> Option<RoomStatus> {
    use RoomStatus::{Cancelled, CheckedIn, CheckedOut, Confirmed, NoShow, Tentative};

    if rooms.is_empty() {
        return None;
    }
    if rooms.iter().all(|status| *status == Cancelled) {
        return Some(Cancelled);
    }
    if rooms.contains(&CheckedIn) {
        return Some(CheckedIn);
    }
    if rooms.contains(&CheckedOut) && rooms.iter().all(|status| matches!(status, CheckedOut | Cancelled)) {
        return Some(CheckedOut);
    }
    if rooms.contains(&NoShow) && rooms.iter().all(|status| matches!(status, NoShow | Cancelled)) {
        return Some(NoShow);
    }
    if rooms.contains(&Confirmed) {
        return Some(Confirmed);
    }
    Some(Tentative)
}

#[cfg(test)]
mod tests {
    use super::{Action, InvalidTransition, RoomStatus, reservation_status, transition};

    const ALL_STATUSES: [RoomStatus; 6] = [
        RoomStatus::Tentative,
        RoomStatus::Confirmed,
        RoomStatus::CheckedIn,
        RoomStatus::CheckedOut,
        RoomStatus::Cancelled,
        RoomStatus::NoShow,
    ];

    const ALL_ACTIONS: [Action; 6] =
        [Action::Confirm, Action::CheckIn, Action::UndoCheckIn, Action::CheckOut, Action::Cancel, Action::NoShow];

    /// The only valid `(from, action)` pairs, straight from the diagram in
    /// `docs/specs/phase-3-reservations.md` § State machine — independent of [`transition`]'s own match, so
    /// this test cannot pass just because both copy the same mistake.
    fn spec(from: RoomStatus, action: Action) -> Option<RoomStatus> {
        match (from, action) {
            (RoomStatus::Tentative, Action::Confirm) => Some(RoomStatus::Confirmed),
            (RoomStatus::Tentative, Action::Cancel) => Some(RoomStatus::Cancelled),
            (RoomStatus::Confirmed, Action::CheckIn) => Some(RoomStatus::CheckedIn),
            (RoomStatus::Confirmed, Action::Cancel) => Some(RoomStatus::Cancelled),
            (RoomStatus::Confirmed, Action::NoShow) => Some(RoomStatus::NoShow),
            (RoomStatus::CheckedIn, Action::UndoCheckIn) => Some(RoomStatus::Confirmed),
            (RoomStatus::CheckedIn, Action::CheckOut) => Some(RoomStatus::CheckedOut),
            _ => None,
        }
    }

    #[test]
    fn every_status_and_action_pair_matches_the_spec_diagram() {
        let (mut valid, mut invalid) = (0, 0);
        for &from in &ALL_STATUSES {
            for &action in &ALL_ACTIONS {
                match (transition(from, action), spec(from, action)) {
                    (Ok(to), Some(want)) => {
                        assert_eq!(to, want, "{from:?} + {action:?} should reach {want:?}");
                        valid += 1;
                    }
                    (Err(InvalidTransition { from: err_from, action: err_action, message }), None) => {
                        assert_eq!(err_from, from);
                        assert_eq!(err_action, action);
                        assert!(!message.is_empty(), "{from:?} + {action:?} has no message");
                        invalid += 1;
                    }
                    (Ok(to), None) => panic!("{from:?} + {action:?} should be refused, but reached {to:?}"),
                    (Err(err), Some(want)) => {
                        panic!("{from:?} + {action:?} should reach {want:?}, but was refused: {err}")
                    }
                }
            }
        }
        // 6 statuses x 6 actions; exactly the 7 edges in the diagram are valid.
        assert_eq!(valid, 7);
        assert_eq!(invalid, 29);
    }

    #[test]
    fn a_checked_out_room_cannot_be_cancelled() {
        let err = transition(RoomStatus::CheckedOut, Action::Cancel).unwrap_err();
        assert_eq!(err.message, "a checked-out room can't be cancelled");
    }

    #[test]
    fn a_tentative_room_cannot_be_checked_in_directly() {
        let err = transition(RoomStatus::Tentative, Action::CheckIn).unwrap_err();
        assert_eq!(err.message, "a tentative room can't be checked in");
    }

    #[test]
    fn no_show_only_applies_to_a_confirmed_room() {
        assert!(transition(RoomStatus::Tentative, Action::NoShow).is_err());
        assert!(transition(RoomStatus::CheckedIn, Action::NoShow).is_err());
        assert_eq!(transition(RoomStatus::Confirmed, Action::NoShow), Ok(RoomStatus::NoShow));
    }

    #[test]
    fn every_status_round_trips_through_its_database_text() {
        for &status in &ALL_STATUSES {
            assert_eq!(RoomStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(RoomStatus::Tentative.as_str(), "tentative");
        assert_eq!(RoomStatus::Confirmed.as_str(), "confirmed");
        assert_eq!(RoomStatus::CheckedIn.as_str(), "checked_in");
        assert_eq!(RoomStatus::CheckedOut.as_str(), "checked_out");
        assert_eq!(RoomStatus::Cancelled.as_str(), "cancelled");
        assert_eq!(RoomStatus::NoShow.as_str(), "no_show");
        assert_eq!(RoomStatus::parse("unknown"), None);
    }

    #[test]
    fn serde_uses_the_database_text() {
        assert_eq!(serde_json::to_string(&RoomStatus::CheckedIn).unwrap(), "\"checked_in\"");
        assert_eq!(serde_json::to_string(&RoomStatus::NoShow).unwrap(), "\"no_show\"");
    }

    #[test]
    fn an_empty_reservation_has_no_derived_status() {
        assert_eq!(reservation_status(&[]), None);
    }

    #[test]
    fn every_room_cancelled_is_cancelled() {
        assert_eq!(reservation_status(&[RoomStatus::Cancelled, RoomStatus::Cancelled]), Some(RoomStatus::Cancelled));
        assert_eq!(reservation_status(&[RoomStatus::Cancelled]), Some(RoomStatus::Cancelled));
    }

    #[test]
    fn any_checked_in_room_wins_over_everything_else() {
        assert_eq!(
            reservation_status(&[RoomStatus::CheckedOut, RoomStatus::CheckedIn, RoomStatus::Cancelled]),
            Some(RoomStatus::CheckedIn)
        );
        assert_eq!(reservation_status(&[RoomStatus::Tentative, RoomStatus::CheckedIn]), Some(RoomStatus::CheckedIn));
    }

    #[test]
    fn checked_out_needs_no_open_rooms_and_at_least_one_checked_out() {
        assert_eq!(reservation_status(&[RoomStatus::CheckedOut, RoomStatus::CheckedOut]), Some(RoomStatus::CheckedOut));
        assert_eq!(reservation_status(&[RoomStatus::CheckedOut, RoomStatus::Cancelled]), Some(RoomStatus::CheckedOut));
        // A still-open room keeps the reservation out of "checked out", even with a checked-out room present.
        assert_eq!(reservation_status(&[RoomStatus::CheckedOut, RoomStatus::Tentative]), Some(RoomStatus::Tentative));
    }

    #[test]
    fn no_show_needs_no_open_rooms_and_at_least_one_no_show() {
        assert_eq!(reservation_status(&[RoomStatus::NoShow, RoomStatus::NoShow]), Some(RoomStatus::NoShow));
        assert_eq!(reservation_status(&[RoomStatus::NoShow, RoomStatus::Cancelled]), Some(RoomStatus::NoShow));
        assert_eq!(reservation_status(&[RoomStatus::NoShow, RoomStatus::Tentative]), Some(RoomStatus::Tentative));
    }

    #[test]
    fn any_confirmed_room_wins_over_tentative_and_finished_rooms() {
        assert_eq!(
            reservation_status(&[RoomStatus::Tentative, RoomStatus::Confirmed, RoomStatus::CheckedOut]),
            Some(RoomStatus::Confirmed)
        );
    }

    #[test]
    fn all_tentative_is_tentative() {
        assert_eq!(reservation_status(&[RoomStatus::Tentative, RoomStatus::Tentative]), Some(RoomStatus::Tentative));
    }
}
