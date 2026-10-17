//! Outlets and POS bills: a bill posted once per outlet and bill number, either to an in-house stay's folios or
//! settled at the outlet with its own payment.

use crate::lines::{MAX_DESCRIPTION, NewCharge, business_date, check_charge, kind_str, presentation_str, violates};
use crate::payments::{Method, pay_pos_bill};
use crate::routing::post_routed;
use crate::rules::without_service_charge;
use crate::{FolioError, Source, audit, valid_outlet_code};
use db::{TenantId, Tx, UserId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::Connection;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

/// The most items on one bill.
const MAX_ITEMS: usize = 200;

/// The longest bill number, in characters.
const MAX_BILL_NO: usize = 50;

/// The longest outlet name, in characters.
const MAX_OUTLET_NAME: usize = 100;

/// The index that keeps an outlet's code to one outlet per property.
const OUTLET_CODE_KEY: &str = "outlet_property_id_code_key";

/// The index that keeps a bill number to one bill per outlet.
const BILL_NO_KEY: &str = "pos_bill_property_id_outlet_id_bill_no_key";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow, Serialize, utoipa::ToSchema)]
pub struct OutletRow {
    pub id: Uuid,
    pub property_id: Uuid,
    pub code: String,
    pub name: String,
    /// Whether the property's service charge applies to the outlet's bills.
    pub service_charge: bool,
    /// An inactive outlet takes no new bills; its past bills stay.
    pub active: bool,
    pub version: i32,
}

const OUTLET_COLUMNS: &str = "id, property_id, code, name, service_charge, active, version";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOutlet {
    pub code: String,
    pub name: String,
    pub service_charge: bool,
}

/// The fields to change; `None` keeps the current value. The code never changes: it names the outlet's category.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutletChanges {
    pub name: Option<String>,
    pub service_charge: Option<bool>,
    pub active: Option<bool>,
}

/// One item of a POS bill: `quantity × unit_net`, before taxes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PosItem {
    pub description: String,
    pub quantity: i32,
    pub unit_net: i64,
    /// `misc`, or the outlet's own `pos:<OUTLET>` (the default when `None`).
    pub category: Option<String>,
}

/// Where a bill goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PosTarget {
    /// To the folios of this in-house stay, routed as its other charges.
    Stay(Uuid),
    /// Paid at the outlet.
    Settled { method: Method },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PosBillIn {
    /// The outlet's code.
    pub outlet: String,
    pub bill_no: String,
    /// The property's business date when `None`.
    pub business_date: Option<Date>,
    pub currency: String,
    pub items: Vec<PosItem>,
    pub target: PosTarget,
}

/// One tax component of a settled bill's line, as `folio_line_tax` holds it for a posted line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PosLineTax {
    pub code: String,
    pub name: String,
    pub kind: String,
    pub rate_bp: i32,
    pub base: i64,
    pub amount: i64,
    pub presentation: String,
}

/// One item of a settled bill, taxed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PosLine {
    pub description: String,
    pub quantity: i32,
    pub unit_net: i64,
    pub category: String,
    pub net: i64,
    /// Net plus the on-bill taxes.
    pub total: i64,
    pub taxes: Vec<PosLineTax>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow, Serialize, utoipa::ToSchema)]
pub struct PosBillRow {
    pub id: Uuid,
    pub outlet_id: Uuid,
    pub bill_no: String,
    pub business_date: Date,
    /// The stay the bill was posted to; `None` for a bill settled at the outlet.
    pub reservation_room_id: Option<Uuid>,
    pub settled: bool,
    /// A settled bill's taxed items; empty for a bill posted to a stay, whose items are folio lines.
    #[sqlx(json)]
    pub lines: Vec<PosLine>,
    pub posted_at: OffsetDateTime,
}

const BILL_COLUMNS: &str = "id, outlet_id, bill_no, business_date, reservation_room_id, settled, lines, posted_at";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PosOutcome {
    /// Posted by this call.
    Posted(PosBillRow),
    /// Posted before with the same body; nothing was posted now.
    Replayed(PosBillRow),
}

