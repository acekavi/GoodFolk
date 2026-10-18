//! Folios: the charges, taxes and payments of a stay, and the invoices issued from them.
//!
//! Every function takes a transaction scoped to the caller's tenant. Money rows are append-only: a correction is a
//! new line (a reversal or a transfer), never an edit. Writes queue a `folio:<id>` change event for every folio they
//! touch, and staff actions record an audit entry in the same transaction.
//!
//! Every write names its property, and a folio (or a line's folio) of another property is `NotFound`.
//!
//! A posting key names one charge in the whole tenant (a unique index holds it), and every key embeds a globally
//! unique id, such as the stay's or the POS bill's. A replay of a key returns the line already posted and posts
//! nothing, wherever routing or a change of account has since sent that charge. A key whose line was reversed stays
//! used: its replay is still a no-op, and a correction is a manual charge. Reversal and transfer lines carry no key.
//!
//! A system posting for a stay (room, meal plan, laundry, POS) goes through [`post_for_stay`], never
//! [`post_charge`]: only `post_for_stay` applies the reservation's routing.
//!
//! A stay's nights post as `room` and `meal_plan` lines under their posting keys: a business date's for every
//! in-house stay ([`post_business_date_charges`], one transaction per stay, a stay that fails reported and skipped),
//! and at check-out every night before `min(departure, business date + 1)` still unposted ([`post_unposted_nights`]);
//! an early check-out reverses the lines of the nights it releases, and undoing a check-in the night it posted
//! ([`reverse_released_nights`]), each having locked every folio of the stay first ([`lock_stay_folios`]).
//!
//! A payment is positive, with its base amount at the business date's rate; a refund is negative, with the
//! original's base in proportion, so a payment and its refunds net to zero in the base currency too. A charge to
//! account (a payment here, an untaxed `adjustment` on the account's folio) and a deposit moved to the first stay (a
//! `transfer` payment on each folio) are pairs sharing a `transfer_id`.
//!
//! A refund comes from a folio its caller names (the payment's own by default, or another folio of the same
//! reservation), which must hold the credit: where a deposit's money sits is never inferred. Undoing a check-in
//! moves back to the deposit folio what the stay still holds of the deposit. If another stay of the reservation is
//! already checked in by then, the deposit is stranded on the deposit folio: it stays refundable there, and staff can
//! move it on.
//!
//! A charge to account is undone by moving its account line back to the folio whose payment charged it, which is
//! always allowed: whatever that folio's kind, and even once the reservation is cancelled or billed to another
//! account. Any other line on the folio of an account the reservation is no longer billed to can't be transferred
//! to the stay; reverse it and post a manual charge instead.
//!
//! A POS bill ([`post_pos_bill`]) is posted once per outlet and bill number: a replay of the same body returns the
//! bill first posted, and a different body under that number is a `Conflict`. A bill to an in-house stay posts each
//! item through [`post_for_stay`] under `pos:<bill>:<n>`; a bill settled at the outlet is recorded with its taxed
//! lines and a payment on no folio. An outlet with its service charge off is taxed as if the property had none.
//!
//! An invoice ([`issue_invoice`]) is issued on a settled folio in whole LKR under a gapless per-property serial, and
//! freezes the folio: every money path refuses an invoiced folio until a credit note ([`credit_note`]) reopens it.
//! An account holds one open folio per property: once it is invoiced, the account's next charge (routed or charged
//! to account) opens a fresh one ([`ensure_account_folio`]), and the account is read as all of its folios. A credit
//! note on an invoiced account folio is refused while a newer one is open, so the account never has two open
//! ledgers.
//! Each queues an `invoice-render` job carrying its [`media_types::InvoiceData`] snapshot; media-svc renders the PDF,
//! whose key arrives through [`set_invoice_pdf`].
//!
//! A property's tax rules change through [`create_tax_rule`], [`update_tax_rule`], [`end_tax_rule`] and
//! [`apply_preset`], each checked against the whole resulting rule set with `tax::validate`.

mod folios;
mod invoices;
mod lines;
mod payments;
mod pos;
mod posting;
mod routing;
mod rules;
mod words;

pub use folios::{
    Balance, FolioKind, FolioQuery, FolioRow, FolioStatus, FolioView, LineTax, LineWithTaxes, MAX_FOLIO_ROWS,
    PaymentRow, ensure_account_folio, ensure_deposit_folio, ensure_guest_folio, folios_for,
};
pub use invoices::{
    INVOICE_RENDER_TOPIC, InvoiceRow, InvoiceSummary, credit_note, invoice_link_key, invoice_summaries, issue_invoice,
    set_invoice_pdf,
};
pub use lines::{LineRow, NewCharge, post_charge, reverse, transfer};
pub use payments::{Method, move_deposit, post_cancellation_fee, refund, take_payment, undo_deposit_move};
pub use pos::{
    NewOutlet, OutletChanges, OutletRow, PosBillIn, PosBillRow, PosItem, PosLine, PosLineTax, PosOutcome, PosTarget,
    create_outlet, list_outlets, post_pos_bill, update_outlet,
};
pub use posting::{
    PostReport, lock_stay_folios, post_business_date_charges, post_unposted_nights, reverse_released_nights,
};
pub use routing::{DEFAULT_ROUTING, follow_account_change, post_for_stay, routing, set_routing};
pub use rules::{
    PresetApplied, TaxConditionSchema, TaxRuleChanges, TaxRuleRow, apply_preset, create_tax_rule, end_tax_rule,
    list_tax_rules, load_rules, update_tax_rule,
};
pub use words::lkr_in_words;

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
        Some(outlet) => valid_outlet_code(outlet),
        None => matches!(category, "room" | "meal_plan" | "laundry" | "misc" | "fee"),
    }
}

/// Whether `code` is an outlet code: 1 to 10 upper-case letters and digits.
fn valid_outlet_code(code: &str) -> bool {
    (1..=10).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
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
