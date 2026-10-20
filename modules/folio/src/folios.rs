//! Folios and reading them: a guest folio per stay, a deposit folio per reservation and an account folio per account
//! and property, each with its lines, their taxes, its payments and its balances.

use crate::FolioError;
use crate::lines::{LINE_COLUMNS, LineRow};
use db::{TenantId, Tx};
use rust_decimal::Decimal;
use serde::{Serialize, Serializer};
use std::collections::HashMap;
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
    /// Oldest first: the newest [`MAX_FOLIO_ROWS`] when `truncated`, none when `lines_omitted`.
    pub lines: Vec<LineWithTaxes>,
    /// Oldest first: the newest [`MAX_FOLIO_ROWS`] when `truncated`, none when `lines_omitted`.
    pub payments: Vec<PaymentRow>,
    /// One per currency the folio has lines or payments in, by currency code, over all of them.
    pub balances: Vec<Balance>,
    /// The folio's lines and payments were left out: an account folio read through a reservation.
    pub lines_omitted: bool,
    /// The folio has more lines or payments than were read.
    pub truncated: bool,
}

/// SQLSTATE `numeric_value_out_of_range`: a balance summed past a bigint.
const NUMERIC_OUT_OF_RANGE: &str = "22003";

/// The most lines, and the most payments, [`folios_for`] reads of one folio: the newest.
pub const MAX_FOLIO_ROWS: i64 = 500;

/// Which folios of a property to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolioQuery {
    /// The reservation's deposit folio, each of its stays' guest folios, and its account's current folio (the open
    /// one, else the newest), in that order.
    Reservation { property: Uuid, reservation: Uuid },
    /// One stay's guest folio.
    ReservationRoom { property: Uuid, reservation_room: Uuid },
    /// Every folio of an account in the property: the open one first, then the invoiced ones, newest first.
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

/// The open folio of `account` in the property, created when it has none: on first use, and again after its open
/// folio is invoiced, so the account's next charges start a fresh ledger. `NotFound` if the account is not in this
/// tenant.
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
            "select id from folio where kind = 'account' and account_id = $1 and property_id = $2 and status = 'open'",
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
        FolioKind::Account => "(property_id, account_id) where kind = 'account' and status = 'open'",
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

/// The folios `query` names, each with its lines (and their taxes), payments and balances. Five statements whatever
/// the number of folios: the folios, then the lines, taxes, payments and balances of all of them at once.
///
/// Bounded for reading: each folio gives its newest [`MAX_FOLIO_ROWS`] lines and newest [`MAX_FOLIO_ROWS`] payments,
/// with `truncated` set when it has more. A [`FolioQuery::Reservation`] reads its account's current folio (the
/// account's whole city ledger) with its balances but no lines or payments (`lines_omitted`): read it by account for
/// those. Balances always cover every line and payment.
pub async fn folios_for(tx: &mut Tx, query: FolioQuery) -> Result<Vec<FolioView>, FolioError> {
    read(tx, query, Some(MAX_FOLIO_ROWS)).await
}

/// One folio of the property with every line and payment, unbounded: what an invoice is issued from.
pub(crate) async fn complete_folio(tx: &mut Tx, property: Uuid, folio: Uuid) -> Result<FolioView, FolioError> {
    read(tx, FolioQuery::Folio { property, folio }, None).await?.pop().ok_or(FolioError::NotFound("folio"))
}

