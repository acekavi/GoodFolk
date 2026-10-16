//! Payments: desk payments and their refunds, charges to account, a reservation's deposit moving to its first stay,
//! and the cancellation penalty posted to the deposit folio. Payments are never changed: a refund is a new, negative
//! payment.

use crate::folios::{
    FolioKind, FolioStatus, Locked, PAYMENT_COLUMNS, PaymentRow, ensure_account_folio, ensure_deposit_folio,
    ensure_guest_folio, lock_folios,
};
use crate::lines::{NewCharge, check_charge, post_charge, post_locked};
use crate::{FolioError, Source, audit, folio_key, notify, reason};
use db::{TenantId, Tx, UserId};
use rust_decimal::Decimal;
use time::Date;
use uuid::Uuid;

/// The longest card approval code, in characters.
const MAX_APPROVAL: usize = 50;

/// The longest card brand, in characters.
const MAX_BRAND: usize = 20;

/// The longest bank transfer reference, in characters.
const MAX_REFERENCE: usize = 100;

/// The longest evidence text, in characters.
const MAX_EVIDENCE: usize = 500;

/// The shortest run of digits refused as a possible card number.
const CARD_NUMBER_DIGITS: usize = 12;

/// How a desk payment was made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Method {
    Cash,
    /// Paid on the card terminal: its approval code and the card's brand and last 4 digits, never the card number.
    CardTerminal {
        approval: String,
        last4: String,
        brand: String,
    },
    BankTransfer {
        reference: String,
    },
    /// Charged to the reservation's account: a credit here and a charge on the account's folio.
    Account,
}

impl Method {
    fn as_str(&self) -> &'static str {
        match self {
            Method::Cash => "cash",
            Method::CardTerminal { .. } => "card_terminal",
            Method::BankTransfer { .. } => "bank_transfer",
            Method::Account => "account",
        }
    }

    /// The payment's `details`, checked: no card number in any field (`Invalid`).
    fn details(&self) -> Result<serde_json::Value, FolioError> {
        Ok(match self {
            Method::Cash | Method::Account => serde_json::json!({}),
            Method::CardTerminal { approval, last4, brand } => {
                let approval = text("approval code", approval, MAX_APPROVAL)?;
                let brand = text("card brand", brand, MAX_BRAND)?;
                if !(last4.len() == 4 && last4.bytes().all(|b| b.is_ascii_digit())) {
                    return Err(FolioError::Invalid("the card's last 4 are exactly 4 digits".into()));
                }
                serde_json::json!({ "approval": approval, "last4": last4, "brand": brand })
            }
            Method::BankTransfer { reference } => {
                serde_json::json!({ "reference": text("bank reference", reference, MAX_REFERENCE)? })
            }
        })
    }
}

/// `value` trimmed: 1 to `max` characters and no card number (`Invalid`).
fn text<'a>(what: &str, value: &'a str, max: usize) -> Result<&'a str, FolioError> {
    let value = value.trim();
    if !(1..=max).contains(&value.chars().count()) {
        return Err(FolioError::Invalid(format!("the {what} has 1 to {max} characters")));
    }
    no_card_number(what, value)?;
    Ok(value)
}

/// `Invalid` when `value` holds a run of [`CARD_NUMBER_DIGITS`] or more digits, spaces and dashes between digits
/// ignored: a card number is never stored.
fn no_card_number(what: &str, value: &str) -> Result<(), FolioError> {
    if has_card_number(value) {
        return Err(FolioError::Invalid(format!("the {what} looks like a card number; never record one")));
    }
    Ok(())
}

fn has_card_number(value: &str) -> bool {
    let mut digits = 0;
    for c in value.chars() {
        if c.is_ascii_digit() {
            digits += 1;
            if digits >= CARD_NUMBER_DIGITS {
                return true;
            }
        } else if !(digits > 0 && (c == ' ' || c == '-')) {
            digits = 0;
        }
    }
    false
}

/// The trimmed evidence, if any is left: at most [`MAX_EVIDENCE`] characters and no card number (`Invalid`).
fn evidence(value: Option<&str>) -> Result<Option<&str>, FolioError> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => text("evidence", value, MAX_EVIDENCE).map(Some),
        None => Ok(None),
    }
}

