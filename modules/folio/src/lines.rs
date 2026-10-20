//! Lines: taxed charges posted once, and the reversals and transfers that correct them. Lines are never changed.

use crate::folios::{FolioKind, FolioStatus, Locked, decimal_string, lock_folios};
use crate::{FolioError, Source, audit, check_category, folio_key, notify, reason};
use db::{TenantId, Tx, UserId};
use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::Connection;
use std::collections::HashMap;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

/// The longest line description, in characters.
pub(crate) const MAX_DESCRIPTION: usize = 200;

/// The longest posting key, in characters.
const MAX_POSTING_KEY: usize = 200;

/// The largest quantity of a line, either way.
const MAX_QUANTITY: u32 = 9_999;

/// The largest unit price, either way, in minor units.
const MAX_UNIT_NET: u64 = 10_000_000_000_000;

/// The indexes that keep a posting key to one line per tenant, and (the older, narrower one) per folio.
const POSTING_KEY_INDEXES: [&str; 2] = ["folio_line_posting_key_idx", "folio_line_folio_id_posting_key_key"];

/// How a charge to account is undone: its account line moves back, never reversed and never refunded.
pub(crate) const ACCOUNT_CHARGE_UNDO: &str = "Move the account charge back to the stay instead";

/// The index that lets a line be reversed (or moved) once.
const REVERSED_ONCE_INDEX: &str = "folio_line_reversed_once_idx";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow, Serialize, utoipa::ToSchema)]
pub struct LineRow {
    pub id: Uuid,
    pub folio_id: Uuid,
    #[sqlx(try_from = "String")]
    pub source: Source,
    pub category: String,
    pub description: String,
    pub quantity: i32,
    /// Before taxes, in minor units of `currency`.
    pub net: i64,
    /// Net plus the on-bill taxes.
    pub total: i64,
    pub currency: String,
    /// `total` in the property's base currency at `rate`.
    pub base_total: i64,
    /// Base currency per one unit of `currency`.
    #[serde(serialize_with = "decimal_string")]
    #[schema(value_type = String)]
    pub rate: Decimal,
    /// The date the rate was published.
    pub rate_date: Date,
    pub business_date: Date,
    pub posting_key: Option<String>,
    /// The line this one reverses.
    pub reverses: Option<Uuid>,
    pub reason: Option<String>,
    /// Shared by the two lines of a transfer.
    pub transfer_id: Option<Uuid>,
    pub posted_by: Option<Uuid>,
    pub posted_at: OffsetDateTime,
}

pub(crate) const LINE_COLUMNS: &str = "id, folio_id, source, category, description, quantity, net, total, \
     currency::text as currency, base_total, rate, rate_date, business_date, posting_key, reverses, reason, \
     transfer_id, posted_by, posted_at";

/// A charge to post: `quantity × unit_net` in `currency`, before taxes, or after them when `inclusive`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCharge {
    pub source: Source,
    pub category: String,
    pub description: String,
    pub quantity: i32,
    pub unit_net: i64,
    pub currency: String,
    pub business_date: Date,
    /// Set by system postings so a repeat is a no-op: unique in the tenant, so it embeds a globally unique id.
    pub posting_key: Option<String>,
    /// `unit_net` already includes the taxes; the net is back-calculated so net plus taxes equal the price exactly.
    pub inclusive: bool,
}

/// How [`post_locked`] posts a charge beyond the charge itself.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Posting {
    /// Whether the stay's rate plan is sold to residents, for the tax rules.
    pub resident: Option<bool>,
    /// Links the line to the payment it moved.
    pub transfer_id: Option<Uuid>,
    /// Posted without taxes: an amount already taxed where it was first charged (a charge to account). Never set for
    /// a new supply, and never by staff.
    pub untaxed: bool,
    /// A system posting of an amount that already exists (a cancellation penalty): the currency need not be one the
    /// property accepts today. Staff charges always check it.
    pub internal: bool,
    /// Taxed without the property's service charge: a POS bill of an outlet that charges none. Set only by the POS
    /// path.
    pub exclude_service_charge: bool,
}

