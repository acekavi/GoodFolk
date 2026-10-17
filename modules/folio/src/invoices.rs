//! Invoices and credit notes: a settled folio's charges, taxes and payments snapshotted in whole LKR under a gapless
//! serial, the folio frozen until a credit note reopens it, and the PDF rendered by media-svc from the snapshot.
//!
//! **Serials.** An invoice is `<PROPERTY CODE>-<YYYY>-<NNNNNN>` and a credit note `CN-<PROPERTY CODE>-<YYYY>-<NNNNNN>`,
//! where YYYY is the year of the business date it was issued on and NNNNNN the property's `invoice` (or
//! `credit_note`) counter. The counter is one series per property that never restarts with the year: the year only
//! dates the document. It is taken inside the issuing transaction, after the folio's lock (the global lock order has
//! `folio` before `property_counter`), so a rolled-back issue gives its number back and concurrent issues queue on
//! the counter row instead of skipping or colliding.
//!
//! **Figures.** Every printed figure is whole rupees, rounded half away from zero, and the invoice total is exactly
//! the money settled: `total_lkr` is the sum of the lines' base totals, rounded once. A reversed line and its reversal
//! print as exact mirrors (see [`mirrored`]); the other lines' `total_lkr` share the total out by the largest
//! remainder method (see [`allocate`]), so they sum to it exactly. Each tax is converted at its
//! line's rate to base minor units, summed per tax and rate, and rounded once; only on-bill taxes are printed, since
//! an absorbed tax is not part of the bill. `net_lkr` is `total_lkr` less the printed taxes, so it takes the rounding
//! difference and the printed figures always add up. Reversals are printed as negative lines, so the corrections stay
//! visible.

use crate::folios::{FolioKind, FolioQuery, FolioStatus, LineWithTaxes, PaymentRow, folios_for, lock_folios};
use crate::lines::violates;
use crate::words::lkr_in_words;
use crate::{FolioError, audit, folio_key, notify, reason};
use db::{TenantId, Tx, UserId};
use media_types::{InvoiceData, InvoiceKind, InvoiceLine, InvoicePayment, InvoiceTax, Purchaser, Supplier};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

/// The outbox topic media-svc renders invoice PDFs from.
pub const INVOICE_RENDER_TOPIC: &str = "invoice-render";

/// The index that lets an invoice be credited once.
const CREDITED_ONCE_INDEX: &str = "invoice_credited_once_idx";

/// The only currency invoices are issued in.
const INVOICE_CURRENCY: &str = "LKR";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow, Serialize, utoipa::ToSchema)]
pub struct InvoiceRow {
    pub id: Uuid,
    pub property_id: Uuid,
    pub folio_id: Uuid,
    #[sqlx(try_from = "String")]
    #[schema(value_type = String)]
    pub kind: InvoiceKind,
    pub serial: String,
    /// The invoice a credit note credits.
    pub credits: Option<Uuid>,
    /// Why a credit note was issued.
    pub reason: Option<String>,
    /// The [`InvoiceData`] snapshot the PDF is rendered from.
    #[schema(value_type = Object)]
    pub data: serde_json::Value,
    pub total_lkr: i64,
    /// The rendered PDF's object key, once media-svc has rendered it.
    pub pdf_key: Option<String>,
    pub issued_by: Option<Uuid>,
    pub issued_at: OffsetDateTime,
}

const INVOICE_COLUMNS: &str =
    "id, property_id, folio_id, kind, serial, credits, reason, data, total_lkr, pdf_key, issued_by, issued_at";

/// What an invoice prints of its property.
#[derive(sqlx::FromRow)]
struct Issuer {
    code: String,
    legal_name: String,
    address: Option<String>,
    tin: Option<String>,
    vat_registered: bool,
    base_currency: String,
    business_date: Date,
}

async fn issuer(tx: &mut Tx, property: Uuid) -> Result<Issuer, FolioError> {
    sqlx::query_as(
        "select code, coalesce(legal_name, name) as legal_name, address, tin, vat_registered,
                base_currency::text as base_currency, business_date
         from property where id = $1",
    )
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(FolioError::NotFound("property"))
}