/// Creates an outlet of `property` (`NotFound` if it is not in the tenant). The code is 1 to 10 upper-case letters
/// and digits and the name 1 to 100 characters (`Invalid`); a code the property already uses is a `Conflict`.
/// Audited as `outlet.created`.
pub async fn create_outlet(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    property: Uuid,
    input: NewOutlet,
) -> Result<OutletRow, FolioError> {
    if !valid_outlet_code(&input.code) {
        return Err(FolioError::Invalid("an outlet code is 1 to 10 upper-case letters and digits".into()));
    }
    let name = outlet_name(&input.name)?;
    business_date(tx, property).await?;
    let outlet: OutletRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "insert into outlet (id, tenant_id, property_id, code, name, service_charge)
         values ($1, $2, $3, $4, $5, $6)
         returning {OUTLET_COLUMNS}"
    )))
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(&input.code)
    .bind(name)
    .bind(input.service_charge)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| {
        if violates(&err, OUTLET_CODE_KEY) {
            FolioError::Conflict(format!("an outlet with code {} already exists", input.code))
        } else {
            err.into()
        }
    })?;
    let data = serde_json::json!({
        "code": outlet.code,
        "name": outlet.name,
        "service_charge": outlet.service_charge,
    });
    audit(tx, tenant, user, "outlet.created", "outlet", outlet.id, data).await?;
    Ok(outlet)
}

/// Changes the outlet `id` of `property` (`NotFound` otherwise) at `expected_version` (`VersionMismatch` otherwise).
/// A deactivated outlet keeps its past bills and takes no new ones. Audited as `outlet.updated`.
pub async fn update_outlet(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    changes: OutletChanges,
) -> Result<OutletRow, FolioError> {
    let name = changes.name.as_deref().map(outlet_name).transpose()?;
    let current: OutletRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {OUTLET_COLUMNS} from outlet where id = $1 and property_id = $2 for update"
    )))
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(FolioError::NotFound("outlet"))?;
    if current.version != expected_version {
        return Err(FolioError::VersionMismatch("outlet"));
    }
    let updated: OutletRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update outlet set name = coalesce($2, name), service_charge = coalesce($3, service_charge),
                active = coalesce($4, active), version = version + 1
         where id = $1
         returning {OUTLET_COLUMNS}"
    )))
    .bind(id)
    .bind(name)
    .bind(changes.service_charge)
    .bind(changes.active)
    .fetch_one(&mut **tx)
    .await?;
    let data = serde_json::json!({
        "name": updated.name,
        "service_charge": updated.service_charge,
        "active": updated.active,
    });
    audit(tx, tenant, user, "outlet.updated", "outlet", id, data).await?;
    Ok(updated)
}

/// Every outlet of the property, active or not, by code.
pub async fn list_outlets(tx: &mut Tx, property: Uuid) -> Result<Vec<OutletRow>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {OUTLET_COLUMNS} from outlet where property_id = $1 order by code"
    )))
    .bind(property)
    .fetch_all(&mut **tx)
    .await
}

/// The trimmed name, 1 to [`MAX_OUTLET_NAME`] characters (`Invalid`).
fn outlet_name(name: &str) -> Result<&str, FolioError> {
    let name = name.trim();
    if (1..=MAX_OUTLET_NAME).contains(&name.chars().count()) {
        Ok(name)
    } else {
        Err(FolioError::Invalid(format!("an outlet's name has 1 to {MAX_OUTLET_NAME} characters")))
    }
}

/// What [`post_pos_bill`] needs to know about the bill's outlet.
#[derive(sqlx::FromRow)]
struct Outlet {
    id: Uuid,
    code: String,
    name: String,
    service_charge: bool,
    active: bool,
}

/// A bill already posted under the number, with the hash of the body it was posted with.
#[derive(sqlx::FromRow)]
struct Posted {
    body_hash: Vec<u8>,
    #[sqlx(flatten)]
    bill: PosBillRow,
}