/// Refuses a malformed charge before anything is locked, and returns its trimmed description.
pub(crate) fn check_charge(charge: &NewCharge) -> Result<String, FolioError> {
    check_category(&charge.category)?;
    let description = charge.description.trim();
    if !(1..=MAX_DESCRIPTION).contains(&description.chars().count()) {
        return Err(FolioError::Invalid(format!("describe the charge in 1 to {MAX_DESCRIPTION} characters")));
    }
    if !(1..=MAX_QUANTITY).contains(&charge.quantity.unsigned_abs()) {
        return Err(FolioError::Invalid(format!("the quantity is 1 to {MAX_QUANTITY}, either way")));
    }
    if charge.unit_net.unsigned_abs() > MAX_UNIT_NET {
        return Err(FolioError::Invalid("the unit price is too large".into()));
    }
    if let Some(key) = &charge.posting_key
        && !(1..=MAX_POSTING_KEY).contains(&key.chars().count())
    {
        return Err(FolioError::Invalid(format!("a posting key has 1 to {MAX_POSTING_KEY} characters")));
    }
    Ok(description.to_owned())
}

/// Posts `charge` on `folio`, taxed with the property's rules effective on its business date and the stay's
/// residency, with its base-currency equivalent at the rate in force that day.
///
/// Locks the folio; `NotFound` unless it is in `property`. A `posting_key` already used on any of the property's
/// folios returns that line and posts nothing. Otherwise the folio must be open (`Conflict`), the currency one the
/// property accepts (`Invalid`), and a rate for it must exist on the business date (`Conflict`, "No exchange rate for
/// <CUR> on <YYYY-MM-DD>"). Queues `folio:<id>`; a `manual` charge is audited as `folio.charge_posted`.
///
/// A `manual` charge is a supply staff post: its quantity is at least 1 and its unit price at least 0 (`Invalid`).
/// Credits are never posted this way; a correction is a reversal. A charge in an outlet's category (`pos:<OUTLET>`)
/// needs that outlet to exist in the property and be active (`Invalid`), and is taxed as its bills are: without the
/// service charge when the outlet's is off.
pub async fn post_charge(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    property: Uuid,
    folio: Uuid,
    charge: NewCharge,
) -> Result<LineRow, FolioError> {
    let description = check_charge(&charge)?;
    if charge.source == Source::Manual {
        check_positive(&charge)?;
    }
    let locked = lock_folios(tx, property, &[folio]).await?.pop().ok_or(FolioError::NotFound("folio"))?;
    if let Some(key) = &charge.posting_key
        && let Some(line) = line_by_key(tx, property, key).await?
    {
        return Ok(line);
    }
    let resident = match (locked.kind, locked.reservation_room_id) {
        (FolioKind::Guest, Some(stay)) => stay_residency(tx, stay).await?,
        _ => None,
    };
    let exclude_service_charge = match charge.category.strip_prefix("pos:") {
        Some(code) => !outlet_service_charge(tx, property, code).await?,
        None => false,
    };
    let posting = Posting { resident, exclude_service_charge, ..Posting::default() };
    post_locked(tx, tenant, user, &locked, charge, description, posting).await
}

/// Whether the active outlet `code` of `property` charges the service charge; `Invalid` when the property has no such
/// outlet, or it is not active. Held `for share` while the charge posts, as a POS bill holds its outlet.
async fn outlet_service_charge(tx: &mut Tx, property: Uuid, code: &str) -> Result<bool, FolioError> {
    let outlet: Option<(bool, bool)> =
        sqlx::query_as("select service_charge, active from outlet where property_id = $1 and code = $2 for share")
            .bind(property)
            .bind(code)
            .fetch_optional(&mut **tx)
            .await?;
    match outlet {
        Some((service_charge, true)) => Ok(service_charge),
        Some((_, false)) => Err(FolioError::Invalid(format!("the outlet {code} is not active"))),
        None => Err(FolioError::Invalid(format!("the property has no outlet {code}"))),
    }
}

/// `Invalid` unless `charge` is a supply: a quantity of at least 1 and a unit price of at least 0. Staff charges and POS
/// items are; a credit is a reversal.
pub(crate) fn check_positive(charge: &NewCharge) -> Result<(), FolioError> {
    if charge.quantity < 1 || charge.unit_net < 0 {
        return Err(FolioError::Invalid(
            "a charge has a quantity of at least 1 and a unit price of at least 0; correct a line by reversing it"
                .into(),
        ));
    }
    Ok(())
}

/// The line posted under `key` on any of `property`'s folios, if there is one.
pub(crate) async fn line_by_key(tx: &mut Tx, property: Uuid, key: &str) -> Result<Option<LineRow>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {LINE_COLUMNS} from folio_line
         where posting_key = $2 and folio_id in (select id from folio where property_id = $1)"
    )))
    .bind(property)
    .bind(key)
    .fetch_optional(&mut **tx)
    .await
}