/// Issues the invoice of `folio`, which must be open (`Conflict`), have a zero balance in every currency (`Conflict`,
/// "The folio must be settled before it is invoiced") and at least one line (`Conflict`, "Nothing to invoice"), in a
/// property whose base currency is LKR (`Invalid`). A VAT-registered property needs its TIN set (`Conflict`). Every
/// refusal comes before the serial is taken.
///
/// Locks the folio (`NotFound` unless it is in `property`), then takes the next `invoice` serial, snapshots the
/// invoice data, marks the folio invoiced, audits `invoice.issued`, queues the `invoice-render` job and queues
/// `folio:<id>`.
pub async fn issue_invoice(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    property: Uuid,
    folio: Uuid,
) -> Result<InvoiceRow, FolioError> {
    let locked = lock_folios(tx, property, &[folio]).await?.pop().ok_or(FolioError::NotFound("folio"))?;
    if locked.status == FolioStatus::Invoiced {
        return Err(FolioError::Conflict("The folio is already invoiced".into()));
    }
    let issuer = issuer(tx, property).await?;
    if issuer.base_currency != INVOICE_CURRENCY {
        return Err(FolioError::Invalid("Invoices are issued in LKR".into()));
    }
    let view =
        folios_for(tx, FolioQuery::Folio { property, folio }).await?.pop().ok_or(FolioError::NotFound("folio"))?;
    if view.balances.iter().any(|balance| balance.amount != 0) {
        return Err(FolioError::Conflict("The folio must be settled before it is invoiced".into()));
    }
    let (Some(supply_from), Some(supply_to)) = (
        view.lines.iter().map(|line| line.line.business_date).min(),
        view.lines.iter().map(|line| line.line.business_date).max(),
    ) else {
        return Err(FolioError::Conflict("Nothing to invoice".into()));
    };
    if issuer.vat_registered && issuer.tin.is_none() {
        return Err(FolioError::Conflict("Set the property's TIN before issuing a tax invoice".into()));
    }
    let purchaser = purchaser(tx, locked.kind, folio).await?;
    let sequences = tax_sequences(tx, property).await?;

    let number = next_number(tx, tenant, property, "invoice").await?;
    let serial = format!("{}-{:04}-{number:06}", issuer.code, issuer.business_date.year());
    let figures = figures(&view.lines, &sequences)?;
    let data = InvoiceData {
        kind: InvoiceKind::Invoice,
        supplier: Supplier { legal_name: issuer.legal_name, address: issuer.address, tin: issuer.tin },
        purchaser,
        serial,
        title: if issuer.vat_registered { "TAX INVOICE" } else { "INVOICE" }.into(),
        issued_on: issuer.business_date,
        supply_from,
        supply_to,
        total_words: lkr_in_words(figures.total_lkr),
        lines: figures.lines,
        taxes: figures.taxes,
        net_lkr: figures.net_lkr,
        total_lkr: figures.total_lkr,
        payments: view.payments.iter().map(payment).collect(),
        vat_registered: issuer.vat_registered,
        credits_serial: None,
        reason: None,
    };
    let invoice = insert(tx, tenant, user, property, folio, &data, None).await?;
    set_status(tx, folio, FolioStatus::Invoiced).await?;
    let audit_data = serde_json::json!({ "serial": invoice.serial, "folio": folio, "total_lkr": invoice.total_lkr });
    audit(tx, tenant, user, "invoice.issued", "invoice", invoice.id, audit_data).await?;
    finish(tx, tenant, &invoice, &data).await?;
    Ok(invoice)
}