async fn read(tx: &mut Tx, query: FolioQuery, cap: Option<i64>) -> Result<Vec<FolioView>, FolioError> {
    let (filter, property, target) = match query {
        FolioQuery::Reservation { property, reservation } => (
            "f.id in (
               select d.id from folio d where d.kind = 'deposit' and d.reservation_id = $2
               union all
               select g.id from folio g join reservation_room rr on rr.id = g.reservation_room_id
               where g.kind = 'guest' and rr.reservation_id = $2 and rr.property_id = $1
               union all
               (select a.id from folio a join reservation r on r.account_id = a.account_id
                where a.kind = 'account' and a.property_id = $1 and r.id = $2 and r.property_id = $1
                order by a.status = 'open' desc, a.created_at desc, a.id desc
                limit 1))",
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
         order by case f.kind when 'deposit' then 0 when 'guest' then 1 else 2 end,
                  f.status <> 'open',
                  case when f.kind = 'account' then f.created_at end desc,
                  case when f.kind = 'account' then f.id end desc,
                  f.created_at, f.id"
    )))
    .bind(property)
    .bind(target)
    .fetch_all(&mut **tx)
    .await?;
    if folios.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<Uuid> = folios.iter().map(|folio| folio.id).collect();
    let omitted =
        |folio: &FolioRow| matches!(query, FolioQuery::Reservation { .. }) && folio.kind == FolioKind::Account;
    let detailed: Vec<Uuid> = folios.iter().filter(|folio| !omitted(folio)).map(|folio| folio.id).collect();
    // One more than the cap, newest first, so a folio holding more is seen to be truncated. `limit null` is no limit.
    let limit = cap.map(|cap| cap + 1);

    let lines: Vec<LineRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select l.* from unnest($1::uuid[]) as f(id)
         cross join lateral (select {LINE_COLUMNS} from folio_line where folio_id = f.id
                             order by posted_at desc, id desc limit $2) as l
         order by l.folio_id, l.posted_at desc, l.id desc"
    )))
    .bind(&detailed)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;
    let line_ids: Vec<Uuid> = lines.iter().map(|line| line.id).collect();
    let taxes: Vec<LineTax> = sqlx::query_as(
        "select line_id, code, name, kind, rate_bp, base, amount, presentation
         from folio_line_tax where line_id = any($1)
         order by line_id, code",
    )
    .bind(&line_ids)
    .fetch_all(&mut **tx)
    .await?;
    let payments: Vec<PaymentRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select p.* from unnest($1::uuid[]) as f(id)
         cross join lateral (select {PAYMENT_COLUMNS} from payment where folio_id = f.id
                             order by id desc limit $2) as p
         order by p.folio_id, p.id desc"
    )))
    .bind(&detailed)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;
    let sums: Vec<(Uuid, String, i64, i64)> = sqlx::query_as(
        "select folio_id, currency, sum(amount)::bigint, sum(base)::bigint
         from (select folio_id, currency::text as currency, total as amount, base_total as base
               from folio_line where folio_id = any($1)
               union all
               select folio_id, currency::text, -amount, -base_amount from payment where folio_id = any($1)) as entries
         group by folio_id, currency
         order by folio_id, currency",
    )
    .bind(&ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| {
        // A sum past the range of a bigint: no folio holds that much money, but say so rather than fail.
        if err.as_database_error().and_then(|db_err| db_err.code()).as_deref() == Some(NUMERIC_OUT_OF_RANGE) {
            FolioError::Invalid("the balance is too large".into())
        } else {
            err.into()
        }
    })?;

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
    let mut balances_by_folio: HashMap<Uuid, Vec<Balance>> = HashMap::new();
    for (folio, currency, amount, base_amount) in sums {
        balances_by_folio.entry(folio).or_default().push(Balance { currency, amount, base_amount });
    }

    Ok(folios
        .into_iter()
        .map(|folio| {
            let lines_omitted = omitted(&folio);
            let (lines, lines_cut) = newest_first(lines_by_folio.remove(&folio.id).unwrap_or_default(), cap);
            let (payments, payments_cut) = newest_first(payments_by_folio.remove(&folio.id).unwrap_or_default(), cap);
            let balances = balances_by_folio.remove(&folio.id).unwrap_or_default();
            FolioView { folio, lines, payments, balances, lines_omitted, truncated: lines_cut || payments_cut }
        })
        .collect())
}

/// `rows`, read newest first and up to one past `cap`, oldest first and within `cap`, and whether any were cut.
fn newest_first<T>(mut rows: Vec<T>, cap: Option<i64>) -> (Vec<T>, bool) {
    let cap = cap.and_then(|cap| usize::try_from(cap).ok());
    let cut = cap.is_some_and(|cap| rows.len() > cap);
    if let Some(cap) = cap {
        rows.truncate(cap);
    }
    rows.reverse();
    (rows, cut)
}
