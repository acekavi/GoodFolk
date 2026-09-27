use crate::auth::TenantContext;
use crate::concurrency::{IfMatch, Versioned};
use crate::error::{ApiError, Changes, validate, validate_changes};
use crate::extract::{ApiJson, ApiPath, SensitiveJson};
use crate::routes::rooms::present;
use crate::state::AppState;
use axum::extract::State;
use db::{Scope, Tx};
use garde::Validate;
use identity::Permission;
use rates::{MealPlan, Residency};
use reservations::{
    AssignedRoom, CancelledRoom, CreatedReservation, Guest, GuestChanges, IdDocType, MAX_ROOMS_PER_RESERVATION,
    NewGuest, NewReservation, NewReservationRoom, ReservationsError, Source,
};
use serde::Deserialize;
use std::fmt;
use time::Date;
use utoipa::ToSchema;
use uuid::Uuid;

/// Maps the reservations module's errors to problem details.
fn reservations_error(err: ReservationsError) -> ApiError {
    match err {
        ReservationsError::NotFound(_) => ApiError::not_found(err.to_string()),
        ReservationsError::VersionMismatch(_) => ApiError::precondition_failed(err.to_string()),
        ReservationsError::Conflict(message) => ApiError::conflict(message),
        ReservationsError::Invalid(message) => ApiError::unprocessable(message),
        ReservationsError::Database(db_err) => db_err.into(),
    }
}

/// Guests belong to the tenant, but are reached through one of its properties so the grant check is per
/// property: a property of another tenant is 404, like every other resource there.
async fn require_property(tx: &mut Tx, property: Uuid) -> Result<(), ApiError> {
    if property::list_properties(tx, Some(&[property])).await?.is_empty() {
        return Err(ApiError::not_found("property not found"));
    }
    Ok(())
}

/// An identity document. `Debug` hides the number, which is sealed on arrival and never shown again.
#[derive(Deserialize, Validate, ToSchema)]
pub struct IdDocRequest {
    #[serde(rename = "type")]
    #[garde(skip)]
    pub doc_type: IdDocType,
    /// 1 to 50 characters. Responses show only its last 4 characters behind a mask, such as `•••• 1234`.
    #[garde(length(chars, min = 1, max = 50))]
    pub number: String,
}

impl fmt::Debug for IdDocRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdDocRequest").field("doc_type", &self.doc_type).field("number", &"<redacted>").finish()
    }
}

impl From<IdDocRequest> for (IdDocType, String) {
    fn from(doc: IdDocRequest) -> Self {
        (doc.doc_type, doc.number)
    }
}

/// A guest of the tenant, usable by all its properties.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateGuestRequest {
    /// Left out or empty for a guest with a single name.
    #[serde(default)]
    #[garde(length(chars, max = 100))]
    pub first_name: String,
    #[garde(length(chars, min = 1, max = 100))]
    pub last_name: String,
    #[garde(inner(length(chars, min = 3, max = 254)))]
    pub email: Option<String>,
    #[garde(inner(length(chars, min = 3, max = 30)))]
    pub phone: Option<String>,
    /// ISO 3166-1 alpha-2, such as `LK`.
    #[garde(inner(pattern(r"^[A-Z]{2}$")))]
    pub country: Option<String>,
    /// Prices the guest's stays: some plans sell only to residents or only to non-residents.
    #[garde(skip)]
    pub residency: Residency,
    #[serde(default)]
    #[garde(length(chars, max = 2000))]
    pub notes: String,
    #[garde(dive)]
    pub id_doc: Option<IdDocRequest>,
}

