//! Folios and reading them: a guest folio per stay, a deposit folio per reservation and an account folio per account
//! and property, each with its lines, their taxes, its payments and its balances.

use crate::FolioError;
use crate::lines::{LINE_COLUMNS, LineRow};
use db::{TenantId, Tx};
use rust_decimal::Decimal;
use serde::{Serialize, Serializer};
use std::collections::{BTreeMap, HashMap};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

db::text_enum!(
    /// Whose charges a folio holds: one stay's, an account's (the city ledger) or a reservation's deposits.
    FolioKind { Guest = "guest", Account = "account", Deposit = "deposit" }
);

db::text_enum!(
    /// An invoiced folio takes no lines until a credit note reopens it.
    FolioStatus { Open = "open", Invoiced = "invoiced" }
);

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow, Serialize, utoipa::ToSchema)]
pub struct FolioRow {
    pub id: Uuid,
    pub property_id: Uuid,
    #[sqlx(try_from = "String")]
    pub kind: FolioKind,
    pub reservation_room_id: Option<Uuid>,
    pub account_id: Option<Uuid>,
    pub reservation_id: Option<Uuid>,
    #[sqlx(try_from = "String")]
    pub status: FolioStatus,
    pub created_at: OffsetDateTime,
    pub version: i32,
}

pub(crate) const FOLIO_COLUMNS: &str =
    "id, property_id, kind, reservation_room_id, account_id, reservation_id, status, created_at, version";

/// One tax, levy or service charge of a line, as calculated when it was posted.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow, Serialize, utoipa::ToSchema)]
pub struct LineTax {
    #[serde(skip)]
    pub line_id: Uuid,
    pub code: String,
    pub name: String,
    pub kind: String,
    pub rate_bp: i32,
    pub base: i64,
    pub amount: i64,
    pub presentation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct LineWithTaxes {
    #[serde(flatten)]
    pub line: LineRow,
    pub taxes: Vec<LineTax>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow, Serialize, utoipa::ToSchema)]
pub struct PaymentRow {
    pub id: Uuid,
    pub folio_id: Option<Uuid>,
    pub method: String,
    pub amount: i64,
    pub currency: String,
    pub base_amount: i64,
    #[serde(serialize_with = "decimal_string")]
    #[schema(value_type = String)]
    pub rate: Decimal,
    pub rate_date: Date,
    #[schema(value_type = Object)]
    pub details: serde_json::Value,
    pub evidence: Option<String>,
    pub refund_of: Option<Uuid>,
    pub business_date: Date,
    pub transfer_id: Option<Uuid>,
    pub taken_by: Option<Uuid>,
    pub taken_at: OffsetDateTime,
    pub pos_bill_id: Option<Uuid>,
}

pub(crate) const PAYMENT_COLUMNS: &str = "id, folio_id, method, amount, currency::text as currency, base_amount, rate, \
     rate_date, details, evidence, refund_of, business_date, transfer_id, taken_by, taken_at, pos_bill_id";

/// What a folio holds in one currency: charges less payments, and the same in the base currency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Balance {
    pub currency: String,
    pub amount: i64,
    pub base_amount: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct FolioView {
    pub folio: FolioRow,
    pub lines: Vec<LineWithTaxes>,
    pub payments: Vec<PaymentRow>,
    /// One per currency the folio has lines or payments in, by currency code.
    pub balances: Vec<Balance>,
}

/// Which folios of a property to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolioQuery {
    /// The reservation's deposit folio, each of its stays' guest folios, and its account's folio, in that order.
    Reservation { property: Uuid, reservation: Uuid },
    /// One stay's guest folio.
    ReservationRoom { property: Uuid, reservation_room: Uuid },
    /// An account's folio in the property.
    Account { property: Uuid, account: Uuid },
    /// One folio.
    Folio { property: Uuid, folio: Uuid },
}

pub(crate) fn decimal_string<S: Serializer>(value: &Decimal, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_str(value)
}

/// A folio row locked by [`lock_folios`].
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct Locked {
    pub id: Uuid,
    pub property_id: Uuid,
    #[sqlx(try_from = "String")]
    pub kind: FolioKind,
    pub reservation_room_id: Option<Uuid>,
    pub account_id: Option<Uuid>,
    pub reservation_id: Option<Uuid>,
    #[sqlx(try_from = "String")]
    pub status: FolioStatus,
}

