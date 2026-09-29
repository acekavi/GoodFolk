//! Companies and travel agents a reservation can be billed to (invoicing and the city ledger stay in Phase
//! 7). Tenant-wide like [`crate::guests`], not scoped to a property: every function takes a transaction
//! already scoped to the caller's tenant by row-level security, and the queries here name no `tenant_id`
//! column of their own.

use crate::guests::{email, phone};
use crate::{ReservationsError, audit};
use db::{TenantId, Tx, UserId};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::postgres::PgRow;
use sqlx::types::Json;
use uuid::Uuid;

/// Most accounts one list returns.
pub const MAX_ACCOUNT_LIST: i64 = 200;

db::text_enum!(
    /// Who a reservation may be billed to.
    AccountKind { Company = "company", TravelAgent = "travel_agent" }
);

/// How to reach an account: every field optional, filled in as known.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AccountContact {
    pub email: Option<String>,
    pub phone: Option<String>,
    pub address: Option<String>,
    pub contact_name: Option<String>,
}

/// A company or travel agent a reservation can be billed to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Account {
    pub id: Uuid,
    pub kind: AccountKind,
    pub name: String,
    pub contact: AccountContact,
    /// In minor units of `currency`; `None` for no limit.
    pub credit_limit: Option<i64>,
    pub currency: String,
    pub active: bool,
    pub version: i32,
}

#[derive(Debug, Clone)]
pub struct NewAccount {
    pub kind: AccountKind,
    pub name: String,
    pub contact: AccountContact,
    pub credit_limit: Option<i64>,
    pub currency: String,
}

/// `None` leaves a field unchanged; for the nullable fields (as [`crate::GuestChanges`]), `Some(None)` clears
/// it and `Some(Some(..))` replaces it.
#[derive(Debug, Clone, Default)]
pub struct AccountChanges {
    pub kind: Option<AccountKind>,
    pub name: Option<String>,
    pub email: Option<Option<String>>,
    pub phone: Option<Option<String>>,
    pub address: Option<Option<String>>,
    pub contact_name: Option<Option<String>>,
    pub credit_limit: Option<Option<i64>>,
    pub currency: Option<String>,
    pub active: Option<bool>,
}

const COLUMNS: &str = "id, kind, name, contact, credit_limit, currency, active, version";

impl sqlx::FromRow<'_, PgRow> for Account {
    fn from_row(row: &PgRow) -> Result<Self, sqlx::Error> {
        let kind: String = row.try_get("kind")?;
        let contact: Json<AccountContact> = row.try_get("contact")?;
        Ok(Account {
            id: row.try_get("id")?,
            kind: AccountKind::parse(&kind).ok_or_else(|| crate::decode_error("kind", &kind))?,
            name: row.try_get("name")?,
            contact: contact.0,
            credit_limit: row.try_get("credit_limit")?,
            currency: row.try_get("currency")?,
            active: row.try_get("active")?,
            version: row.try_get("version")?,
        })
    }
}

fn invalid(message: impl Into<String>) -> ReservationsError {
    ReservationsError::Invalid(message.into())
}

fn name(value: &str) -> Result<String, ReservationsError> {
    let value = value.trim();
    if (1..=200).contains(&value.chars().count()) {
        Ok(value.to_owned())
    } else {
        Err(invalid("a name is 1 to 200 characters"))
    }
}

fn address(value: Option<String>) -> Result<Option<String>, ReservationsError> {
    value
        .map(|value| {
            let value = value.trim();
            if value.chars().count() <= 500 {
                Ok(value.to_owned())
            } else {
                Err(invalid("an address is at most 500 characters"))
            }
        })
        .transpose()
}

fn contact_name(value: Option<String>) -> Result<Option<String>, ReservationsError> {
    value
        .map(|value| {
            let value = value.trim();
            if value.chars().count() <= 200 {
                Ok(value.to_owned())
            } else {
                Err(invalid("a contact name is at most 200 characters"))
            }
        })
        .transpose()
}

fn credit_limit(value: Option<i64>) -> Result<Option<i64>, ReservationsError> {
    value
        .map(|value| {
            if (0..=100_000_000_000).contains(&value) {
                Ok(value)
            } else {
                Err(invalid("a credit limit is 0 to 100,000,000,000"))
            }
        })
        .transpose()
}

/// Three uppercase letters, such as `USD`.
fn currency(value: &str) -> Result<String, ReservationsError> {
    let value = value.trim();
    if value.len() == 3 && value.bytes().all(|b| b.is_ascii_uppercase()) {
        Ok(value.to_owned())
    } else {
        Err(invalid("a currency is a three-letter uppercase code such as USD"))
    }
}

fn validated_contact(contact: AccountContact) -> Result<AccountContact, ReservationsError> {
    Ok(AccountContact {
        email: email(contact.email)?,
        phone: phone(contact.phone)?,
        address: address(contact.address)?,
        contact_name: contact_name(contact.contact_name)?,
    })
}

