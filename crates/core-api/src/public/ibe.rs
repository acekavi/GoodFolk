//! The booking site's public routes: resolving a custom host, the site, the search and booking.

use crate::error::ApiError;
use crate::extract::{ApiPath, ApiQuery};
use crate::idempotency;
use crate::public::limits::{self, BOOKINGS_PER_MINUTE, SEARCHES_PER_MINUTE};
use crate::public::{ClientIp, turnstile};
use crate::state::AppState;
use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use db::{Scope, TenantId};
use ibe::{BookingError, BookingRequest, IbeError, RoomAsk, SearchRequest, SearchResult, SiteView};
use rates::Residency;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use time::Date;
use utoipa::ToSchema;
use uuid::Uuid;

/// The edge keeps the site this long.
const SITE_CACHE: &str = "public, s-maxage=300";
/// The edge keeps a search, per full query string, this long.
const SEARCH_CACHE: &str = "public, s-maxage=30";

/// The code a custom host's site is addressed by.
#[derive(Debug, Serialize, ToSchema)]
pub struct SiteCode {
    pub code: String,
}

/// A booking from the site's form.
#[derive(Deserialize, ToSchema)]
pub struct BookingBody {
    /// The Turnstile widget's token.
    #[serde(default)]
    pub turnstile_token: String,
    /// A field people never see, so never fill: a form that fills it is a bot's.
    #[serde(default)]
    pub website: String,
    #[serde(flatten)]
    pub booking: BookingRequest,
}

/// The Turnstile token stays out of logs; the booking redacts its guests' details itself.
impl std::fmt::Debug for BookingBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BookingBody")
            .field("turnstile_token", &"<redacted>")
            .field("website", &self.website)
            .field("booking", &self.booking)
            .finish()
    }
}

/// A booking made.
#[derive(Debug, Serialize, ToSchema)]
pub struct BookingConfirmed {
    pub confirmation_no: String,
    /// The manage page's link, with the booking's manage token: in the first answer only. The token is never stored
    /// for a replay, so a retry with the same `Idempotency-Key` answers without it; the link is in the confirmation
    /// email.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manage_url: Option<String>,
}

/// What a booking that cannot be made as sent answers: the new totals for a price that changed.
#[derive(Debug, Serialize, ToSchema)]
pub struct BookingConflict {
    /// `price_changed` or `sold_out`.
    pub reason: String,
    /// The totals now, per currency in minor units, when the price changed.
    pub totals: Option<BTreeMap<String, i64>>,
}

/// A stay to search.
#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub check_in: Date,
    pub check_out: Date,
    pub residency: Residency,
    /// Each room's adults and children, `adults-children`, comma-separated: `2-0,2-1`.
    pub rooms: String,
}

pub(crate) fn ibe_error(err: IbeError) -> ApiError {
    match err {
        IbeError::NotFound(_) => ApiError::not_found(err.to_string()),
        IbeError::Invalid(message) => ApiError::unprocessable(message),
        IbeError::Tax(error) => {
            tracing::error!(%error, "a booking site price could not be taxed");
            ApiError::internal()
        }
        IbeError::Media(error) => {
            tracing::error!(%error, "a booking site image could not be read");
            ApiError::internal()
        }
        IbeError::Database(db_err) => db_err.into(),
    }
}

/// What the guest is told when a room chosen is no longer offered.
const SOLD_OUT: &str = "That room was just booked; here is what is still available";

fn booking_error(err: BookingError) -> ApiError {
    match err {
        BookingError::Invalid(message) => ApiError::unprocessable(message),
        BookingError::NotAgreed => ApiError::unprocessable(err.to_string()),
        BookingError::PriceChanged { totals } => {
            ApiError::conflict("The price has changed; confirm the new total to book")
                .with("reason", "price_changed")
                .with("totals", totals)
        }
        BookingError::SoldOut => ApiError::conflict(SOLD_OUT).with("reason", "sold_out"),
        BookingError::Ibe(err) => ibe_error(err),
    }
}