/// Locks the folios `ids` (`for update`, in ascending id order, as every folio command does) and returns those that
/// exist in `property`, in id order.
pub(crate) async fn lock_folios(tx: &mut Tx, property: Uuid, ids: &[Uuid]) -> Result<Vec<Locked>, sqlx::Error> {
    sqlx::query_as(
        "select id, property_id, kind, reservation_room_id, account_id, reservation_id, status
         from folio where id = any($1) and property_id = $2 order by id for update",
    )
    .bind(ids)
    .bind(property)
    .fetch_all(&mut **tx)
    .await
}

/// The guest folio of the stay `reservation_room`, created if it has none. `NotFound` if the stay is not in the
/// property.
pub async fn ensure_guest_folio(
    tx: &mut Tx,
    tenant: TenantId,
    property: Uuid,
    reservation_room: Uuid,
) -> Result<Uuid, FolioError> {
    ensure(tx, tenant, property, FolioKind::Guest, reservation_room).await
}

/// The deposit folio of `reservation`, created if it has none. `NotFound` if the reservation is not in the property.
pub async fn ensure_deposit_folio(
    tx: &mut Tx,
    tenant: TenantId,
    property: Uuid,
    reservation: Uuid,
) -> Result<Uuid, FolioError> {
    ensure(tx, tenant, property, FolioKind::Deposit, reservation).await
}

/// The folio of `account` in the property, created on first use. `NotFound` if the account is not in this tenant.
pub async fn ensure_account_folio(
    tx: &mut Tx,
    tenant: TenantId,
    property: Uuid,
    account: Uuid,
) -> Result<Uuid, FolioError> {
    ensure(tx, tenant, property, FolioKind::Account, account).await
}

/// The folio of `kind` for `owner`, created if missing. The owner is checked first, so a bad id is `NotFound`
/// rather than a foreign-key error that would abort the caller's transaction. A concurrent create of the same folio
/// waits on the unique index and then finds the other's row.
async fn ensure(
    tx: &mut Tx,
    tenant: TenantId,
    property: Uuid,
    kind: FolioKind,
    owner: Uuid,
) -> Result<Uuid, FolioError> {
    let (column, find, owner_exists, what) = match kind {
        FolioKind::Guest => (
            "reservation_room_id",
            "select id from folio where kind = 'guest' and reservation_room_id = $1 and property_id = $2",
            "select exists (select 1 from reservation_room where id = $1 and property_id = $2)",
            "reservation room",
        ),
        FolioKind::Deposit => (
            "reservation_id",
            "select id from folio where kind = 'deposit' and reservation_id = $1 and property_id = $2",
            "select exists (select 1 from reservation where id = $1 and property_id = $2)",
            "reservation",
        ),
        FolioKind::Account => (
            "account_id",
            "select id from folio where kind = 'account' and account_id = $1 and property_id = $2",
            "select exists (select 1 from account where id = $1)
                    and exists (select 1 from property where id = $2)",
            "account",
        ),
    };
    let found: Option<Uuid> = sqlx::query_scalar(find).bind(owner).bind(property).fetch_optional(&mut **tx).await?;
    if let Some(id) = found {
        return Ok(id);
    }
    let exists: bool = sqlx::query_scalar(owner_exists).bind(owner).bind(property).fetch_one(&mut **tx).await?;
    if !exists {
        return Err(FolioError::NotFound(what));
    }
    let conflict = match kind {
        FolioKind::Guest => "(reservation_room_id) where kind = 'guest'",
        FolioKind::Deposit => "(reservation_id) where kind = 'deposit'",
        FolioKind::Account => "(property_id, account_id) where kind = 'account'",
    };
    let inserted: Option<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "insert into folio (id, tenant_id, property_id, kind, {column}) values ($1, $2, $3, $4, $5)
         on conflict {conflict} do nothing returning id"
    )))
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(kind.as_str())
    .bind(owner)
    .fetch_optional(&mut **tx)
    .await?;
    match inserted {
        Some(id) => Ok(id),
        None => Ok(sqlx::query_scalar(find).bind(owner).bind(property).fetch_one(&mut **tx).await?),
    }
}