/// Issues a credit note for `invoice` with `reason` (1 to 500 characters): the invoice's figures negated under the
/// next `credit_note` serial. Reopens the folio for new lines, so a new invoice can be issued as usual.
///
/// Locks the invoice's folio. `NotFound` unless the invoice is in `property`; `Invalid` for a credit note; `Conflict`
/// for an invoice already credited. Audits `invoice.credited` on the invoice, queues the `invoice-render` job for the
/// credit note and queues `folio:<id>`.
pub async fn credit_note(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    property: Uuid,
    invoice: Uuid,
    reason_text: &str,
) -> Result<InvoiceRow, FolioError> {
    let reason = reason(reason_text)?;
    let folio: Uuid = sqlx::query_scalar("select folio_id from invoice where id = $1 and property_id = $2")
        .bind(invoice)
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(FolioError::NotFound("invoice"))?;
    lock_folios(tx, property, &[folio]).await?.pop().ok_or(FolioError::NotFound("invoice"))?;
    let original: InvoiceRow =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("select {INVOICE_COLUMNS} from invoice where id = $1")))
            .bind(invoice)
            .fetch_one(&mut **tx)
            .await?;
    if original.kind != InvoiceKind::Invoice {
        return Err(FolioError::Invalid("Only an invoice can be credited".into()));
    }
    let credited: bool = sqlx::query_scalar("select exists (select 1 from invoice where credits = $1)")
        .bind(invoice)
        .fetch_one(&mut **tx)
        .await?;
    if credited {
        return Err(already_credited());
    }
    let original_data: InvoiceData = serde_json::from_value(original.data.clone())
        .map_err(|err| FolioError::Conflict(format!("the invoice's data can't be read: {err}")))?;
    let issuer = issuer(tx, property).await?;

    let number = next_number(tx, tenant, property, "credit_note").await?;
    let serial = format!("CN-{}-{:04}-{number:06}", issuer.code, issuer.business_date.year());
    let data = negated(original_data, serial, issuer.business_date, reason);
    let note = insert(tx, tenant, user, property, folio, &data, Some((invoice, reason))).await?;
    set_status(tx, folio, FolioStatus::Open).await?;
    let audit_data = serde_json::json!({
        "serial": original.serial,
        "credit_note": note.id,
        "credit_note_serial": note.serial,
        "reason": reason,
        "total_lkr": note.total_lkr,
    });
    audit(tx, tenant, user, "invoice.credited", "invoice", invoice, audit_data).await?;
    finish(tx, tenant, &note, &data).await?;
    Ok(note)
}

/// The object key of the rendered PDF of `invoice`, for a short-lived link. Audited as `invoice.viewed`.
///
/// `NotFound` unless the invoice is in `property`; `Conflict` while the PDF is still being rendered.
pub async fn invoice_link_key(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    property: Uuid,
    invoice: Uuid,
) -> Result<String, FolioError> {
    let (serial, pdf_key): (String, Option<String>) =
        sqlx::query_as("select serial, pdf_key from invoice where id = $1 and property_id = $2")
            .bind(invoice)
            .bind(property)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(FolioError::NotFound("invoice"))?;
    let key = pdf_key.ok_or_else(|| FolioError::Conflict("The invoice PDF is still being prepared".into()))?;
    audit(tx, tenant, user, "invoice.viewed", "invoice", invoice, serde_json::json!({ "serial": serial })).await?;
    Ok(key)
}

/// Records the rendered PDF of `invoice`, for the internal events receiver. The key must name an object under
/// `t/<tenant>/invoices/<invoice>` (`Invalid` otherwise), so no link is ever signed for another document's file. The
/// first key stays: setting it again, or for an invoice that doesn't exist, changes nothing. Queues `folio:<id>`
/// when it sets the key.
pub async fn set_invoice_pdf(tx: &mut Tx, tenant: TenantId, invoice: Uuid, key: &str) -> Result<(), FolioError> {
    if !owned_key(tenant, invoice, key) {
        return Err(FolioError::Invalid(format!("{key:?} is not an object of invoice {invoice}")));
    }
    let changed: Option<(Uuid, Uuid)> = sqlx::query_as(
        "update invoice set pdf_key = $2 where id = $1 and tenant_id = $3 and pdf_key is null
         returning folio_id, property_id",
    )
    .bind(invoice)
    .bind(key)
    .bind(tenant.0)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some((folio, property)) = changed {
        notify(tx, tenant, property, vec![folio_key(folio)]).await?;
    }
    Ok(())
}

/// Whether `key` is `t/<tenant>/invoices/<invoice>` followed by an extension of 1 to 10 letters and digits (`.pdf`)
/// or by one file name that is not a dotfile and has no `..` (`/x.pdf`).
fn owned_key(tenant: TenantId, invoice: Uuid, key: &str) -> bool {
    let prefix = format!("t/{}/invoices/{invoice}", tenant.0);
    let Some(rest) = key.strip_prefix(&prefix) else {
        return false;
    };
    if let Some(extension) = rest.strip_prefix('.') {
        (1..=10).contains(&extension.len()) && extension.bytes().all(|b| b.is_ascii_alphanumeric())
    } else if let Some(name) = rest.strip_prefix('/') {
        !name.is_empty() && !name.starts_with('.') && !name.contains('/') && !name.contains("..")
    } else {
        false
    }
}

fn already_credited() -> FolioError {
    FolioError::Conflict("The invoice has already been credited".into())
}