/// The tenant and property of the enabled site named by `code` (any case) or, when given, `host`. `404` otherwise.
async fn resolve(state: &AppState, code: Option<&str>, host: Option<&str>) -> Result<(TenantId, Uuid), ApiError> {
    db::ibe_resolve(&state.pool, code, host).await?.ok_or_else(|| ApiError::not_found("No booking site here"))
}

/// The code of the enabled site at the custom host `host`, for the booking site's server to address it by. `404` when
/// no enabled site has that host.
#[utoipa::path(get, operation_id = "ibe_by_host", path = "/api/public/v1/ibe/by-host/{host}",
    params(("host" = String, Path, description = "a custom host, e.g. `book.galleforthotel.lk`")),
    responses((status = 200, body = SiteCode), (status = 404)))]
pub async fn by_host(
    State(state): State<AppState>,
    ApiPath(host): ApiPath<String>,
) -> Result<impl IntoResponse, ApiError> {
    let (tenant, property) = resolve(&state, None, Some(&host)).await?;
    let mut tx = db::begin(&state.pool, Scope::tenant(tenant)).await?;
    let code = ibe::public_code(&mut tx, property).await.map_err(ibe_error)?;
    tx.commit().await?;
    Ok(([(header::CACHE_CONTROL, SITE_CACHE)], Json(SiteCode { code })))
}

/// The booking site of the property coded `code`: branding, contact, terms, and its active room types with their
/// photos and tour videos. `404` unless its site is enabled. Cached at the edge for 5 minutes.
#[utoipa::path(get, operation_id = "ibe_site", path = "/api/public/v1/ibe/{code}/site",
    params(("code" = String, Path, description = "the property code, any case")),
    responses((status = 200, body = SiteView, headers(("Cache-Control" = String))), (status = 404)))]
pub async fn site(
    State(state): State<AppState>,
    ApiPath(code): ApiPath<String>,
) -> Result<impl IntoResponse, ApiError> {
    let (tenant, property) = resolve(&state, Some(&code), None).await?;
    let mut tx = db::begin(&state.pool, Scope::tenant(tenant)).await?;
    let view = ibe::site(&mut tx, property, |key| state.storage.public_url(key).into()).await.map_err(ibe_error)?;
    tx.commit().await?;
    Ok(([(header::CACHE_CONTROL, SITE_CACHE)], Json(view)))
}

/// What the site offers for a stay: each room type free for every room asked that it can hold, with every `IBE` plan
/// open for the stay, priced in its own currency with the taxes on the bill. Without any, `next_available` is the
/// nearest later check-in within 14 days for the same stay. 1 to 30 nights from the business date up to 18 months
/// ahead, 1 to 4 rooms (`422`). 60 searches a minute per IP (`429` with `Retry-After`). Cached at the edge for 30 s.
#[utoipa::path(get, operation_id = "ibe_search", path = "/api/public/v1/ibe/{code}/search",
    params(("code" = String, Path, description = "the property code, any case"),
        ("check_in" = Date, Query), ("check_out" = Date, Query),
        ("residency" = Residency, Query, description = "the guest's answer to \"Are you a resident of Sri Lanka?\""),
        ("rooms" = String, Query, description = "each room's adults and children, `adults-children`, comma-separated: `2-0,2-1`")),
    responses((status = 200, body = SearchResult, headers(("Cache-Control" = String))), (status = 400), (status = 404),
        (status = 422), (status = 429, headers(("Retry-After" = String)))))]
pub async fn search(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiPath(code): ApiPath<String>,
    ApiQuery(query): ApiQuery<SearchQuery>,
) -> Result<impl IntoResponse, ApiError> {
    limits::check(&state.pool, "search", ip, SEARCHES_PER_MINUTE).await?;
    let (tenant, property) = resolve(&state, Some(&code), None).await?;
    let request = SearchRequest {
        check_in: query.check_in,
        check_out: query.check_out,
        residency: query.residency,
        rooms: parse_rooms(&query.rooms)?,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(tenant)).await?;
    let result = ibe::search(&mut tx, property, &request).await.map_err(ibe_error)?;
    tx.commit().await?;
    Ok(([(header::CACHE_CONTROL, SEARCH_CACHE)], Json(result)))
}

