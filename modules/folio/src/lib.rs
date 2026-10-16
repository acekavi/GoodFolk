//! Folios: the charges, taxes and payments of a stay, and the invoices issued from them.
//!
//! Every function takes a transaction scoped to the caller's tenant. Money rows are append-only: a correction is a
//! new line (a reversal or a transfer), never an edit. Writes queue a `folio:<id>` change event for every folio they
//! touch, and staff actions record an audit entry in the same transaction.
//!
//! A system posting for a stay (room, meal plan, laundry, POS) goes through [`post_for_stay`], never
//! [`post_charge`]: only `post_for_stay` applies the reservation's routing and checks a posting key across every
//! folio the stay posts to, so a replay after a routing change returns the existing line instead of posting twice.

mod folios;
mod lines;
mod routing;
mod rules;

pub use folios::{
    Balance, FolioKind, FolioQuery, FolioRow, FolioStatus, FolioView, LineTax, LineWithTaxes, PaymentRow,
    ensure_account_folio, ensure_deposit_folio, ensure_guest_folio, folios_for,
};
pub use lines::{LineRow, NewCharge, post_charge, reverse, transfer};
pub use routing::{DEFAULT_ROUTING, follow_account_change, post_for_stay, set_routing};
pub use rules::load_rules;

use db::{Event, TenantId, Tx, UserId};
use uuid::Uuid;

db::text_enum!(
    /// What posted a line.
    Source {
        Room = "room",
        MealPlan = "meal_plan",
        Laundry = "laundry",
        Pos = "pos",
        Manual = "manual",
        CancellationFee = "cancellation_fee",
        NoShowFee = "no_show_fee",
        Adjustment = "adjustment"
    }
);

#[derive(Debug, thiserror::Error)]
pub enum FolioError {
    /// The property or the named resource does not exist in this tenant.
    #[error("{0} not found")]
    NotFound(&'static str),
    /// `If-Match` named an older version of the named resource.
    #[error("the {0} was changed by someone else; reload and try again")]
    VersionMismatch(&'static str),
    /// The request clashes with other data, such as a charge on an invoiced folio or a missing exchange rate.
    #[error("{0}")]
    Conflict(String),
    /// A business rule, such as a currency the property does not accept.
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

impl From<fx::FxError> for FolioError {
    fn from(err: fx::FxError) -> Self {
        match err {
            fx::FxError::NotFound(what) => FolioError::NotFound(what),
            fx::FxError::Invalid(message) => FolioError::Invalid(message),
            fx::FxError::Database(err) => FolioError::Database(err),
            other => FolioError::Conflict(other.to_string()),
        }
    }
}

impl From<tax::TaxError> for FolioError {
    fn from(err: tax::TaxError) -> Self {
        match err {
            tax::TaxError::Overflow => FolioError::Invalid("the amount is too large".into()),
            other => FolioError::Conflict(format!("the property's tax rules are invalid: {other}")),
        }
    }
}

/// Cache key for one folio: its lines, taxes, payments and balances.
pub fn folio_key(folio: Uuid) -> String {
    format!("folio:{folio}")
}

/// Whether `category` is one a line can carry: `room`, `meal_plan`, `laundry`, `misc`, `fee` or `pos:<OUTLET>`.
fn valid_category(category: &str) -> bool {
    match category.strip_prefix("pos:") {
        Some(outlet) => {
            (1..=10).contains(&outlet.len()) && outlet.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        }
        None => matches!(category, "room" | "meal_plan" | "laundry" | "misc" | "fee"),
    }
}

fn check_category(category: &str) -> Result<(), FolioError> {
    if valid_category(category) {
        Ok(())
    } else {
        Err(FolioError::Invalid(format!(
            "{category:?} is not a category: use room, meal_plan, laundry, misc, fee or pos:<OUTLET>"
        )))
    }
}

/// The longest reversal or transfer reason, in characters.
const MAX_REASON: usize = 500;

/// The trimmed reason, 1 to [`MAX_REASON`] characters (`Invalid`).
fn reason(text: &str) -> Result<&str, FolioError> {
    let text = text.trim();
    if (1..=MAX_REASON).contains(&text.chars().count()) {
        Ok(text)
    } else {
        Err(FolioError::Invalid(format!("give a reason of 1 to {MAX_REASON} characters")))
    }
}

async fn audit(
    tx: &mut Tx,
    tenant: TenantId,
    actor: Option<UserId>,
    action: &str,
    entity: &str,
    id: Uuid,
    data: serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into audit_log (id, tenant_id, actor_user_id, action, entity, entity_id, data)
         values ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(actor.map(|user| user.0))
    .bind(action)
    .bind(entity)
    .bind(id)
    .bind(data)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Queues one change event for `keys`.
async fn notify(tx: &mut Tx, tenant: TenantId, property: Uuid, keys: Vec<String>) -> Result<(), sqlx::Error> {
    db::notify(tx, &Event { tenant_id: tenant, property_id: Some(property), keys }).await
}

#[cfg(test)]
mod tests {
    use super::valid_category;

    #[test]
    fn categories_are_the_fixed_set_and_outlets() {
        for good in ["room", "meal_plan", "laundry", "misc", "fee", "pos:BAR", "pos:R2", "pos:ABCDEFGHIJ"] {
            assert!(valid_category(good), "{good}");
        }
        for bad in ["", "Room", "pos:", "pos:bar", "pos:ABCDEFGHIJK", "pos:*", "spa", "pos:B-1"] {
            assert!(!valid_category(bad), "{bad}");
        }
    }
}