/// The folio's purchaser: an account folio's account, a guest folio's primary guest, or a deposit folio's booker.
async fn purchaser(tx: &mut Tx, kind: FolioKind, folio: Uuid) -> Result<Purchaser, FolioError> {
    let query = match kind {
        FolioKind::Account => "select a.name, a.tin from folio f join account a on a.id = f.account_id where f.id = $1",
        FolioKind::Guest => {
            "select btrim(g.first_name || ' ' || g.last_name) as name, g.tin
             from folio f
             join reservation_room rr on rr.id = f.reservation_room_id
             join guest g on g.id = rr.primary_guest_id
             where f.id = $1"
        }
        FolioKind::Deposit => {
            "select btrim(g.first_name || ' ' || g.last_name) as name, g.tin
             from folio f
             join reservation r on r.id = f.reservation_id
             join guest g on g.id = r.booker_guest_id
             where f.id = $1"
        }
    };
    let (name, tin): (String, Option<String>) =
        sqlx::query_as(query).bind(folio).fetch_optional(&mut **tx).await?.ok_or(FolioError::NotFound("purchaser"))?;
    Ok(Purchaser { name, tin })
}

/// The lowest sequence of each tax code of the property, which orders the printed taxes.
async fn tax_sequences(tx: &mut Tx, property: Uuid) -> Result<HashMap<String, i32>, sqlx::Error> {
    let rows: Vec<(String, i32)> =
        sqlx::query_as("select code, min(sequence) from tax_rule where property_id = $1 group by code")
            .bind(property)
            .fetch_all(&mut **tx)
            .await?;
    Ok(rows.into_iter().collect())
}

/// Takes the next number of the property's counter `name`, under the counter row's lock until the transaction ends.
async fn next_number(tx: &mut Tx, tenant: TenantId, property: Uuid, name: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "insert into property_counter (tenant_id, property_id, name, value) values ($1, $2, $3, 1)
         on conflict (property_id, name) do update set value = property_counter.value + 1
         returning value",
    )
    .bind(tenant.0)
    .bind(property)
    .bind(name)
    .fetch_one(&mut **tx)
    .await
}

/// The printed lines and taxes of an invoice and its totals, in whole rupees.
struct Figures {
    lines: Vec<InvoiceLine>,
    taxes: Vec<InvoiceTax>,
    net_lkr: i64,
    total_lkr: i64,
}

fn figures(lines: &[LineWithTaxes], sequences: &HashMap<String, i32>) -> Result<Figures, FolioError> {
    let fit =
        |value: i128| i64::try_from(value).map_err(|_| FolioError::Invalid("the invoice total is too large".into()));
    let base_exponent = fx::exponent(INVOICE_CURRENCY);
    let unit = 10_i128.pow(base_exponent);
    let line_rupees = mirrored(lines, unit);
    let rest = lines.iter().zip(&line_rupees).filter(|(_, paired)| paired.is_none());
    let (shares, total) = allocate(rest.map(|(entry, _)| i128::from(entry.line.base_total)), unit);
    let mut shares = shares.into_iter();
    // `allocate` returns one share per unpaired line, in line order.
    let line_rupees: Vec<i128> =
        line_rupees.into_iter().map(|paired| paired.or_else(|| shares.next()).unwrap_or_default()).collect();
    let printed = lines
        .iter()
        .zip(line_rupees)
        .map(|(entry, total_lkr)| {
            let line = &entry.line;
            Ok(InvoiceLine {
                description: line.description.clone(),
                quantity: line.quantity,
                currency: line.currency.clone(),
                total: line.total,
                total_lkr: fit(total_lkr)?,
            })
        })
        .collect::<Result<Vec<_>, FolioError>>()?;
    // Base minor units of each tax, keyed by (sequence, code, rate) so the taxes print in the order they are
    // calculated.
    let mut taxes: BTreeMap<(i32, String, i32), (String, i128)> = BTreeMap::new();
    for entry in lines {
        let line = &entry.line;
        for tax in entry.taxes.iter().filter(|tax| tax.presentation == "on_bill") {
            let base = fx::to_base(tax.amount, fx::exponent(&line.currency), base_exponent, line.rate)?;
            let sequence = sequences.get(&tax.code).copied().unwrap_or(i32::MAX);
            let sum = taxes.entry((sequence, tax.code.clone(), tax.rate_bp)).or_insert_with(|| (tax.name.clone(), 0));
            sum.1 += i128::from(base);
        }
    }
    let taxes = taxes
        .into_iter()
        .map(|((_, _, rate_bp), (name, base))| {
            Ok(InvoiceTax { name, rate_bp, amount_lkr: fit(round_half_up(base, unit))? })
        })
        .collect::<Result<Vec<_>, FolioError>>()?;
    let tax_total: i128 = taxes.iter().map(|tax| i128::from(tax.amount_lkr)).sum();
    Ok(Figures { lines: printed, taxes, net_lkr: fit(total - tax_total)?, total_lkr: fit(total)? })
}