/// Fields left out stay as they are; `email`, `phone`, `country` and `id_doc` sent as `null` are cleared.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateGuestRequest {
    #[garde(inner(length(chars, max = 100)))]
    pub first_name: Option<String>,
    #[garde(inner(length(chars, min = 1, max = 100)))]
    pub last_name: Option<String>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable)]
    #[garde(inner(inner(length(chars, min = 3, max = 254))))]
    pub email: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable)]
    #[garde(inner(inner(length(chars, min = 3, max = 30))))]
    pub phone: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable)]
    #[garde(inner(inner(pattern(r"^[A-Z]{2}$"))))]
    pub country: Option<Option<String>>,
    #[garde(skip)]
    pub residency: Option<Residency>,
    #[garde(inner(length(chars, max = 2000)))]
    pub notes: Option<String>,
    /// Replaces the identity document; `null` removes it.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<IdDocRequest>, nullable)]
    #[garde(dive)]
    pub id_doc: Option<Option<IdDocRequest>>,
}

impl Changes for UpdateGuestRequest {
    fn is_empty(&self) -> bool {
        self.first_name.is_none()
            && self.last_name.is_none()
            && self.email.is_none()
            && self.phone.is_none()
            && self.country.is_none()
            && self.residency.is_none()
            && self.notes.is_none()
            && self.id_doc.is_none()
    }
}

/// One room of a booking: a room type on a rate plan and meal plan for `[check_in, check_out)`.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ReservationRoomRequest {
    #[garde(skip)]
    pub room_type_id: Uuid,
    #[garde(skip)]
    pub rate_plan_id: Uuid,
    #[garde(skip)]
    pub meal_plan: MealPlan,
    /// On or after the business date.
    #[garde(skip)]
    pub check_in: Date,
    /// The morning the guest leaves, at most 730 days after the business date.
    #[garde(skip)]
    pub check_out: Date,
    #[garde(range(min = 1, max = 50))]
    pub adults: i32,
    #[serde(default)]
    #[garde(range(min = 0, max = 50))]
    pub children: i32,
    /// Who stays in the room; left out, the booker. The room is priced for this guest's residency.
    #[garde(skip)]
    pub primary_guest_id: Option<Uuid>,
}

/// Books every room, confirmed, or none: a night with no room left is 409, and a stay its plan does not sell
/// (a restriction, a missing price) is 422 with every reason.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateReservationRequest {
    #[garde(skip)]
    pub booker_guest_id: Uuid,
    /// `front_desk`, `phone` or `email`.
    #[garde(skip)]
    pub source: Source,
    #[serde(default)]
    #[garde(length(chars, max = 2000))]
    pub notes: String,
    #[garde(length(min = 1, max = MAX_ROOMS_PER_RESERVATION), dive)]
    pub rooms: Vec<ReservationRoomRequest>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct AssignRoomRequest {
    /// An active room of the booked type, free and unblocked on the stay's nights.
    #[garde(skip)]
    pub room_id: Uuid,
}

