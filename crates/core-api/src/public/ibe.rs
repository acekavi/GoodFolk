//! The booking site's public routes: resolving a custom host, the site and the search.

use crate::error::ApiError;
use crate::extract::{ApiPath, ApiQuery};
use crate::public::ClientIp;
use crate::public::limits::{self, SEARCHES_PER_MINUTE};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;
use db::{Scope, TenantId};
use ibe::{IbeError, RoomAsk, SearchRequest, SearchResult, SiteView};
use rates::Residency;
use serde::{Deserialize, Serialize};
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
        IbeError::Database(db_err) => db_err.into(),
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
    use super::parse_rooms;
    use ibe::RoomAsk;

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
}