/// Books rooms on the site of the property coded `code`, paid at the hotel: a confirmed reservation from the booking
/// site, and its confirmation email with the manage link. Prices are always worked out again: a total per currency
/// other than `expected_totals` is `409` with `reason: price_changed` and the new `totals`, which the guest confirms;
/// a room no longer free is `409` with `reason: sold_out`. A booking the site does not sell is `422`.
///
/// Checked in this order: 5 bookings a minute per IP (`429` with `Retry-After`); the `Idempotency-Key` (required);
/// the hidden `website` field, which a bot fills (it is answered as if booked, the same way for the same key, and
/// nothing is kept); the key's stored answer (scoped to the property: the same request again gets the first answer,
/// without asking Turnstile again, but with no `manage_url`, since the manage token is never stored; another request
/// with the same key is `409`); then, for a new key only, the Turnstile token (`400` when missing or refused, `503`
/// when it cannot be checked; neither keeps anything under the key).
///
/// Send a new `Idempotency-Key` with each new attempt, and treat any `4xx` as the end of its key: a changed price is
/// confirmed with a new key, since its body differs.
#[utoipa::path(post, operation_id = "ibe_book", path = "/api/public/v1/ibe/{code}/bookings",
    params(("code" = String, Path, description = "the property code, any case"),
        ("Idempotency-Key" = String, Header, description = "8 to 160 characters, new for each booking attempt")),
    request_body = BookingBody,
    responses((status = 201, body = BookingConfirmed), (status = 400), (status = 404),
        (status = 409, body = BookingConflict), (status = 422), (status = 429, headers(("Retry-After" = String))),
        (status = 503)))]
pub async fn book(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiPath(code): ApiPath<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    limits::check(&state.pool, "booking", ip, BOOKINGS_PER_MINUTE).await?;
    let (tenant, property) = resolve(&state, Some(&code), None).await?;
    // Fixed messages: serde's would quote the guest's details back.
    let request: BookingBody = serde_json::from_slice(&body).map_err(|err| match err.classify() {
        serde_json::error::Category::Data => ApiError::unprocessable("the booking does not have the expected shape"),
        _ => ApiError::bad_request("the booking is not valid JSON"),
    })?;
    let key = idempotency::public_key(&headers)?;
    if !request.website.is_empty() {
        return Ok(decoy(&state, property, &code, &key));
    }
    // Turnstile spends a token once, so a retry carrying the same one is answered from the store before this runs.
    let booking = async {
        if let Err(refused) = turnstile::verify(&state.ibe, &request.turnstile_token, ip).await {
            // A bot check that did not pass booked nothing: the key stays free for a retry with a new token.
            let mut response = refused.into_response();
            response.extensions_mut().insert(idempotency::ReleaseClaim);
            return response;
        }
        let booked = async {
            let mut tx = db::begin(&state.pool, Scope::tenant(tenant)).await?;
            let booked = ibe::book(&mut tx, tenant, property, &request.booking).await.map_err(booking_error)?;
            let code = ibe::public_code(&mut tx, property).await.map_err(ibe_error)?;
            tx.commit().await?;
            Ok::<_, ApiError>((manage_url(&state, &code, &booked.token), booked.confirmation_no))
        };
        match booked.await {
            Ok((manage_url, confirmation_no)) => {
                let kept = BookingConfirmed { confirmation_no: confirmation_no.clone(), manage_url: None };
                let kept = serde_json::to_vec(&kept).expect("a confirmation serializes");
                let confirmed = BookingConfirmed { confirmation_no, manage_url: Some(manage_url) };
                let mut response = (StatusCode::CREATED, Json(confirmed)).into_response();
                response.extensions_mut().insert(idempotency::ReplayBody(kept.into()));
                response
            }
            Err(err) => err.into_response(),
        }
    };
    idempotency::property_scoped(&state, tenant, property, &key, (&method, &uri, &body), booking).await
}