/// Whether the stay's rate plan is sold to residents (`Some(true)`), non-residents (`Some(false)`) or anyone.
pub(crate) async fn stay_residency(tx: &mut Tx, reservation_room: Uuid) -> Result<Option<bool>, sqlx::Error> {
    let residency: Option<Option<String>> = sqlx::query_scalar(
        "select rp.residency from reservation_room rr join rate_plan rp on rp.id = rr.rate_plan_id where rr.id = $1",
    )
    .bind(reservation_room)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(residency.flatten().map(|residency| residency == "resident"))
}

/// Posts `charge` on the locked `folio`, whose posting key (if any) the caller has already looked up, as `posting`
/// says.
pub(crate) async fn post_locked(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    folio: &Locked,
    charge: NewCharge,
    description: String,
    posting: Posting,
) -> Result<LineRow, FolioError> {
    let Posting { resident, transfer_id, untaxed, internal, exclude_service_charge } = posting;
    check_open(folio)?;
    let (base_currency, accepted, vat_registered): (String, Vec<String>, bool) = sqlx::query_as(
        "select base_currency::text, accepted_currencies::text[], vat_registered from property where id = $1",
    )
    .bind(folio.property_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(FolioError::NotFound("property"))?;
    if !internal && !accepted.contains(&charge.currency) {
        return Err(FolioError::Invalid(format!("{} is not a currency this property accepts", charge.currency)));
    }

    let amount = i64::from(charge.quantity)
        .checked_mul(charge.unit_net)
        .ok_or_else(|| FolioError::Invalid("the amount is too large".into()))?;
    let breakdown = if untaxed {
        tax::Breakdown { net: amount, components: Vec::new(), total: amount }
    } else {
        let mut rules = crate::load_rules(tx, folio.property_id).await?;
        if exclude_service_charge {
            crate::rules::without_service_charge(&mut rules);
        }
        let context = tax::Context {
            date: charge.business_date,
            category: charge.category.clone(),
            vat_registered,
            resident,
            currency: charge.currency.clone(),
        };
        if charge.inclusive {
            tax::back_calculate(amount, &rules, &context)?
        } else {
            tax::calculate(amount, &rules, &context)?
        }
    };

    let rate = fx::rate_for(tx, folio.property_id, &charge.currency, charge.business_date).await?.ok_or_else(|| {
        FolioError::Conflict(format!("No exchange rate for {} on {}", charge.currency, charge.business_date))
    })?;
    let base_total =
        fx::to_base(breakdown.total, fx::exponent(&charge.currency), fx::exponent(&base_currency), rate.rate)?;

    let mut savepoint = tx.begin().await?;
    let inserted: Result<LineRow, sqlx::Error> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "insert into folio_line (id, tenant_id, folio_id, source, category, description, quantity, net, total,
                                 currency, base_total, rate, rate_date, business_date, posting_key, posted_by,
                                 transfer_id)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)
         returning {LINE_COLUMNS}"
    )))
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(folio.id)
    .bind(charge.source.as_str())
    .bind(&charge.category)
    .bind(&description)
    .bind(charge.quantity)
    .bind(breakdown.net)
    .bind(breakdown.total)
    .bind(&charge.currency)
    .bind(base_total)
    .bind(rate.rate)
    .bind(rate.source_date)
    .bind(charge.business_date)
    .bind(&charge.posting_key)
    .bind(user.map(|user| user.0))
    .bind(transfer_id)
    .fetch_one(&mut *savepoint)
    .await;
    // A key another transaction posted since the lookup: its line is the answer, as for any replay.
    let line = match inserted {
        Ok(line) => {
            savepoint.commit().await?;
            line
        }
        Err(err) if POSTING_KEY_INDEXES.iter().any(|index| violates(&err, index)) => {
            savepoint.rollback().await?;
            let key = charge.posting_key.as_deref().unwrap_or_default();
            return line_by_key(tx, folio.property_id, key)
                .await?
                .ok_or_else(|| FolioError::Conflict("the posting key is used in another property".into()));
        }
        Err(err) => return Err(err.into()),
    };

    if !breakdown.components.is_empty() {
        let components = &breakdown.components;
        let rates = components
            .iter()
            .map(|component| i32::try_from(component.rate_bp))
            .collect::<Result<Vec<i32>, _>>()
            .map_err(|_| FolioError::Conflict("a tax rate is out of range".into()))?;
        sqlx::query(
            "insert into folio_line_tax (tenant_id, line_id, code, name, kind, rate_bp, base, amount, presentation)
             select $1, $2, t.* from unnest($3::text[], $4::text[], $5::text[], $6::int[], $7::bigint[],
                                            $8::bigint[], $9::text[]) as t",
        )
        .bind(tenant.0)
        .bind(line.id)
        .bind(components.iter().map(|component| component.code.as_str()).collect::<Vec<_>>())
        .bind(components.iter().map(|component| component.name.as_str()).collect::<Vec<_>>())
        .bind(components.iter().map(|component| kind_str(component.kind)).collect::<Vec<_>>())
        .bind(&rates)
        .bind(components.iter().map(|component| component.base).collect::<Vec<_>>())
        .bind(components.iter().map(|component| component.amount).collect::<Vec<_>>())
        .bind(components.iter().map(|component| presentation_str(component.presentation)).collect::<Vec<_>>())
        .execute(&mut **tx)
        .await?;
    }

    if charge.source == Source::Manual {
        let data = serde_json::json!({
            "line": line.id,
            "category": line.category,
            "description": line.description,
            "quantity": line.quantity,
            "total": line.total,
            "currency": line.currency,
        });
        audit(tx, tenant, user, "folio.charge_posted", "folio", folio.id, data).await?;
    }
    notify(tx, tenant, folio.property_id, vec![folio_key(folio.id)]).await?;
    Ok(line)
}