/// A payment row to insert.
struct NewPayment<'a> {
    folio: Uuid,
    method: &'a str,
    amount: i64,
    currency: &'a str,
    base_amount: i64,
    rate: Decimal,
    rate_date: Date,
    details: serde_json::Value,
    evidence: Option<&'a str>,
    refund_of: Option<Uuid>,
    business_date: Date,
    transfer_id: Option<Uuid>,
}

async fn insert_payment(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    payment: NewPayment<'_>,
) -> Result<PaymentRow, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "insert into payment (id, tenant_id, folio_id, method, amount, currency, base_amount, rate, rate_date, details,
                              evidence, refund_of, business_date, transfer_id, taken_by)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
         returning {PAYMENT_COLUMNS}"
    )))
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(payment.folio)
    .bind(payment.method)
    .bind(payment.amount)
    .bind(payment.currency)
    .bind(payment.base_amount)
    .bind(payment.rate)
    .bind(payment.rate_date)
    .bind(payment.details)
    .bind(payment.evidence)
    .bind(payment.refund_of)
    .bind(payment.business_date)
    .bind(payment.transfer_id)
    .bind(user.map(|user| user.0))
    .fetch_one(&mut **tx)
    .await
}

/// What a payment in a currency is converted with: the property's business date, and the rate in force that day.
struct Conversion {
    business_date: Date,
    rate: Decimal,
    rate_date: Date,
    currency_exponent: u32,
    base_exponent: u32,
}

impl Conversion {
    fn to_base(&self, amount: i64) -> Result<i64, FolioError> {
        Ok(fx::to_base(amount, self.currency_exponent, self.base_exponent, self.rate)?)
    }
}

/// The conversion of `currency` on the property's business date. `Invalid` for a currency the property does not
/// accept; `Conflict` ("No exchange rate for <CUR> on <YYYY-MM-DD>") when no rate is in force.
async fn conversion(tx: &mut Tx, property: Uuid, currency: &str) -> Result<Conversion, FolioError> {
    let (business_date, base_currency, accepted): (Date, String, Vec<String>) = sqlx::query_as(
        "select business_date, base_currency::text, accepted_currencies::text[] from property where id = $1",
    )
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(FolioError::NotFound("property"))?;
    if !accepted.iter().any(|code| code == currency) {
        return Err(FolioError::Invalid(format!("{currency} is not a currency this property accepts")));
    }
    let rate = fx::rate_for(tx, property, currency, business_date)
        .await?
        .ok_or_else(|| FolioError::Conflict(format!("No exchange rate for {currency} on {business_date}")))?;
    Ok(Conversion {
        business_date,
        rate: rate.rate,
        rate_date: rate.source_date,
        currency_exponent: fx::exponent(currency),
        base_exponent: fx::exponent(&base_currency),
    })
}

fn check_open(folio: &Locked) -> Result<(), FolioError> {
    match folio.status {
        FolioStatus::Open => Ok(()),
        FolioStatus::Invoiced => {
            Err(FolioError::Conflict("the folio is invoiced; a credit note reopens it for payments".into()))
        }
    }
}

/// Where a folio's reservation bills: its confirmation number and account.
#[derive(sqlx::FromRow)]
struct Billing {
    confirmation_no: Option<String>,
    account_id: Option<Uuid>,
}