#[utoipa::path(post, operation_id = "create_guest", path = "/api/v1/properties/{property}/guests", request_body = CreateGuestRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = Guest,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create_guest(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    SensitiveJson(body): SensitiveJson<CreateGuestRequest>,
) -> Result<Versioned<Guest>, ApiError> {
    ctx.require(Permission::ReservationsManage, Some(property))?;
    validate(&body)?;
    let input = NewGuest {
        first_name: body.first_name,
        last_name: body.last_name,
        email: body.email,
        phone: body.phone,
        country: body.country,
        residency: body.residency,
        notes: body.notes,
        id_doc: body.id_doc.map(Into::into),
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    require_property(&mut tx, property).await?;
    let created = reservations::create_guest(&mut tx, ctx.tenant, ctx.user, &state.guest_id_key, input)
        .await
        .map_err(reservations_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

#[utoipa::path(patch, operation_id = "update_guest", path = "/api/v1/properties/{property}/guests/{guest}", request_body = UpdateGuestRequest,
    params(("property" = Uuid, Path), ("guest" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = Guest,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
pub async fn update_guest(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, guest)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    SensitiveJson(body): SensitiveJson<UpdateGuestRequest>,
) -> Result<Versioned<Guest>, ApiError> {
    ctx.require(Permission::ReservationsManage, Some(property))?;
    validate_changes(&body)?;
    let changes = GuestChanges {
        first_name: body.first_name,
        last_name: body.last_name,
        email: body.email,
        phone: body.phone,
        country: body.country,
        residency: body.residency,
        notes: body.notes,
        id_doc: body.id_doc.map(|doc| doc.map(Into::into)),
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    require_property(&mut tx, property).await?;
    let updated =
        reservations::update_guest(&mut tx, ctx.tenant, ctx.user, &state.guest_id_key, guest, version, changes)
            .await
            .map_err(reservations_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}

#[utoipa::path(post, operation_id = "create_reservation", path = "/api/v1/properties/{property}/reservations", request_body = CreateReservationRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = CreatedReservation,
        headers(("ETag" = String, description = "the reservation's version, e.g. \"1\""))), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create_reservation(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateReservationRequest>,
) -> Result<Versioned<CreatedReservation>, ApiError> {
    ctx.require(Permission::ReservationsManage, Some(property))?;
    validate(&body)?;
    let input = NewReservation {
        booker_guest_id: body.booker_guest_id,
        source: body.source,
        notes: body.notes,
        rooms: body
            .rooms
            .into_iter()
            .map(|room| NewReservationRoom {
                room_type_id: room.room_type_id,
                rate_plan_id: room.rate_plan_id,
                meal_plan: room.meal_plan,
                check_in: room.check_in,
                check_out: room.check_out,
                adults: room.adults,
                children: room.children,
                primary_guest_id: room.primary_guest_id,
            })
            .collect(),
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created = reservations::create_reservation(&mut tx, ctx.tenant, ctx.user, property, input)
        .await
        .map_err(reservations_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

/// Cancels a tentative or confirmed room, releases its nights from the business date on, and records what
/// its booked cancellation terms charge today.
#[utoipa::path(post, operation_id = "cancel_reservation_room", path = "/api/v1/properties/{property}/reservation-rooms/{room}/cancel",
    params(("property" = Uuid, Path), ("room" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = CancelledRoom,
        headers(("ETag" = String, description = "the reservation room's version, e.g. \"2\""))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 428)))]
pub async fn cancel_room(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, room)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
) -> Result<Versioned<CancelledRoom>, ApiError> {
    ctx.require(Permission::ReservationsManage, Some(property))?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let cancelled = reservations::cancel_room(&mut tx, ctx.tenant, ctx.user, property, room, version)
        .await
        .map_err(reservations_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(cancelled.version, cancelled))
}

/// Puts a confirmed stay in a room, or moves it to another. A room taken on any of the nights is 409, naming
/// the reservation that has it.
#[utoipa::path(post, operation_id = "assign_reservation_room", path = "/api/v1/properties/{property}/reservation-rooms/{room}/assign", request_body = AssignRoomRequest,
    params(("property" = Uuid, Path), ("room" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = AssignedRoom,
        headers(("ETag" = String, description = "the reservation room's version, e.g. \"2\""))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
pub async fn assign_room(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, room)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<AssignRoomRequest>,
) -> Result<Versioned<AssignedRoom>, ApiError> {
    ctx.require(Permission::ReservationsManage, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let assigned = reservations::assign_room(&mut tx, ctx.tenant, ctx.user, property, room, version, body.room_id)
        .await
        .map_err(reservations_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(assigned.version, assigned))
}

/// Takes a confirmed stay out of its room; it stays booked.
#[utoipa::path(post, operation_id = "unassign_reservation_room", path = "/api/v1/properties/{property}/reservation-rooms/{room}/unassign",
    params(("property" = Uuid, Path), ("room" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = AssignedRoom,
        headers(("ETag" = String, description = "the reservation room's version, e.g. \"3\""))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 428)))]
pub async fn unassign_room(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, room)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
) -> Result<Versioned<AssignedRoom>, ApiError> {
    ctx.require(Permission::ReservationsManage, Some(property))?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let unassigned = reservations::unassign_room(&mut tx, ctx.tenant, ctx.user, property, room, version)
        .await
        .map_err(reservations_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(unassigned.version, unassigned))
}
