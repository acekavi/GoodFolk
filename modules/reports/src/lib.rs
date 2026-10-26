//! The hotel's reports: the everyday reports read from the night audit's stored statistics, the daily lists, and the
//! liability reports computed live from closed business dates.
//!
//! Every function takes a transaction scoped to the caller's tenant and only reads.

mod everyday;
pub mod liability;

pub use everyday::{
    BreakdownTotal, DayRow, Dimension, Headline, ListKind, ListRow, RangeKind, Summary, daily_list, last_year_range,
    summary,
};
pub use liability::service_charge::{OutletServiceCharge, ServiceCharge, ServiceChargeReport, service_charge};
pub use liability::sscl::{CalendarQuarter, SsclMonth, SsclReport, SsclTotals, sscl};
pub use liability::tdl::{FiscalQuarter, TdlForm, tdl};
pub use liability::vat::{
    DUMMY_TIN, Schedule01Row, Schedule04Row, VAT_DOCUMENTS_SQL, VatPeriod, VatReport, VatTimingDifference,
    schedule_file_name, schedule01_csv, schedule04_csv, vat,
};

#[derive(Debug, thiserror::Error)]
pub enum ReportsError {
    /// The property does not exist in this tenant.
    #[error("{0} not found")]
    NotFound(&'static str),
    /// A malformed range or value.
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

impl From<audit::AuditError> for ReportsError {
    fn from(err: audit::AuditError) -> Self {
        match err {
            audit::AuditError::NotFound(what) => Self::NotFound(what),
            audit::AuditError::Database(err) => Self::Database(err),
            other => Self::Invalid(other.to_string()),
        }
    }
}

impl From<folio::FolioError> for ReportsError {
    fn from(err: folio::FolioError) -> Self {
        match err {
            folio::FolioError::NotFound(what) => Self::NotFound(what),
            folio::FolioError::Database(err) => Self::Database(err),
            other => Self::Invalid(other.to_string()),
        }
    }
}