/// The whole rupees of each line that is a reversal, or the original of one, both on this folio (a reversal, or the
/// source side of a transfer): the original prints its base total rounded half up and the reversal exactly its
/// negation, so a pair always mirrors. A pair sums to zero in base, so leaving it out of [`allocate`] leaves the
/// total unchanged. `None` for every other line.
fn mirrored(lines: &[LineWithTaxes], unit: i128) -> Vec<Option<i128>> {
    let index: HashMap<Uuid, usize> = lines.iter().enumerate().map(|(i, entry)| (entry.line.id, i)).collect();
    let mut paired = vec![None; lines.len()];
    for (reversal, entry) in lines.iter().enumerate() {
        if let Some(&original) = entry.line.reverses.as_ref().and_then(|id| index.get(id)) {
            let rupees = round_half_up(i128::from(lines[original].line.base_total), unit);
            paired[original] = Some(rupees);
            paired[reversal] = Some(-rupees);
        }
    }
    paired
}

/// Whole units of `minors` (in units of `unit` minor units) that sum exactly to their rounded total, by the largest
/// remainder method: the total is the sum rounded half up; each amount is floored toward −∞ (so a negative amount's
/// remainder is measured from below, like a positive one's), and the amounts with the largest remainders, the
/// earliest first on a tie, each get one more unit until the parts sum to the total. Returns the parts and the total.
fn allocate(minors: impl Iterator<Item = i128>, unit: i128) -> (Vec<i128>, i128) {
    let minors: Vec<i128> = minors.collect();
    let total = round_half_up(minors.iter().sum(), unit);
    let mut parts: Vec<i128> = minors.iter().map(|minor| minor.div_euclid(unit)).collect();
    let short = total - parts.iter().sum::<i128>();
    let mut order: Vec<usize> = (0..minors.len()).collect();
    // A stable sort keeps the earlier line first among equal remainders.
    order.sort_by_key(|&index| std::cmp::Reverse(minors[index].rem_euclid(unit)));
    // The remainders sum to less than one unit per amount, so `short` is between 0 and the number of amounts.
    for &index in order.iter().take(usize::try_from(short).unwrap_or(0)) {
        parts[index] += 1;
    }
    (parts, total)
}

/// `minor` in whole units of `unit` minor units, rounded half away from zero, so a negative amount rounds as its
/// positive does.
fn round_half_up(minor: i128, unit: i128) -> i128 {
    let (quotient, remainder) = (minor / unit, minor % unit);
    if remainder * 2 >= unit {
        quotient + 1
    } else if remainder * 2 <= -unit {
        quotient - 1
    } else {
        quotient
    }
}

fn payment(payment: &PaymentRow) -> InvoicePayment {
    InvoicePayment { method: payment.method.clone(), amount: payment.amount, currency: payment.currency.clone() }
}

/// The credit note of `original`: its parties, supply period and figures (negated), dated `issued_on`, with no
/// payments.
fn negated(original: InvoiceData, serial: String, issued_on: Date, reason: &str) -> InvoiceData {
    let total_lkr = -original.total_lkr;
    InvoiceData {
        kind: InvoiceKind::CreditNote,
        title: "CREDIT NOTE".into(),
        issued_on,
        credits_serial: Some(original.serial),
        serial,
        reason: Some(reason.to_owned()),
        lines: original
            .lines
            .into_iter()
            .map(|line| InvoiceLine {
                quantity: -line.quantity,
                total: -line.total,
                total_lkr: -line.total_lkr,
                ..line
            })
            .collect(),
        taxes: original.taxes.into_iter().map(|tax| InvoiceTax { amount_lkr: -tax.amount_lkr, ..tax }).collect(),
        // The money stays on the reopened folio: a credit note records no payment.
        payments: Vec::new(),
        net_lkr: -original.net_lkr,
        total_lkr,
        total_words: lkr_in_words(total_lkr),
        ..original
    }
}