/// Whether `err` is a violation of the unique index `index`.
pub(crate) fn violates(err: &sqlx::Error, index: &str) -> bool {
    err.as_database_error().and_then(|db_err| db_err.constraint()).is_some_and(|name| name == index)
}

pub(crate) fn kind_str(kind: tax::Kind) -> &'static str {
    match kind {
        tax::Kind::Tax => "tax",
        tax::Kind::Levy => "levy",
        tax::Kind::ServiceCharge => "service_charge",
    }
}

pub(crate) fn presentation_str(presentation: tax::Presentation) -> &'static str {
    match presentation {
        tax::Presentation::OnBill => "on_bill",
        tax::Presentation::Absorbed => "absorbed",
    }
}

fn check_open(folio: &Locked) -> Result<(), FolioError> {
    match folio.status {
        FolioStatus::Open => Ok(()),
        FolioStatus::Invoiced => {
            Err(FolioError::Conflict("the folio is invoiced; a credit note reopens it for new lines".into()))
        }
    }
}

/// Reverses `line` of `folio` with an equal negative line carrying the original's taxes negated, posted on the
/// property's business date with `reason` (1 to 500 characters).
///
/// Locks the folio. `NotFound` if the folio is not in `property` or the line is not on it; `Invalid` for a reversal
/// (a reversal is never reversed) and for the account's line of a charge to account (it moves back to the stay);
/// `Conflict` for a line already reversed or transferred, or an invoiced folio. Audited as `folio.line_reversed`; queues `folio:<id>`.
pub async fn reverse(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    property: Uuid,
    folio: Uuid,
    line: Uuid,
    reason_text: &str,
) -> Result<LineRow, FolioError> {
    let reason = reason(reason_text)?;
    let locked = lock_folios(tx, property, &[folio]).await?.pop().ok_or(FolioError::NotFound("folio"))?;
    let original: LineRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {LINE_COLUMNS} from folio_line where id = $1 and folio_id = $2"
    )))
    .bind(line)
    .bind(folio)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(FolioError::NotFound("line"))?;
    check_reversible(tx, &original).await?;
    if let Some(transfer_id) = original.transfer_id {
        let charged_to_account: bool =
            sqlx::query_scalar("select exists (select 1 from payment where transfer_id = $1 and method = 'account')")
                .bind(transfer_id)
                .fetch_one(&mut **tx)
                .await?;
        if charged_to_account {
            return Err(FolioError::Invalid(ACCOUNT_CHARGE_UNDO.into()));
        }
    }
    check_open(&locked)?;

    let today = business_date(tx, locked.property_id).await?;
    let reversal = copy_line(tx, user, &original, folio, -1, today, Some(original.id), reason, None).await?;
    let data = serde_json::json!({
        "line": original.id,
        "reversal": reversal.id,
        "reason": reason,
        "total": original.total,
        "base_total": original.base_total,
        "currency": original.currency,
    });
    audit(tx, tenant, user, "folio.line_reversed", "folio", folio, data).await?;
    notify(tx, tenant, locked.property_id, vec![folio_key(folio)]).await?;
    Ok(reversal)
}