/// Takes a payment of `amount` (more than 0) in `currency` on `folio` of the property, with optional `evidence`.
///
/// The folio must be in the property (`NotFound`) and open (`Conflict`); the currency one the property accepts
/// (`Invalid`), with a rate on the business date (`Conflict`, "No exchange rate for <CUR> on <YYYY-MM-DD>"). Card
/// details and evidence never hold a card number (`Invalid`). The payment is stored with its base amount at that
/// rate, on the business date.
///
/// [`Method::Account`] needs a guest or deposit folio whose reservation has an account (`Invalid`): it also posts an
/// untaxed `adjustment` of the amount on the account's folio (the original lines carried the taxes), linked to the
/// payment by a `transfer_id`. Both folios are locked in ascending id order.
///
/// Audited as `folio.payment_taken` (never with card details beyond brand and last 4); queues `folio:<id>` of every
/// folio it touches.
#[allow(clippy::too_many_arguments)]
pub async fn take_payment(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    property: Uuid,
    folio: Uuid,
    method: Method,
    amount: i64,
    currency: &str,
    evidence_text: Option<&str>,
) -> Result<PaymentRow, FolioError> {
    if amount <= 0 {
        return Err(FolioError::Invalid("a payment is more than 0".into()));
    }
    let details = method.details()?;
    let evidence = evidence(evidence_text)?;

    let (kind, billing): (String, Billing) = {
        let row: Option<(String, Option<String>, Option<Uuid>)> = sqlx::query_as(
            "select f.kind, r.confirmation_no, r.account_id
             from folio f
             left join reservation_room rr on rr.id = f.reservation_room_id
             left join reservation r on r.id = coalesce(f.reservation_id, rr.reservation_id)
             where f.id = $1 and f.property_id = $2",
        )
        .bind(folio)
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?;
        let (kind, confirmation_no, account_id) = row.ok_or(FolioError::NotFound("folio"))?;
        (kind, Billing { confirmation_no, account_id })
    };
    let account_folio = match method {
        Method::Account => {
            let reservation_folio = kind == FolioKind::Guest.as_str() || kind == FolioKind::Deposit.as_str();
            let account = billing.account_id.filter(|_| reservation_folio).ok_or_else(|| {
                FolioError::Invalid("only a stay or deposit of a reservation billed to an account can charge it".into())
            })?;
            Some(ensure_account_folio(tx, tenant, property, account).await?)
        }
        _ => None,
    };

    let ids: Vec<Uuid> = std::iter::once(folio).chain(account_folio).collect();
    let locked = lock_folios(tx, property, &ids).await?;
    for id in &ids {
        check_open(locked.iter().find(|row| row.id == *id).ok_or(FolioError::NotFound("folio"))?)?;
    }
    let conversion = conversion(tx, property, currency).await?;
    let transfer_id = account_folio.map(|_| Uuid::now_v7());

    let payment = insert_payment(
        tx,
        tenant,
        user,
        NewPayment {
            folio,
            method: method.as_str(),
            amount,
            currency,
            base_amount: conversion.to_base(amount)?,
            rate: conversion.rate,
            rate_date: conversion.rate_date,
            details,
            evidence,
            refund_of: None,
            business_date: conversion.business_date,
            transfer_id,
        },
    )
    .await?;

    let mut keys = vec![folio_key(folio)];
    if let Some(account_folio) = account_folio {
        let target = locked.iter().find(|row| row.id == account_folio).ok_or(FolioError::NotFound("folio"))?;
        let charge = NewCharge {
            source: Source::Adjustment,
            category: "misc".into(),
            description: format!("Charged to account from {}", billing.confirmation_no.unwrap_or_default()),
            quantity: 1,
            unit_net: amount,
            currency: currency.to_owned(),
            business_date: conversion.business_date,
            posting_key: None,
            inclusive: false,
            untaxed: true,
        };
        let description = check_charge(&charge)?;
        post_locked(tx, tenant, user, target, charge, description, None, transfer_id).await?;
        keys.push(folio_key(account_folio));
    }

    let mut data = serde_json::json!({
        "payment": payment.id,
        "folio": folio,
        "amount": amount,
        "currency": currency,
        "method": payment.method,
    });
    if let Method::CardTerminal { .. } = method {
        data["brand"] = payment.details["brand"].clone();
        data["last4"] = payment.details["last4"].clone();
    }
    if let Some(account_folio) = account_folio {
        data["account_folio"] = serde_json::json!(account_folio);
    }
    audit(tx, tenant, user, "folio.payment_taken", "folio", folio, data).await?;
    notify(tx, tenant, property, keys).await?;
    Ok(payment)
}