/// Inserts the document `data` describes; `credits` is the invoice a credit note credits and its reason.
async fn insert(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    property: Uuid,
    folio: Uuid,
    data: &InvoiceData,
    credits: Option<(Uuid, &str)>,
) -> Result<InvoiceRow, FolioError> {
    let json = serde_json::to_value(data).map_err(|err| FolioError::Invalid(err.to_string()))?;
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "insert into invoice (id, tenant_id, property_id, folio_id, kind, serial, credits, reason, data, total_lkr,
                              issued_by)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
         returning {INVOICE_COLUMNS}"
    )))
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(folio)
    .bind(data.kind.as_str())
    .bind(&data.serial)
    .bind(credits.map(|(invoice, _)| invoice))
    .bind(credits.map(|(_, reason)| reason))
    .bind(json)
    .bind(data.total_lkr)
    .bind(user.map(|user| user.0))
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| if violates(&err, CREDITED_ONCE_INDEX) { already_credited() } else { err.into() })
}

async fn set_status(tx: &mut Tx, folio: Uuid, status: FolioStatus) -> Result<(), sqlx::Error> {
    sqlx::query("update folio set status = $2, version = version + 1 where id = $1")
        .bind(folio)
        .bind(status.as_str())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Queues the render job of `invoice` and its folio's change event.
async fn finish(tx: &mut Tx, tenant: TenantId, invoice: &InvoiceRow, data: &InvoiceData) -> Result<(), FolioError> {
    let payload = serde_json::json!({ "tenant_id": tenant.0, "invoice_id": invoice.id, "data": data });
    events::enqueue(tx, INVOICE_RENDER_TOPIC, &payload).await?;
    notify(tx, tenant, invoice.property_id, vec![folio_key(invoice.folio_id)]).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{allocate, owned_key, round_half_up};
    use db::TenantId;
    use uuid::Uuid;

    #[test]
    fn rupees_round_half_away_from_zero() {
        for (minor, rupees) in [(0, 0), (49, 0), (50, 1), (149, 1), (150, 2), (-49, 0), (-50, -1), (-150, -2)] {
            assert_eq!(round_half_up(minor, 100), rupees, "{minor}");
        }
    }

    #[test]
    fn line_rupees_sum_to_the_rounded_total() {
        // 30 nights of 10,050.50: 301,515 in all, where rounding each night would print 301,530.
        let (parts, total) = allocate(std::iter::repeat_n(1_005_050, 30), 100);
        assert_eq!((total, parts.iter().sum::<i128>()), (301_515, 301_515));
        assert_eq!((parts[0], parts[14], parts[15], parts[29]), (10_051, 10_051, 10_050, 10_050));
        // A reversal's remainder is measured from below: -5,176.25 floors to -5,177 with 75 left over.
        assert_eq!(allocate([517_625, -517_625].into_iter(), 100), (vec![5_176, -5_176], 0));
        assert_eq!(allocate([517_625, -517_625, 1_035_250].into_iter(), 100), (vec![5_176, -5_176, 10_353], 10_353));
        // Equal remainders go to the earlier line.
        assert_eq!(allocate([150, 150, 150].into_iter(), 100), (vec![2, 2, 1], 5));
        assert_eq!(allocate(std::iter::empty(), 100), (vec![], 0));
    }

    #[test]
    fn a_pdf_key_must_be_the_invoices_own() {
        let (tenant, invoice) = (TenantId(Uuid::now_v7()), Uuid::now_v7());
        let prefix = format!("t/{}/invoices/{invoice}", tenant.0);
        for good in [format!("{prefix}.pdf"), format!("{prefix}/invoice.pdf")] {
            assert!(owned_key(tenant, invoice, &good), "{good}");
        }
        let other = Uuid::now_v7();
        for bad in [
            prefix.clone(),
            format!("{prefix}/"),
            format!("{prefix}/.pdf"),
            format!("{prefix}/../x.pdf"),
            format!("{prefix}/a/b.pdf"),
            format!("{prefix}0.pdf"),
            format!("{prefix}."),
            format!("{prefix}..pdf"),
            format!("{prefix}.p/df"),
            format!("{prefix}.abcdefghijk"),
            format!("{prefix}/a..pdf"),
            format!("t/{}/invoices/{other}.pdf", tenant.0),
            format!("t/{other}/invoices/{invoice}.pdf"),
        ] {
            assert!(!owned_key(tenant, invoice, &bad), "{bad}");
        }
    }
}