/// Moves `from_line` of `from_folio` to `to_folio`: a reversal on its folio and an identical line on the target,
/// sharing a `transfer_id`, both on the property's business date with `reason`.
///
/// `NotFound` unless the line is on `from_folio`, checked before anything is written. Locks both folios in ascending
/// id order. Both must be in `property` (`NotFound` otherwise) and open
/// (`Conflict`). A guest folio exchanges lines with its reservation's deposit folio, its reservation's account folio,
/// and any other stay's guest folio in the property (`Invalid` otherwise). The account's line of a charge to account
/// always moves back to the folio whose payment charged it, even a deposit folio, or after the reservation's account
/// has changed or the reservation was cancelled. The line must be neither a reversal
/// (`Invalid`) nor already reversed or transferred (`Conflict`). Audited as `folio.line_transferred`; queues both
/// folios' keys. Returns the reversal and the new line.
#[allow(clippy::too_many_arguments)]
pub async fn transfer(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    property: Uuid,
    from_folio: Uuid,
    from_line: Uuid,
    to_folio: Uuid,
    reason_text: &str,
) -> Result<(LineRow, LineRow), FolioError> {
    let reason = reason(reason_text)?;
    let on_folio: bool = sqlx::query_scalar(
        "select exists (select 1 from folio_line l join folio f on f.id = l.folio_id
                        where l.id = $1 and l.folio_id = $2 and f.property_id = $3)",
    )
    .bind(from_line)
    .bind(from_folio)
    .bind(property)
    .fetch_one(&mut **tx)
    .await?;
    if !on_folio {
        return Err(FolioError::NotFound("line"));
    }
    if from_folio == to_folio {
        return Err(FolioError::Invalid("the line is already on that folio".into()));
    }
    let locked = lock_folios(tx, property, &[from_folio, to_folio]).await?;
    let source = locked.iter().find(|folio| folio.id == from_folio).ok_or(FolioError::NotFound("line"))?;
    let target = locked.iter().find(|folio| folio.id == to_folio).ok_or(FolioError::NotFound("folio"))?;
    let original: LineRow =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("select {LINE_COLUMNS} from folio_line where id = $1")))
            .bind(from_line)
            .fetch_one(&mut **tx)
            .await?;
    if !moves_back_to_payer(tx, &original, to_folio).await? && !may_transfer(tx, source, target).await? {
        return Err(FolioError::Invalid(
            "a stay's line moves only to its reservation's deposit or account folio, or to another stay's folio".into(),
        ));
    }
    check_reversible(tx, &original).await?;
    check_open(source)?;
    check_open(target)?;

    let today = business_date(tx, source.property_id).await?;
    let transfer_id = Uuid::now_v7();
    let reversal =
        copy_line(tx, user, &original, from_folio, -1, today, Some(original.id), reason, Some(transfer_id)).await?;
    let moved = copy_line(tx, user, &original, to_folio, 1, today, None, reason, Some(transfer_id)).await?;
    let data = serde_json::json!({
        "line": original.id,
        "to_folio": to_folio,
        "reversal": reversal.id,
        "moved": moved.id,
        "transfer_id": transfer_id,
        "reason": reason,
        "total": original.total,
        "base_total": original.base_total,
        "currency": original.currency,
    });
    audit(tx, tenant, user, "folio.line_transferred", "folio", from_folio, data).await?;
    notify(tx, tenant, source.property_id, vec![folio_key(from_folio), folio_key(to_folio)]).await?;
    Ok((reversal, moved))
}

/// `Invalid` for a reversal, `Conflict` for a line that already has one.
async fn check_reversible(tx: &mut Tx, line: &LineRow) -> Result<(), FolioError> {
    if line.reverses.is_some() {
        return Err(FolioError::Invalid("a reversal can't be reversed or moved".into()));
    }
    let reversed: bool = sqlx::query_scalar("select exists (select 1 from folio_line where reverses = $1)")
        .bind(line.id)
        .fetch_one(&mut **tx)
        .await?;
    if reversed {
        return Err(FolioError::Conflict("the line has already been reversed or moved".into()));
    }
    Ok(())
}

