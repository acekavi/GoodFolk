use crate::{ReservationsError, audit, decode_error};
use db::crypto::{GuestIdKey, Sealed, guest_aad, last4, mask_tail};
use db::{TenantId, Tx, UserId};
use rates::Residency;
use serde::Serialize;
use sqlx::Row;
use sqlx::postgres::PgRow;
use std::fmt;
use uuid::Uuid;

/// Most guests one search returns.
pub const MAX_GUEST_SEARCH: i64 = 50;

db::text_enum!(
    /// The kind of identity document a guest showed.
    IdDocType { Passport = "passport", Nic = "nic", DrivingLicence = "driving_licence", Other = "other" }
);

/// A guest as the API shows it. The ID number appears only masked (`•••• 1234`): the sealed value and its key
/// id never leave the database, and nothing here decrypts it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Guest {
    pub id: Uuid,
    /// Empty for a guest with a single name.
    pub first_name: String,
    pub last_name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    /// ISO 3166-1 alpha-2, such as `LK`.
    pub country: Option<String>,
    pub residency: Residency,
    pub id_doc_type: Option<IdDocType>,
    /// The ID number's last 4 characters behind a mask, such as `•••• 1234`.
    pub id_doc_masked: Option<String>,
    pub notes: String,
    pub version: i32,
}

/// `id_doc` holds the ID number in plain text until it is sealed; `Debug` hides it.
#[derive(Clone)]
pub struct NewGuest {
    pub first_name: String,
    pub last_name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub country: Option<String>,
    pub residency: Residency,
    pub notes: String,
    pub id_doc: Option<(IdDocType, String)>,
}

/// `None` leaves a field unchanged; `Some(None)` clears an optional one, so `id_doc: Some(None)` removes the
/// ID document and `Some(Some(..))` replaces it. `Debug` hides the ID number.
#[derive(Clone, Default)]
pub struct GuestChanges {
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub email: Option<Option<String>>,
    pub phone: Option<Option<String>>,
    pub country: Option<Option<String>>,
    pub residency: Option<Residency>,
    pub notes: Option<String>,
    pub id_doc: Option<Option<(IdDocType, String)>>,
}

/// The indexed expression guest names are searched on (`guest_name_trgm_idx`).
const NAME: &str = "lower(first_name || ' ' || last_name)";

/// Never the sealed number or its key id: only the last 4 characters, for the mask.
pub(crate) const COLUMNS: &str = "id, first_name, last_name, email, phone, country::text as country, residency, \
                       id_doc_type, id_doc_last4, notes, version";

impl sqlx::FromRow<'_, PgRow> for Guest {
    fn from_row(row: &PgRow) -> Result<Self, sqlx::Error> {
        let residency: String = row.try_get("residency")?;
        let id_doc_type: Option<String> = row.try_get("id_doc_type")?;
        let last4: Option<String> = row.try_get("id_doc_last4")?;
        Ok(Guest {
            id: row.try_get("id")?,
            first_name: row.try_get("first_name")?,
            last_name: row.try_get("last_name")?,
            email: row.try_get("email")?,
            phone: row.try_get("phone")?,
            country: row.try_get("country")?,
            residency: Residency::parse(&residency).ok_or_else(|| decode_error("residency", &residency))?,
            id_doc_type: id_doc_type
                .map(|value| IdDocType::parse(&value).ok_or_else(|| decode_error("id_doc_type", &value)))
                .transpose()?,
            id_doc_masked: last4.as_deref().map(mask_tail),
            notes: row.try_get("notes")?,
            version: row.try_get("version")?,
        })
    }
}

fn invalid(message: &str) -> ReservationsError {
    ReservationsError::Invalid(message.into())
}

fn first_name(value: &str) -> Result<String, ReservationsError> {
    let value = value.trim();
    if value.chars().count() <= 100 {
        Ok(value.to_owned())
    } else {
        Err(invalid("a first name is at most 100 characters"))
    }
}

fn last_name(value: &str) -> Result<String, ReservationsError> {
    let value = value.trim();
    if (1..=100).contains(&value.chars().count()) {
        Ok(value.to_owned())
    } else {
        Err(invalid("a last name is 1 to 100 characters"))
    }
}

/// One `@` between a local part and a dotted domain, no spaces, 3 to 254 characters.
fn looks_like_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    (3..=254).contains(&value.chars().count())
        && !value.chars().any(char::is_whitespace)
        && !local.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && domain.split('.').all(|label| !label.is_empty())
}

fn email(value: Option<String>) -> Result<Option<String>, ReservationsError> {
    value
        .map(|value| {
            let value = value.trim();
            if looks_like_email(value) {
                Ok(value.to_lowercase())
            } else {
                Err(invalid("an email looks like name@example.com"))
            }
        })
        .transpose()
}

