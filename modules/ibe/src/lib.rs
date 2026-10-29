//! The public booking site (IBE): what a property's site shows, and the rooms it offers for a stay, priced with the
//! taxes on the bill.
//!
//! Every function takes a transaction scoped to the tenant of an enabled site (`db::ibe_resolve`), and only reads.

mod search;
mod site;

pub use search::{
    MAX_NIGHTS, MAX_ROOMS, MONTHS_AHEAD, MealOffer, NEXT_AVAILABLE_DAYS, NightPrice, PlanOffer, RoomAsk,
    RoomTypeResult, SearchRequest, SearchResult, TaxLine, cancellation_text, search,
};
pub use site::{Image, MediaKind, SiteMedia, SiteRoomType, SiteView, VideoSource, public_code, site};

#[derive(Debug, thiserror::Error)]
pub enum IbeError {
    /// The property or its site does not exist in this tenant.
    #[error("{0} not found")]
    NotFound(&'static str),
    /// A search outside the site's limits, such as too many nights.
    #[error("{0}")]
    Invalid(String),
    /// The property's tax rules cannot price a night (a broken rule set or an amount too large).
    #[error("the prices cannot be taxed: {0}")]
    Tax(#[from] tax::TaxError),
    /// A stored asset the site shows could not be read: never the visitor's doing.
    #[error("a booking site image could not be read: {0}")]
    Media(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

impl From<rates::RatesError> for IbeError {
    fn from(err: rates::RatesError) -> Self {
        match err {
            rates::RatesError::NotFound(what) => Self::NotFound(what),
            rates::RatesError::Database(err) => Self::Database(err),
            other => Self::Invalid(other.to_string()),
        }
    }
}

impl From<media::MediaError> for IbeError {
    fn from(err: media::MediaError) -> Self {
        match err {
            media::MediaError::NotFound(what) => Self::NotFound(what),
            media::MediaError::Database(err) => Self::Database(err),
            other => Self::Media(other.to_string()),
        }
    }
}