/// Posts the POS bill `bill` of one of `property`'s outlets, once: the outlet and bill number name it.
///
/// A bill already posted under that number with the same body (its SHA-256 over the canonical JSON of the outlet,
/// number, business date, currency, items and target) is `Replayed`, and nothing is posted; a different body is a
/// `Conflict`. Concurrent identical posts post one bill: the `pos_bill` row is inserted before anything is posted, so
/// the second waits on the first and then replays it.
///
/// A new bill needs 1 to 200 items and a bill number of 1 to 50 characters, an active outlet and a currency the
/// property accepts (each `Invalid`); an outlet or stay of another property is `NotFound`. An item's category is the
/// outlet's `pos:<OUTLET>` unless it names `misc`. The business date is the bill's, or the property's when it names
/// none. Items are taxed with the property's rules, without the service charge when the outlet's is off.
///
/// - **To a stay** ([`PosTarget::Stay`]), which must be checked in (`Invalid`, "Room is not in house"; its row is
///   held `for share` so it can't check out meanwhile): each item posts through the stay's routing as a `pos` line
///   "<OUTLET NAME>: <item>" (200 characters at most) under the posting key `pos:<bill>:<n>`, `n` counting items from
///   1. Each folio posted to queues `folio:<id>`.
/// - **Settled** ([`PosTarget::Settled`]): the taxed items are stored on the bill, and one payment of the bill's
///   total (more than 0) on no folio, at the rate of the business date (`Conflict` when none is in force). The
///   method is never `Account` (`Invalid`), and card details never hold a card number (`Invalid`).
///
/// Audited as `pos.bill_posted` with the outlet, bill number, total, currency and target kind. The caller checks
/// `PosPost`.
pub async fn post_pos_bill(
    tx: &mut Tx,
    tenant: TenantId,
    user: Option<UserId>,
    property: Uuid,
    bill: PosBillIn,
) -> Result<PosOutcome, FolioError> {
    if !(1..=MAX_ITEMS).contains(&bill.items.len()) {
        return Err(FolioError::Invalid(format!("a bill has 1 to {MAX_ITEMS} items")));
    }
    if bill.bill_no.trim().is_empty() || bill.bill_no.chars().count() > MAX_BILL_NO {
        return Err(FolioError::Invalid(format!("a bill number has 1 to {MAX_BILL_NO} characters")));
    }
    let outlet: Outlet = sqlx::query_as(
        "select id, code, name, service_charge, active from outlet where property_id = $1 and code = $2",
    )
    .bind(property)
    .bind(&bill.outlet)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(FolioError::NotFound("outlet"))?;
    let hash = body_hash(&bill)?;
    if let Some(posted) = posted_bill(tx, property, outlet.id, &bill.bill_no).await? {
        return replay(posted, &hash, &bill.bill_no);
    }

    if !outlet.active {
        return Err(FolioError::Invalid(format!("the outlet {} is not active", outlet.code)));
    }
    let (today, accepted, vat_registered): (Date, Vec<String>, bool) =
        sqlx::query_as("select business_date, accepted_currencies::text[], vat_registered from property where id = $1")
            .bind(property)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(FolioError::NotFound("property"))?;
    if !accepted.contains(&bill.currency) {
        return Err(FolioError::Invalid(format!("{} is not a currency this property accepts", bill.currency)));
    }
    let business_date = bill.business_date.unwrap_or(today);
    let id = Uuid::now_v7();
    let charges = bill
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| charge(&outlet, id, index + 1, item, &bill.currency, business_date))
        .collect::<Result<Vec<_>, _>>()?;

    let lines = match &bill.target {
        PosTarget::Stay(stay) => {
            let status: String =
                sqlx::query_scalar("select status from reservation_room where id = $1 and property_id = $2 for share")
                    .bind(stay)
                    .bind(property)
                    .fetch_optional(&mut **tx)
                    .await?
                    .ok_or(FolioError::NotFound("reservation room"))?;
            if status != "checked_in" {
                return Err(FolioError::Invalid("Room is not in house".into()));
            }
            Vec::new()
        }
        PosTarget::Settled { .. } => {
            let mut rules = crate::load_rules(tx, property).await?;
            if !outlet.service_charge {
                without_service_charge(&mut rules);
            }
            bill.items
                .iter()
                .zip(&charges)
                .map(|(item, charge)| settled_line(item, charge, &rules, vat_registered))
                .collect::<Result<Vec<_>, _>>()?
        }
    };

    let (stay, settled) = match &bill.target {
        PosTarget::Stay(stay) => (Some(*stay), false),
        PosTarget::Settled { .. } => (None, true),
    };
    let mut savepoint = tx.begin().await?;
    let inserted: Result<PosBillRow, sqlx::Error> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "insert into pos_bill (id, tenant_id, property_id, outlet_id, bill_no, business_date, body_hash,
                               reservation_room_id, settled, lines)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         returning {BILL_COLUMNS}"
    )))
    .bind(id)
    .bind(tenant.0)
    .bind(property)
    .bind(outlet.id)
    .bind(&bill.bill_no)
    .bind(business_date)
    .bind(&hash)
    .bind(stay)
    .bind(settled)
    .bind(sqlx::types::Json(&lines))
    .fetch_one(&mut *savepoint)
    .await;
    // A concurrent post of the same number committed first: its bill is the answer, as for any replay.
    let posted = match inserted {
        Ok(posted) => {
            savepoint.commit().await?;
            posted
        }
        Err(err) if violates(&err, BILL_NO_KEY) => {
            savepoint.rollback().await?;
            let posted = posted_bill(tx, property, outlet.id, &bill.bill_no)
                .await?
                .ok_or_else(|| FolioError::Conflict(format!("POS bill {} is being posted", bill.bill_no)))?;
            return replay(posted, &hash, &bill.bill_no);
        }
        Err(err) => return Err(err.into()),
    };

    let mut data = serde_json::json!({
        "outlet": outlet.code,
        "bill_no": posted.bill_no,
        "currency": bill.currency,
    });
    let total = match &bill.target {
        PosTarget::Stay(stay) => {
            let mut total = 0_i64;
            for charge in charges {
                let line = post_routed(tx, tenant, user, *stay, charge, !outlet.service_charge).await?;
                total = total.checked_add(line.total).ok_or_else(too_large)?;
            }
            data["target"] = "stay".into();
            data["reservation_room"] = serde_json::json!(stay);
            total
        }
        PosTarget::Settled { method } => {
            let total = lines.iter().try_fold(0_i64, |sum, line| sum.checked_add(line.total)).ok_or_else(too_large)?;
            let payment =
                pay_pos_bill(tx, tenant, user, property, posted.id, method, total, &bill.currency, business_date)
                    .await?;
            data["target"] = "settled".into();
            data["payment"] = serde_json::json!(payment.id);
            data["method"] = payment.method.into();
            total
        }
    };
    data["total"] = total.into();
    audit(tx, tenant, user, "pos.bill_posted", "pos_bill", posted.id, data).await?;
    Ok(PosOutcome::Posted(posted))
}