fn phone(value: Option<String>) -> Result<Option<String>, ReservationsError> {
    value
        .map(|value| {
            let value = value.trim();
            if (3..=30).contains(&value.chars().count()) {
                Ok(value.to_owned())
            } else {
                Err(invalid("a phone number is 3 to 30 characters"))
            }
        })
        .transpose()
}

fn country(value: Option<String>) -> Result<Option<String>, ReservationsError> {
    value
        .map(|value| {
            let value = value.trim();
            if value.len() == 2 && value.bytes().all(|b| b.is_ascii_uppercase()) {
                Ok(value.to_owned())
            } else {
                Err(invalid("a country is a two-letter code such as LK"))
            }
        })
        .transpose()
}

pub(crate) fn notes(value: String) -> Result<String, ReservationsError> {
    if value.chars().count() <= 2000 { Ok(value) } else { Err(invalid("notes are at most 2000 characters")) }
}

/// An ID document ready to store: its number sealed to the guest, and the last 4 characters for the mask.
struct IdDoc {
    kind: IdDocType,
    sealed: Sealed,
    last4: String,
}

impl IdDoc {
    fn seal(
        key: &GuestIdKey,
        tenant: TenantId,
        guest: Uuid,
        (kind, number): (IdDocType, String),
    ) -> Result<IdDoc, ReservationsError> {
        let number = number.trim();
        if !(1..=50).contains(&number.chars().count()) {
            return Err(invalid("an ID number is 1 to 50 characters"));
        }
        Ok(IdDoc { kind, sealed: key.seal(number, &guest_aad(tenant.0, guest)), last4: last4(number) })
    }

    /// How the audit log names the document: never the number.
    fn audit(doc: Option<&IdDoc>) -> serde_json::Value {
        doc.map_or(serde_json::Value::Null, |doc| serde_json::json!({ "type": doc.kind.as_str(), "last4": doc.last4 }))
    }
}

pub async fn create_guest(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    key: &GuestIdKey,
    input: NewGuest,
) -> Result<Guest, ReservationsError> {
    let id = Uuid::now_v7();
    let (first_name, last_name) = (first_name(&input.first_name)?, last_name(&input.last_name)?);
    let (email, phone, country) = (email(input.email)?, phone(input.phone)?, country(input.country)?);
    let notes = notes(input.notes)?;
    let id_doc = input.id_doc.map(|doc| IdDoc::seal(key, tenant, id, doc)).transpose()?;
    let created: Guest = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "insert into guest (id, tenant_id, first_name, last_name, email, phone, country, residency, notes,
                            id_doc_type, id_doc_number_enc, id_doc_key_id, id_doc_last4)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
         returning {COLUMNS}"
    )))
    .bind(id)
    .bind(tenant.0)
    .bind(first_name)
    .bind(last_name)
    .bind(email)
    .bind(phone)
    .bind(country)
    .bind(input.residency.as_str())
    .bind(notes)
    .bind(id_doc.as_ref().map(|doc| doc.kind.as_str()))
    .bind(id_doc.as_ref().map(|doc| &doc.sealed.bytes))
    .bind(id_doc.as_ref().map(|doc| &doc.sealed.key_id))
    .bind(id_doc.as_ref().map(|doc| &doc.last4))
    .fetch_one(&mut **tx)
    .await?;
    audit(
        tx,
        tenant,
        actor,
        "guest.created",
        "guest",
        id,
        serde_json::json!({ "id_doc": IdDoc::audit(id_doc.as_ref()) }),
    )
    .await?;
    Ok(created)
}

