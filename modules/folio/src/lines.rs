//! Lines: taxed charges posted once, and the reversals and transfers that correct them. Lines are never changed.

use crate::folios::{FolioKind, FolioStatus, Locked, decimal_string, lock_folios};
use crate::{FolioError, Source, audit, check_category, folio_key, notify, reason};
use db::{TenantId, Tx, UserId};
use rust_decimal::Decimal;
use serde::Serialize;
use std::collections::HashMap;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

/// The longest line description, in characters.
const MAX_DESCRIPTION: usize = 200;

/// The longest posting key, in characters.
const MAX_POSTING_KEY: usize = 200;

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
    /// Set by system postings so a repeat is a no-op.
    pub posting_key: Option<String>,
    /// `unit_net` already includes the taxes; the net is back-calculated so net plus taxes equal the price exactly.
    pub inclusive: bool,
}

/// Refuses a malformed charge before anything is locked, and returns its trimmed description.
pub(crate) fn check_charge(charge: &NewCharge) -> Result<String, FolioError> {
    check_category(&charge.category)?;
    let description = charge.description.trim();
    if !(1..=MAX_DESCRIPTION).contains(&description.chars().count()) {
        return Err(FolioError::Invalid(format!("describe the charge in 1 to {MAX_DESCRIPTION} characters")));
    }
    if charge.quantity == 0 {
        return Err(FolioError::Invalid("the quantity can't be zero".into()));
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
/// Locks the folio. A `posting_key` the folio already has returns that line and posts nothing. Otherwise the folio
/// must be open (`Conflict`), the currency one the property accepts (`Invalid`), and a rate for it must exist on the
/// business date (`Conflict`, "No exchange rate for <CUR> on <YYYY-MM-DD>"). Queues `folio:<id>`; a `manual` charge
/// is audited as `folio.charge_posted`.
pub async fn post_charge(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    folio: Uuid,
    charge: NewCharge,
) -> Result<LineRow, FolioError> {
    let description = check_charge(&charge)?;
    let locked = lock_folios(tx, &[folio]).await?.pop().ok_or(FolioError::NotFound("folio"))?;
    if let Some(key) = &charge.posting_key
        && let Some(line) = line_by_key(tx, &[folio], key).await?
    {
        return Ok(line);
    }
    let resident = match (locked.kind, locked.reservation_room_id) {
        (FolioKind::Guest, Some(stay)) => stay_residency(tx, stay).await?,
        _ => None,
    };
    post_locked(tx, tenant, user, &locked, charge, description, resident).await
}

/// The line posted under `key` on any of `folios`, if there is one.
pub(crate) async fn line_by_key(tx: &mut Tx, folios: &[Uuid], key: &str) -> Result<Option<LineRow>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {LINE_COLUMNS} from folio_line where folio_id = any($1) and posting_key = $2 limit 1"
    )))
    .bind(folios)
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

/// Posts `charge` on the locked `folio`, whose posting key (if any) the caller has already looked up.
pub(crate) async fn post_locked(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    folio: &Locked,
    charge: NewCharge,
    description: String,
    resident: Option<bool>,
) -> Result<LineRow, FolioError> {
    check_open(folio)?;
    let (base_currency, accepted, vat_registered): (String, Vec<String>, bool) = sqlx::query_as(
        "select base_currency::text, accepted_currencies::text[], vat_registered from property where id = $1",
    )
    .bind(folio.property_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(FolioError::NotFound("property"))?;
    if !accepted.contains(&charge.currency) {
        return Err(FolioError::Invalid(format!("{} is not a currency this property accepts", charge.currency)));
    }

    let rules = crate::load_rules(tx, folio.property_id).await?;
    let context = tax::Context {
        date: charge.business_date,
        category: charge.category.clone(),
        vat_registered,
        resident,
        currency: charge.currency.clone(),
    };
    let amount = i64::from(charge.quantity)
        .checked_mul(charge.unit_net)
        .ok_or_else(|| FolioError::Invalid("the amount is too large".into()))?;
    let breakdown = if charge.inclusive {
        tax::back_calculate(amount, &rules, &context)?
    } else {
        tax::calculate(amount, &rules, &context)?
    };

    let rate = fx::rate_for(tx, folio.property_id, &charge.currency, charge.business_date).await?.ok_or_else(|| {
        FolioError::Conflict(format!("No exchange rate for {} on {}", charge.currency, charge.business_date))
    })?;
    let base_total =
        fx::to_base(breakdown.total, fx::exponent(&charge.currency), fx::exponent(&base_currency), rate.rate)?;

    let line: LineRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "insert into folio_line (id, tenant_id, folio_id, source, category, description, quantity, net, total,
                                 currency, base_total, rate, rate_date, business_date, posting_key, posted_by)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
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
    .fetch_one(&mut **tx)
    .await?;

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

fn kind_str(kind: tax::Kind) -> &'static str {
    match kind {
        tax::Kind::Tax => "tax",
        tax::Kind::Levy => "levy",
        tax::Kind::ServiceCharge => "service_charge",
    }
}