/// Refunds `amount` (more than 0) of `payment` in the property, for `reason` (1 to 500 characters, no card number):
/// a negative payment in the same currency and method with `refund_of` set, at the business date's rate.
///
/// `NotFound` unless the payment is on a folio of the property. Its folio is locked first, so concurrent refunds of
/// one payment run one at a time, and together they never exceed it (`Invalid`). A refund, a charge to account
/// ("Reverse the account charge instead") and a transfer between folios are never refunded (`Invalid`). The folio
/// must be open (`Conflict`). The caller checks `FolioAdjust`. Audited as `folio.refunded`; queues `folio:<id>`.
#[allow(clippy::too_many_arguments)]
pub async fn refund(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    property: Uuid,
    payment: Uuid,
    amount: i64,
    reason_text: &str,
) -> Result<PaymentRow, FolioError> {
    if amount <= 0 {
        return Err(FolioError::Invalid("a refund is more than 0".into()));
    }
    let reason = reason(reason_text)?;
    no_card_number("reason", reason)?;
    let folio: Uuid = sqlx::query_scalar(
        "select p.folio_id from payment p join folio f on f.id = p.folio_id where p.id = $1 and f.property_id = $2",
    )
    .bind(payment)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(FolioError::NotFound("payment"))?;
    let locked = lock_folios(tx, property, &[folio]).await?.pop().ok_or(FolioError::NotFound("payment"))?;

    let original: PaymentRow =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("select {PAYMENT_COLUMNS} from payment where id = $1")))
            .bind(payment)
            .fetch_one(&mut **tx)
            .await?;
    if original.refund_of.is_some() || original.amount < 0 {
        return Err(FolioError::Invalid("a refund can't be refunded".into()));
    }
    match original.method.as_str() {
        "account" => return Err(FolioError::Invalid("Reverse the account charge instead".into())),
        "transfer" => return Err(FolioError::Invalid("a transfer between folios can't be refunded".into())),
        _ => {}
    }
    check_open(&locked)?;
    let refunded: i64 =
        sqlx::query_scalar("select coalesce(-sum(amount), 0)::bigint from payment where refund_of = $1")
            .bind(payment)
            .fetch_one(&mut **tx)
            .await?;
    let left = original.amount - refunded;
    if amount > left {
        return Err(FolioError::Invalid(format!("at most {left} of the payment is left to refund")));
    }

    let conversion = conversion(tx, property, &original.currency).await?;
    let refund = insert_payment(
        tx,
        tenant,
        user,
        NewPayment {
            folio,
            method: &original.method,
            amount: -amount,
            currency: &original.currency,
            base_amount: conversion.to_base(-amount)?,
            rate: conversion.rate,
            rate_date: conversion.rate_date,
            details: serde_json::json!({ "reason": reason }),
            evidence: None,
            refund_of: Some(payment),
            business_date: conversion.business_date,
            transfer_id: None,
        },
    )
    .await?;
    let data = serde_json::json!({
        "payment": refund.id,
        "refund_of": payment,
        "folio": folio,
        "amount": -amount,
        "currency": original.currency,
        "method": original.method,
        "reason": reason,
    });
    audit(tx, tenant, user, "folio.refunded", "folio", folio, data).await?;
    notify(tx, tenant, property, vec![folio_key(folio)]).await?;
    Ok(refund)
}

/// Moves the deposit folio's balance of `reservation`, in each currency, to the guest folio of the stay
/// `to_reservation_room`, at the reservation's first check-in: a no-op when another of its stays has been checked in,
/// or when nothing is left. Each currency moves as a pair of `transfer` payments sharing a `transfer_id`, one on
/// each folio, so both folios keep their base amounts exactly (the deposit folio clears in the base currency too);
/// the pair records the business date's rate (`Conflict` if none).
///
/// Called by check-in after the stay is checked in: locks both folios (in ascending id order) and nothing else.
/// `NotFound` unless the stay is the reservation's, in the property; `Conflict` for an invoiced folio. Audited as
/// `folio.deposit_moved`; queues both folios' keys.
pub async fn move_deposit(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    property: Uuid,
    reservation: Uuid,
    to_reservation_room: Uuid,
) -> Result<(), FolioError> {
    let found: bool = sqlx::query_scalar(
        "select exists (select 1 from reservation_room where id = $1 and reservation_id = $2 and property_id = $3)",
    )
    .bind(to_reservation_room)
    .bind(reservation)
    .bind(property)
    .fetch_one(&mut **tx)
    .await?;
    if !found {
        return Err(FolioError::NotFound("reservation room"));
    }
    let deposit = ensure_deposit_folio(tx, tenant, property, reservation).await?;
    let guest = ensure_guest_folio(tx, tenant, property, to_reservation_room).await?;
    let locked = lock_folios(tx, property, &[deposit, guest]).await?;

    // After the deposit folio's lock, so a concurrent first check-in of another stay has committed or not begun.
    let checked_in_before: bool = sqlx::query_scalar(
        "select exists (select 1 from reservation_room
                        where reservation_id = $1 and id <> $2 and checked_in_at is not null)",
    )
    .bind(reservation)
    .bind(to_reservation_room)
    .fetch_one(&mut **tx)
    .await?;
    if checked_in_before {
        return Ok(());
    }

    // `(currency, amount, base)`: charges less payments, per currency.
    let balances: Vec<(String, i64, i64)> = sqlx::query_as(
        "select currency, sum(amount)::bigint, sum(base)::bigint
         from (select currency::text as currency, total as amount, base_total as base from folio_line
               where folio_id = $1
               union all
               select currency::text, -amount, -base_amount from payment where folio_id = $1) as entries
         group by currency
         having sum(amount) <> 0
         order by currency",
    )
    .bind(deposit)
    .fetch_all(&mut **tx)
    .await?;
    if balances.is_empty() {
        return Ok(());
    }
    for folio in &locked {
        check_open(folio)?;
    }

    let mut moved = Vec::new();
    for (currency, amount, base) in balances {
        let conversion = conversion(tx, property, &currency).await?;
        let transfer_id = Uuid::now_v7();
        for (folio, sign, details) in [
            (deposit, 1, serde_json::json!({ "to_folio": guest })),
            (guest, -1, serde_json::json!({ "from_folio": deposit })),
        ] {
            insert_payment(
                tx,
                tenant,
                user,
                NewPayment {
                    folio,
                    method: "transfer",
                    amount: sign * amount,
                    currency: &currency,
                    base_amount: sign * base,
                    rate: conversion.rate,
                    rate_date: conversion.rate_date,
                    details,
                    evidence: None,
                    refund_of: None,
                    business_date: conversion.business_date,
                    transfer_id: Some(transfer_id),
                },
            )
            .await?;
        }
        moved.push(serde_json::json!({ "currency": currency, "amount": -amount, "transfer_id": transfer_id }));
    }
    let data = serde_json::json!({ "reservation_room": to_reservation_room, "to_folio": guest, "moved": moved });
    audit(tx, tenant, user, "folio.deposit_moved", "folio", deposit, data).await?;
    notify(tx, tenant, property, vec![folio_key(deposit), folio_key(guest)]).await?;
    Ok(())
}