pub async fn update_guest(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    key: &GuestIdKey,
    id: Uuid,
    expected_version: i32,
    changes: GuestChanges,
) -> Result<Guest, ReservationsError> {
    let row = sqlx::query("select tenant_id from guest where id = $1 for update")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ReservationsError::NotFound("guest"))?;
    let row_tenant: Uuid = row.try_get("tenant_id")?;
    let current: Guest = sqlx::query_as(sqlx::AssertSqlSafe(format!("select {COLUMNS} from guest where id = $1")))
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
    if current.version != expected_version {
        return Err(ReservationsError::VersionMismatch("guest"));
    }
    let fields: Vec<&str> = [
        ("first_name", changes.first_name.is_some()),
        ("last_name", changes.last_name.is_some()),
        ("email", changes.email.is_some()),
        ("phone", changes.phone.is_some()),
        ("country", changes.country.is_some()),
        ("residency", changes.residency.is_some()),
        ("notes", changes.notes.is_some()),
        ("id_doc", changes.id_doc.is_some()),
    ]
    .into_iter()
    .filter_map(|(field, changed)| changed.then_some(field))
    .collect();
    let first_name = changes.first_name.as_deref().map(first_name).transpose()?.unwrap_or(current.first_name);
    let last_name = changes.last_name.as_deref().map(last_name).transpose()?.unwrap_or(current.last_name);
    let email = changes.email.map(email).transpose()?.unwrap_or(current.email);
    let phone = changes.phone.map(phone).transpose()?.unwrap_or(current.phone);
    let country = changes.country.map(country).transpose()?.unwrap_or(current.country);
    let notes = changes.notes.map(notes).transpose()?.unwrap_or(current.notes);
    let residency = changes.residency.unwrap_or(current.residency);
    // `None`: keep the stored document; `Some(None)`: remove it; `Some(Some(..))`: replace it.
    // Seal with the row's tenant_id, not the argument, so a mismatched argument cannot make a number unopenable.
    let id_doc = changes
        .id_doc
        .map(|doc| doc.map(|doc| IdDoc::seal(key, TenantId(row_tenant), id, doc)).transpose())
        .transpose()?;
    let replaced = id_doc.as_ref().and_then(Option::as_ref);
    let updated: Guest = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update guest set first_name = $2, last_name = $3, email = $4, phone = $5, country = $6, residency = $7,
                notes = $8,
                id_doc_type = case when $9 then id_doc_type else $10 end,
                id_doc_number_enc = case when $9 then id_doc_number_enc else $11 end,
                id_doc_key_id = case when $9 then id_doc_key_id else $12 end,
                id_doc_last4 = case when $9 then id_doc_last4 else $13 end,
                version = version + 1
         where id = $1
         returning {COLUMNS}"
    )))
    .bind(id)
    .bind(first_name)
    .bind(last_name)
    .bind(email)
    .bind(phone)
    .bind(country)
    .bind(residency.as_str())
    .bind(notes)
    .bind(id_doc.is_none())
    .bind(replaced.map(|doc| doc.kind.as_str()))
    .bind(replaced.map(|doc| &doc.sealed.bytes))
    .bind(replaced.map(|doc| &doc.sealed.key_id))
    .bind(replaced.map(|doc| &doc.last4))
    .fetch_one(&mut **tx)
    .await?;
    let mut data = serde_json::json!({ "fields": fields });
    if id_doc.is_some() {
        data["id_doc"] = IdDoc::audit(replaced);
    }
    audit(tx, tenant, actor, "guest.updated", "guest", id, data).await?;
    Ok(updated)
}

pub async fn get_guest(tx: &mut Tx, id: Uuid) -> Result<Guest, ReservationsError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("select {COLUMNS} from guest where id = $1")))
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ReservationsError::NotFound("guest"))
}

/// Up to `limit` guests (at most [`MAX_GUEST_SEARCH`]) whose name is like `text`, typos included, or whose
/// email or phone is exactly `text`: exact matches first, then by how closely the name matches. Blank `text`
/// lists the newest guests.
pub async fn search_guests(tx: &mut Tx, text: &str, limit: i64) -> Result<Vec<Guest>, sqlx::Error> {
    let (text, limit) = (text.trim(), limit.clamp(1, MAX_GUEST_SEARCH));
    if text.is_empty() {
        return sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {COLUMNS} from guest order by created_at desc, id desc limit $1"
        )))
        .bind(limit)
        .fetch_all(&mut **tx)
        .await;
    }
    // `<%` (word similarity) matches a part of the name, such as a last name or its first letters, which `%`
    // (whole-string similarity) misses. Under forced row-level security the trigram operator is not leakproof,
    // so this query scans the tenant's guests rather than using the trigram index alone; see the plan's
    // Decision 13.
    let text_lower = text.to_lowercase();
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {COLUMNS} from guest
         where lower($1) <% {NAME} or email = $2 or phone = $1
         order by (email = $2 or phone = $1) is true desc, word_similarity(lower($1), {NAME}) desc,
                  similarity(lower($1), {NAME}) desc, id
         limit $3"
    )))
    .bind(text)
    .bind(text_lower)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await
}

impl fmt::Debug for NewGuest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewGuest")
            .field("first_name", &self.first_name)
            .field("last_name", &self.last_name)
            .field("email", &self.email)
            .field("phone", &self.phone)
            .field("country", &self.country)
            .field("residency", &self.residency)
            .field("notes", &self.notes)
            .field("id_doc", &self.id_doc.as_ref().map(|(kind, _)| kind))
            .finish()
    }
}

impl fmt::Debug for GuestChanges {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GuestChanges")
            .field("first_name", &self.first_name)
            .field("last_name", &self.last_name)
            .field("email", &self.email)
            .field("phone", &self.phone)
            .field("country", &self.country)
            .field("residency", &self.residency)
            .field("notes", &self.notes)
            .field("id_doc", &self.id_doc.as_ref().map(|doc| doc.as_ref().map(|(kind, _)| kind)))
            .finish()
    }
}