fn too_large() -> FolioError {
    FolioError::Invalid("the bill's total is too large".into())
}

/// The bill of `outlet` numbered `bill_no`, if one is posted.
async fn posted_bill(tx: &mut Tx, property: Uuid, outlet: Uuid, bill_no: &str) -> Result<Option<Posted>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select body_hash, {BILL_COLUMNS} from pos_bill where property_id = $1 and outlet_id = $2 and bill_no = $3"
    )))
    .bind(property)
    .bind(outlet)
    .bind(bill_no)
    .fetch_optional(&mut **tx)
    .await
}

/// `Replayed` when `posted` was posted with the body hashing to `hash`, else `Conflict`.
fn replay(posted: Posted, hash: &[u8], bill_no: &str) -> Result<PosOutcome, FolioError> {
    if posted.body_hash == hash {
        Ok(PosOutcome::Replayed(posted.bill))
    } else {
        Err(FolioError::Conflict(format!("POS bill {bill_no} was already posted with different contents")))
    }
}

/// Item `n` (from 1) of the bill `bill` as a charge on `business_date`, checked as any charge is.
fn charge(
    outlet: &Outlet,
    bill: Uuid,
    n: usize,
    item: &PosItem,
    currency: &str,
    business_date: Date,
) -> Result<NewCharge, FolioError> {
    let own = format!("pos:{}", outlet.code);
    let category = match item.category.as_deref() {
        None => own,
        Some("misc") => "misc".to_owned(),
        Some(category) if category == own => own,
        Some(other) => return Err(FolioError::Invalid(format!("an item's category is {own} or misc, not {other:?}"))),
    };
    let description = item.description.trim();
    if !(1..=MAX_DESCRIPTION).contains(&description.chars().count()) {
        return Err(FolioError::Invalid(format!("describe each item in 1 to {MAX_DESCRIPTION} characters")));
    }
    let charge = NewCharge {
        source: Source::Pos,
        category,
        description: format!("{}: {description}", outlet.name).chars().take(MAX_DESCRIPTION).collect(),
        quantity: item.quantity,
        unit_net: item.unit_net,
        currency: currency.to_owned(),
        business_date,
        posting_key: Some(format!("pos:{bill}:{n}")),
        inclusive: false,
    };
    check_charge(&charge)?;
    Ok(charge)
}