fn presentation_str(presentation: tax::Presentation) -> &'static str {
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
/// Locks the folio. `NotFound` if the line is not on the folio; `Invalid` for a reversal (a reversal is never
/// reversed); `Conflict` for a line already reversed or transferred, or an invoiced folio. Audited as
/// `folio.line_reversed`; queues `folio:<id>`.
pub async fn reverse(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    folio: Uuid,
    line: Uuid,
    reason_text: &str,
) -> Result<LineRow, FolioError> {
    let reason = reason(reason_text)?;
    let locked = lock_folios(tx, &[folio]).await?.pop().ok_or(FolioError::NotFound("folio"))?;
    let original: LineRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {LINE_COLUMNS} from folio_line where id = $1 and folio_id = $2"
    )))
    .bind(line)
    .bind(folio)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(FolioError::NotFound("line"))?;
    check_reversible(tx, &original).await?;
    check_open(&locked)?;

    let today = business_date(tx, locked.property_id).await?;
    let reversal = copy_line(tx, user, &original, folio, -1, today, Some(original.id), reason, None).await?;
    let data = serde_json::json!({ "line": original.id, "reversal": reversal.id, "reason": reason });
    audit(tx, tenant, user, "folio.line_reversed", "folio", folio, data).await?;
    notify(tx, tenant, locked.property_id, vec![folio_key(folio)]).await?;
    Ok(reversal)
}

/// Moves `from_line` to `to_folio`: a reversal on its folio and an identical line on the target, sharing a
/// `transfer_id`, both on the property's business date with `reason`.
///
/// Locks both folios in ascending id order. Both must be in the same property (`NotFound` otherwise) and open
/// (`Conflict`). A guest folio exchanges lines with its reservation's deposit folio, its reservation's account folio,
/// and any other stay's guest folio in the property (`Invalid` otherwise). The line must be neither a reversal
/// (`Invalid`) nor already reversed or transferred (`Conflict`). Audited as `folio.line_transferred`; queues both
/// folios' keys. Returns the reversal and the new line.
pub async fn transfer(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    from_line: Uuid,
    to_folio: Uuid,
    reason_text: &str,
) -> Result<(LineRow, LineRow), FolioError> {
    let reason = reason(reason_text)?;
    let from_folio: Uuid = sqlx::query_scalar("select folio_id from folio_line where id = $1")
        .bind(from_line)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(FolioError::NotFound("line"))?;
    if from_folio == to_folio {
        return Err(FolioError::Invalid("the line is already on that folio".into()));
    }
    let locked = lock_folios(tx, &[from_folio, to_folio]).await?;
    let source = locked.iter().find(|folio| folio.id == from_folio).ok_or(FolioError::NotFound("line"))?;
    let target = locked
        .iter()
        .find(|folio| folio.id == to_folio && folio.property_id == source.property_id)
        .ok_or(FolioError::NotFound("folio"))?;
    if !may_transfer(tx, source, target).await? {
        return Err(FolioError::Invalid(
            "a stay's line moves only to its reservation's deposit or account folio, or to another stay's folio".into(),
        ));
    }
    let original: LineRow =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("select {LINE_COLUMNS} from folio_line where id = $1")))
            .bind(from_line)
            .fetch_one(&mut **tx)
            .await?;
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

async fn business_date(tx: &mut Tx, property: Uuid) -> Result<Date, FolioError> {
    sqlx::query_scalar("select business_date from property where id = $1")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(FolioError::NotFound("property"))
}

/// A copy of `original` on `folio`, with every amount (its taxes' too) multiplied by `sign`, at the original's rate.
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
) -> Result<LineRow, sqlx::Error> {
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
    .await?;
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