/// Whether `line` is the account's line of a charge to account and `to_folio` the folio whose payment charged it:
/// the undo of a charge to account, allowed whatever that folio's kind and whatever account the reservation is
/// billed to now.
async fn moves_back_to_payer(tx: &mut Tx, line: &LineRow, to_folio: Uuid) -> Result<bool, sqlx::Error> {
    let Some(transfer_id) = line.transfer_id else {
        return Ok(false);
    };
    sqlx::query_scalar(
        "select exists (select 1 from payment where transfer_id = $1 and method = 'account' and folio_id = $2)",
    )
    .bind(transfer_id)
    .bind(to_folio)
    .fetch_one(&mut **tx)
    .await
}

/// Whether a line may move between the folios `a` and `b` (in either direction), which are in one property.
async fn may_transfer(tx: &mut Tx, a: &Locked, b: &Locked) -> Result<bool, sqlx::Error> {
    // Each folio's reservation (a stay's, or a deposit folio's own) and that reservation's account.
    let rows: Vec<(Uuid, Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        "select f.id, r.id, r.account_id
         from folio f
         left join reservation_room rr on rr.id = f.reservation_room_id
         left join reservation r on r.id = coalesce(f.reservation_id, rr.reservation_id)
         where f.id = any($1)",
    )
    .bind([a.id, b.id])
    .fetch_all(&mut **tx)
    .await?;
    let context: HashMap<Uuid, (Option<Uuid>, Option<Uuid>)> =
        rows.into_iter().map(|(id, reservation, account)| (id, (reservation, account))).collect();
    let (guest, other) = match (a.kind, b.kind) {
        (FolioKind::Guest, FolioKind::Guest) => return Ok(true),
        (FolioKind::Guest, _) => (a, b),
        (_, FolioKind::Guest) => (b, a),
        _ => return Ok(false),
    };
    let (reservation, account) = context.get(&guest.id).copied().unwrap_or_default();
    Ok(match other.kind {
        FolioKind::Deposit => reservation.is_some() && other.reservation_id == reservation,
        FolioKind::Account => account.is_some() && other.account_id == account,
        FolioKind::Guest => true,
    })
}

pub(crate) async fn business_date(tx: &mut Tx, property: Uuid) -> Result<Date, FolioError> {
    sqlx::query_scalar("select business_date from property where id = $1")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(FolioError::NotFound("property"))
}

/// A copy of `original` on `folio`, with every amount (its taxes' too) multiplied by `sign`, at the original's rate.
/// The copy carries no posting key: the original keeps it. A second reversal of `original` that a concurrent
/// transaction committed first is a `Conflict`.
#[allow(clippy::too_many_arguments)]
async fn copy_line(
    tx: &mut Tx,
    user: Option<UserId>,
    original: &LineRow,
    folio: Uuid,
    sign: i32,
    business_date: Date,
    reverses: Option<Uuid>,
    reason: &str,
    transfer_id: Option<Uuid>,
) -> Result<LineRow, FolioError> {
    let line: LineRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "insert into folio_line (id, tenant_id, folio_id, source, category, description, quantity, net, total,
                                 currency, base_total, rate, rate_date, business_date, reverses, reason, transfer_id,
                                 posted_by)
         select $1, tenant_id, $2, source, category, description, $3 * quantity, $3::bigint * net,
                $3::bigint * total, currency, $3::bigint * base_total, rate, rate_date, $4, $5, $6, $7, $8
         from folio_line where id = $9
         returning {LINE_COLUMNS}"
    )))
    .bind(Uuid::now_v7())
    .bind(folio)
    .bind(sign)
    .bind(business_date)
    .bind(reverses)
    .bind(reason)
    .bind(transfer_id)
    .bind(user.map(|user| user.0))
    .bind(original.id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| {
        if violates(&err, REVERSED_ONCE_INDEX) {
            FolioError::Conflict("This line was already reversed".into())
        } else {
            err.into()
        }
    })?;
    sqlx::query(
        "insert into folio_line_tax (tenant_id, line_id, code, name, kind, rate_bp, base, amount, presentation)
         select tenant_id, $1, code, name, kind, rate_bp, $2::bigint * base, $2::bigint * amount, presentation
         from folio_line_tax where line_id = $3",
    )
    .bind(line.id)
    .bind(sign)
    .bind(original.id)
    .execute(&mut **tx)
    .await?;
    Ok(line)
}