/// `item` of a settled bill, taxed with `rules` as `charge` would be on a folio of no stay.
fn settled_line(
    item: &PosItem,
    charge: &NewCharge,
    rules: &[tax::Rule],
    vat_registered: bool,
) -> Result<PosLine, FolioError> {
    let amount = i64::from(charge.quantity).checked_mul(charge.unit_net).ok_or_else(too_large)?;
    let context = tax::Context {
        date: charge.business_date,
        category: charge.category.clone(),
        vat_registered,
        resident: None,
        currency: charge.currency.clone(),
    };
    let breakdown = tax::calculate(amount, rules, &context)?;
    let taxes = breakdown
        .components
        .into_iter()
        .map(|component| {
            Ok(PosLineTax {
                rate_bp: i32::try_from(component.rate_bp)
                    .map_err(|_| FolioError::Conflict("a tax rate is out of range".into()))?,
                kind: kind_str(component.kind).to_owned(),
                presentation: presentation_str(component.presentation).to_owned(),
                code: component.code,
                name: component.name,
                base: component.base,
                amount: component.amount,
            })
        })
        .collect::<Result<Vec<_>, FolioError>>()?;
    Ok(PosLine {
        description: item.description.trim().to_owned(),
        quantity: charge.quantity,
        unit_net: charge.unit_net,
        category: charge.category.clone(),
        net: breakdown.net,
        total: breakdown.total,
        taxes,
    })
}

/// The bill as hashed. serde_json writes a struct's fields in declaration order, so this struct (never a map, whose
/// order could vary) fixes the canonical form: changing a field or its order changes every bill's hash.
#[derive(Serialize)]
struct Canonical<'a> {
    outlet: &'a str,
    bill_no: &'a str,
    /// `YYYY-MM-DD`.
    business_date: Option<String>,
    currency: &'a str,
    items: Vec<CanonicalItem<'a>>,
    target: CanonicalTarget<'a>,
}

#[derive(Serialize)]
struct CanonicalItem<'a> {
    description: &'a str,
    quantity: i32,
    unit_net: i64,
    category: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum CanonicalTarget<'a> {
    Stay(Uuid),
    Settled { method: CanonicalMethod<'a> },
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum CanonicalMethod<'a> {
    Cash,
    CardTerminal { approval: &'a str, last4: &'a str, brand: &'a str },
    BankTransfer { reference: &'a str },
    Account,
}

/// SHA-256 of the canonical JSON of `bill`, as sent.
fn body_hash(bill: &PosBillIn) -> Result<Vec<u8>, FolioError> {
    let target = match &bill.target {
        PosTarget::Stay(stay) => CanonicalTarget::Stay(*stay),
        PosTarget::Settled { method } => CanonicalTarget::Settled {
            method: match method {
                Method::Cash => CanonicalMethod::Cash,
                Method::CardTerminal { approval, last4, brand } => {
                    CanonicalMethod::CardTerminal { approval, last4, brand }
                }
                Method::BankTransfer { reference } => CanonicalMethod::BankTransfer { reference },
                Method::Account => CanonicalMethod::Account,
            },
        },
    };
    let canonical = Canonical {
        outlet: &bill.outlet,
        bill_no: &bill.bill_no,
        business_date: bill.business_date.map(|date| date.to_string()),
        currency: &bill.currency,
        items: bill
            .items
            .iter()
            .map(|item| CanonicalItem {
                description: &item.description,
                quantity: item.quantity,
                unit_net: item.unit_net,
                category: item.category.as_deref(),
            })
            .collect(),
        target,
    };
    let json = serde_json::to_vec(&canonical).map_err(|err| FolioError::Invalid(err.to_string()))?;
    Ok(Sha256::digest(&json).to_vec())
}