/// Posts the cancellation penalty `amount` in `currency` on the deposit folio of `reservation` in the property,
/// as a `cancellation_fee` line in category `fee`, taxed by the property's rules (a penalty is a supply), on the
/// business date. Nothing for a zero penalty; `Invalid` for a negative one.
///
/// Posted once under the key `cancellation:<reservation>`, so a replay is a no-op. A reservation whose stays were
/// penalised in several currencies posts one line per currency, under `cancellation:<reservation>:<CUR>`.
#[allow(clippy::too_many_arguments)]
pub async fn post_cancellation_fee(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    property: Uuid,
    reservation: Uuid,
    amount: i64,
    currency: &str,
) -> Result<(), FolioError> {
    if amount < 0 {
        return Err(FolioError::Invalid("a cancellation penalty is not negative".into()));
    }
    if amount == 0 {
        return Ok(());
    }
    let deposit = ensure_deposit_folio(tx, tenant, property, reservation).await?;
    let (business_date, currencies): (Date, i64) = sqlx::query_as(
        "select p.business_date,
                (select count(distinct rr.currency) from reservation_room rr
                 where rr.reservation_id = $2 and rr.cancellation_penalty > 0)
         from property p where p.id = $1",
    )
    .bind(property)
    .bind(reservation)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(FolioError::NotFound("property"))?;
    let posting_key = if currencies > 1 {
        format!("cancellation:{reservation}:{currency}")
    } else {
        format!("cancellation:{reservation}")
    };
    let charge = NewCharge {
        source: Source::CancellationFee,
        category: "fee".into(),
        description: "Cancellation fee".into(),
        quantity: 1,
        unit_net: amount,
        currency: currency.to_owned(),
        business_date,
        posting_key: Some(posting_key),
        inclusive: false,
        untaxed: false,
    };
    post_charge(tx, tenant, user, property, deposit, charge).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::has_card_number;

    #[test]
    fn a_run_of_twelve_digits_is_a_card_number_whatever_separates_them() {
        for card in
            ["4111111111111111", "4111 1111 1111 1111", "4111-1111-1111-1111", "ref 123456789012", "4111 - 1111 1111"]
        {
            assert!(has_card_number(card), "{card}");
        }
        for fine in ["12345678901", "ABC123", "1234 5678 abc 9012", "", "2026-10-16 receipt 55", "1234-"] {
            assert!(!has_card_number(fine), "{fine}");
        }
    }
}
