//! The night audit: the review of a property's business date before it closes.
//!
//! [`night_audit`] reads the five review lists for the business date D: the arrivals not checked in, the departures
//! not checked out, D's POS bills by outlet, the booking bills (a dry run of tonight's room and meal-plan postings
//! and the day's manual, fee and laundry lines) and D's payments. Its blocking count, and [`blocking`]'s, is the
//! unresolved arrivals plus the unresolved departures: the close waits until both are resolved.
//!
//! The review only reads, so it takes no locks.
//!
//! [`compute`] takes a business date's statistics, [`store`] snapshots them when the day closes, and [`daily_stats`]
//! reads them back by date range.
//!
//! [`start_run`] opens the close of the business date, and [`execute`] runs its steps (`resolve`, `post_charges`,
//! `lock_fx`, then `snapshot_stats` and `advance_business_date` together), each committed with its step row, under the
//! property's session advisory lock: a run stopped anywhere resumes where it stopped. A failed run waits for
//! [`retry_run`]; an automatic one failing its third attempt raises the `audit` ops alert ([`alerts`]).

pub mod alerts;
mod close;
mod review;
mod stats;

pub use alerts::{AlertKind, OpsAlert};
pub use close::{
    AuditRun, CloseHooks, Mode, NoHooks, RunStatus, RunStep, Step, execute, get_run, list_runs, retry_run, start_run,
};

pub use stats::{BreakdownRow, DAY_LINES_SQL, DailyStats, compute, daily_stats, store};

pub use review::{
    ARRIVALS_SQL, ArrivalRow, BookingBills, CategoryTotal, DEPARTURES_SQL, DepartureRow, NightAudit, OutletBills,
    PaymentGroup, PosBillRow, blocking, night_audit,
};

use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    /// The property or the named resource does not exist in this tenant.
    #[error("{0} not found")]
    NotFound(&'static str),
    /// The request clashes with the property's state.
    #[error("{0}")]
    Conflict(String),
    /// A business rule.
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Folio(#[from] folio::FolioError),
    #[error(transparent)]
    Reservations(#[from] reservations::ReservationsError),
    /// Only from a test's CloseHooks: the run stops as if the process died (left `running`).
    #[error("interrupted")]
    Interrupted,
}

impl From<fx::FxError> for AuditError {
    fn from(err: fx::FxError) -> Self {
        match err {
            fx::FxError::NotFound(what) => AuditError::NotFound(what),
            fx::FxError::Invalid(message) => AuditError::Invalid(message),
            fx::FxError::Database(err) => AuditError::Database(err),
            other => AuditError::Conflict(other.to_string()),
        }
    }
}

/// Cache key for a property's night audit: its review and its runs.
pub fn audit_key(property: Uuid) -> String {
    format!("audit:{property}")
}