pub async fn create_account(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    input: NewAccount,
) -> Result<Account, ReservationsError> {
    let id = Uuid::now_v7();
    let account_name = name(&input.name)?;
    let contact = validated_contact(input.contact)?;
    let credit_limit = credit_limit(input.credit_limit)?;
    let currency = currency(&input.currency)?;
    let created: Account = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "insert into account (id, tenant_id, kind, name, contact, credit_limit, currency)
         values ($1, $2, $3, $4, $5, $6, $7)
         returning {COLUMNS}"
    )))
    .bind(id)
    .bind(tenant.0)
    .bind(input.kind.as_str())
    .bind(account_name)
    .bind(Json(contact))
    .bind(credit_limit)
    .bind(currency)
    .fetch_one(&mut **tx)
    .await?;
    let data = serde_json::json!({ "kind": created.kind.as_str(), "name": created.name });
    audit(tx, tenant, actor, "account.created", "account", id, data).await?;
    Ok(created)
}

pub async fn update_account(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    id: Uuid,
    expected_version: i32,
    changes: AccountChanges,
) -> Result<Account, ReservationsError> {
    sqlx::query("select 1 from account where id = $1 for update")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ReservationsError::NotFound("account"))?;
    let current: Account = sqlx::query_as(sqlx::AssertSqlSafe(format!("select {COLUMNS} from account where id = $1")))
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
    if current.version != expected_version {
        return Err(ReservationsError::VersionMismatch("account"));
    }
    let fields: Vec<&str> = [
        ("kind", changes.kind.is_some()),
        ("name", changes.name.is_some()),
        ("email", changes.email.is_some()),
        ("phone", changes.phone.is_some()),
        ("address", changes.address.is_some()),
        ("contact_name", changes.contact_name.is_some()),
        ("credit_limit", changes.credit_limit.is_some()),
        ("currency", changes.currency.is_some()),
        ("active", changes.active.is_some()),
    ]
    .into_iter()
    .filter_map(|(field, changed)| changed.then_some(field))
    .collect();
    let kind = changes.kind.unwrap_or(current.kind);
    let name = changes.name.as_deref().map(name).transpose()?.unwrap_or(current.name);
    let email = changes.email.map(email).transpose()?.unwrap_or(current.contact.email);
    let phone = changes.phone.map(phone).transpose()?.unwrap_or(current.contact.phone);
    let address = changes.address.map(address).transpose()?.unwrap_or(current.contact.address);
    let contact_name = changes.contact_name.map(contact_name).transpose()?.unwrap_or(current.contact.contact_name);
    let credit_limit = changes.credit_limit.map(credit_limit).transpose()?.unwrap_or(current.credit_limit);
    let currency = changes.currency.as_deref().map(currency).transpose()?.unwrap_or(current.currency);
    let active = changes.active.unwrap_or(current.active);
    let contact = AccountContact { email, phone, address, contact_name };
    let updated: Account = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update account set kind = $2, name = $3, contact = $4, credit_limit = $5, currency = $6, active = $7,
                version = version + 1
         where id = $1
         returning {COLUMNS}"
    )))
    .bind(id)
    .bind(kind.as_str())
    .bind(name)
    .bind(Json(contact))
    .bind(credit_limit)
    .bind(currency)
    .bind(active)
    .fetch_one(&mut **tx)
    .await?;
    let data = serde_json::json!({ "fields": fields });
    audit(tx, tenant, actor, "account.updated", "account", id, data).await?;
    Ok(updated)
}

pub async fn get_account(tx: &mut Tx, id: Uuid) -> Result<Account, ReservationsError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("select {COLUMNS} from account where id = $1")))
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ReservationsError::NotFound("account"))
}

/// Up to `limit` accounts (at most [`MAX_ACCOUNT_LIST`]) whose name contains `search` (case-insensitive),
/// ordered by name then id. Active accounts only unless `include_inactive`. A blank `search` lists every
/// matching account.
///
/// Filters `lower(name)`, which is not leakproof, so under forced row-level security only the `tenant_id`
/// half of `account_tenant_name_idx` becomes an index condition; the rest is a plain filter over the tenant's
/// own accounts (see the index's comment in `migrations/0008_reservations_3b.sql`). Accounts are few per
/// tenant, so that scan is fine.
pub async fn list_accounts(
    tx: &mut Tx,
    search: &str,
    include_inactive: bool,
    limit: i64,
) -> Result<Vec<Account>, sqlx::Error> {
    let search = search.trim().to_lowercase();
    let limit = limit.clamp(1, MAX_ACCOUNT_LIST);
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {COLUMNS} from account
         where (active or $1) and ($2 = '' or position($2 in lower(name)) > 0)
         order by name, id
         limit $3"
    )))
    .bind(include_inactive)
    .bind(search)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await
}

/// `Invalid` ("no such account") if `account` does not name an account of this tenant (row-level security
/// already scopes what this can see); `Invalid` ("`<name>` is no longer active") if it does but is inactive.
/// Used both by `reservations::create_reservation` and `reservations::update_reservation`.
pub(crate) async fn check_account(tx: &mut Tx, account: Uuid) -> Result<(), ReservationsError> {
    let row: Option<(String, bool)> = sqlx::query_as("select name, active from account where id = $1")
        .bind(account)
        .fetch_optional(&mut **tx)
        .await?;
    match row {
        None => Err(invalid("no such account")),
        Some((_, true)) => Ok(()),
        Some((account_name, false)) => Err(invalid(format!("{account_name} is no longer active"))),
    }
}