/// The folios `query` names, each with its lines (and their taxes), payments and balances. Four statements whatever
/// the number of folios: the folios, then every line, every tax and every payment of all of them at once.
pub async fn folios_for(tx: &mut Tx, query: FolioQuery) -> Result<Vec<FolioView>, FolioError> {
    let (filter, property, target) = match query {
        FolioQuery::Reservation { property, reservation } => (
            "f.id in (
               select d.id from folio d where d.kind = 'deposit' and d.reservation_id = $2
               union all
               select g.id from folio g join reservation_room rr on rr.id = g.reservation_room_id
               where g.kind = 'guest' and rr.reservation_id = $2 and rr.property_id = $1
               union all
               select a.id from folio a join reservation r on r.account_id = a.account_id
               where a.kind = 'account' and a.property_id = $1 and r.id = $2 and r.property_id = $1)",
            property,
            reservation,
        ),
        FolioQuery::ReservationRoom { property, reservation_room } => {
            ("f.kind = 'guest' and f.reservation_room_id = $2", property, reservation_room)
        }
        FolioQuery::Account { property, account } => ("f.kind = 'account' and f.account_id = $2", property, account),
        FolioQuery::Folio { property, folio } => ("f.id = $2", property, folio),
    };
    let folios: Vec<FolioRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {FOLIO_COLUMNS} from folio f where f.property_id = $1 and {filter}
         order by case f.kind when 'deposit' then 0 when 'guest' then 1 else 2 end, f.created_at, f.id"
    )))
    .bind(property)
    .bind(target)
    .fetch_all(&mut **tx)
    .await?;
    if folios.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<Uuid> = folios.iter().map(|folio| folio.id).collect();

    let lines: Vec<LineRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {LINE_COLUMNS} from folio_line where folio_id = any($1) order by posted_at, id"
    )))
    .bind(&ids)
    .fetch_all(&mut **tx)
    .await?;
    let taxes: Vec<LineTax> = sqlx::query_as(
        "select t.line_id, t.code, t.name, t.kind, t.rate_bp, t.base, t.amount, t.presentation
         from folio_line_tax t join folio_line l on l.id = t.line_id
         where l.folio_id = any($1)
         order by t.line_id, t.code",
    )
    .bind(&ids)
    .fetch_all(&mut **tx)
    .await?;
    let payments: Vec<PaymentRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {PAYMENT_COLUMNS} from payment where folio_id = any($1) order by taken_at, id"
    )))
    .bind(&ids)
    .fetch_all(&mut **tx)
    .await?;

    let mut taxes_by_line: HashMap<Uuid, Vec<LineTax>> = HashMap::new();
    for tax in taxes {
        taxes_by_line.entry(tax.line_id).or_default().push(tax);
    }
    let mut lines_by_folio: HashMap<Uuid, Vec<LineWithTaxes>> = HashMap::new();
    for line in lines {
        let taxes = taxes_by_line.remove(&line.id).unwrap_or_default();
        lines_by_folio.entry(line.folio_id).or_default().push(LineWithTaxes { line, taxes });
    }
    let mut payments_by_folio: HashMap<Uuid, Vec<PaymentRow>> = HashMap::new();
    for payment in payments {
        if let Some(folio) = payment.folio_id {
            payments_by_folio.entry(folio).or_default().push(payment);
        }
    }

    folios
        .into_iter()
        .map(|folio| {
            let lines = lines_by_folio.remove(&folio.id).unwrap_or_default();
            let payments = payments_by_folio.remove(&folio.id).unwrap_or_default();
            let balances = balances(&lines, &payments)?;
            Ok(FolioView { folio, lines, payments, balances })
        })
        .collect()
}

/// Charges less payments per currency, by currency code.
fn balances(lines: &[LineWithTaxes], payments: &[PaymentRow]) -> Result<Vec<Balance>, FolioError> {
    let mut sums: BTreeMap<&str, (i128, i128)> = BTreeMap::new();
    for line in lines {
        let sum = sums.entry(line.line.currency.as_str()).or_default();
        sum.0 += i128::from(line.line.total);
        sum.1 += i128::from(line.line.base_total);
    }
    for payment in payments {
        let sum = sums.entry(payment.currency.as_str()).or_default();
        sum.0 -= i128::from(payment.amount);
        sum.1 -= i128::from(payment.base_amount);
    }
    let fit = |value: i128| i64::try_from(value).map_err(|_| FolioError::Invalid("the balance is too large".into()));
    sums.into_iter()
        .map(|(currency, (amount, base))| {
            Ok(Balance { currency: currency.to_owned(), amount: fit(amount)?, base_amount: fit(base)? })
        })
        .collect()
}