/// The manage page of the booking whose token is `token`, on the booking domain.
fn manage_url(state: &AppState, code: &str, token: &str) -> String {
    format!("{}/{code}/manage?token={token}", state.ibe.public_origin)
}

/// What a bot that filled the hidden field is told: a booking's answer in every way, for a booking that was never
/// made. Its confirmation number and manage token are derived from the key with the server's Turnstile secret, so the
/// same key gets the same answer again with nothing stored.
fn decoy(state: &AppState, property: Uuid, code: &str, key: &str) -> Response {
    let mut hasher = Sha256::new();
    for part in [state.ibe.turnstile_secret.as_bytes(), property.as_bytes(), key.as_bytes()] {
        hasher.update(part);
        hasher.update([0]);
    }
    let seed = hasher.finalize();
    let number = u64::from_be_bytes(Sha256::digest(seed)[..8].try_into().expect("8 bytes")) % 1_000_000;
    let code = code.to_ascii_uppercase();
    let confirmed = BookingConfirmed {
        confirmation_no: format!("{code}-{number:06}"),
        manage_url: Some(manage_url(state, &code, &URL_SAFE_NO_PAD.encode(seed))),
    };
    (StatusCode::CREATED, Json(confirmed)).into_response()
}

/// `2-0,2-1` as rooms; `400` unless every room is `adults-children` in digits and the list is short. The search
/// itself refuses more rooms than a booking may have, with its own message.
fn parse_rooms(rooms: &str) -> Result<Vec<RoomAsk>, ApiError> {
    let malformed = || ApiError::bad_request("rooms is each room's adults and children, e.g. 2-0,2-1");
    if rooms.len() > 64 {
        return Err(malformed());
    }
    rooms
        .split(',')
        .map(|room| {
            let (adults, children) = room.split_once('-').ok_or_else(malformed)?;
            let count = |text: &str| text.parse::<u8>().map(i32::from).map_err(|_| malformed());
            Ok(RoomAsk { adults: count(adults)?, children: count(children)? })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{ibe_error, parse_rooms};
    use axum::http::StatusCode;
    use ibe::{IbeError, RoomAsk};

    #[test]
    fn rooms_are_adults_and_children_pairs() {
        assert_eq!(
            parse_rooms("2-0,4-1").unwrap(),
            [RoomAsk { adults: 2, children: 0 }, RoomAsk { adults: 4, children: 1 }]
        );
        for bad in ["", "2", "2-", "-1", "2-0,", "a-b", "2-0;1-0", "1000-0"] {
            assert!(parse_rooms(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_booking_body_debug_hides_the_turnstile_token_and_the_guests_details() {
        let body: super::BookingBody = serde_json::from_value(serde_json::json!({
            "turnstile_token": "s3cr3t-turnstile",
            "residency": "resident",
            "booker": {"first_name": "Kamala", "last_name": "Perera", "email": "kamala@example.com",
                       "phone": "+94 77 123 4567", "country": "LK"},
            "rooms": [{"room_type_id": uuid::Uuid::nil(), "rate_plan_id": uuid::Uuid::nil(), "meal_plan": "RO",
                       "adults": 2, "children": 0, "occupant_name": "Nimal Silva"}],
            "check_in": "2026-11-01",
            "check_out": "2026-11-03",
            "expected_totals": {"LKR": 20000},
            "agreed": true,
        }))
        .unwrap();

        let debug = format!("{body:?}");
        for secret in ["s3cr3t-turnstile", "Kamala", "Perera", "kamala@example.com", "123 4567", "Nimal"] {
            assert!(!debug.contains(secret), "{secret} in {debug}");
        }
    }

    #[test]
    fn an_unreadable_image_is_a_server_error_and_a_missing_one_not_found() {
        assert_eq!(ibe_error(IbeError::Media("bad renditions".into())).status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(ibe_error(media::MediaError::NotFound("asset").into()).status(), StatusCode::NOT_FOUND);
    }
}
