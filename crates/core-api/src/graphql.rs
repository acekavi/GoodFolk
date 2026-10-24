//! Read-only GraphQL API. All writes go through REST commands.

use crate::auth::TenantContext;
use crate::error::ApiError;
use crate::persisted;
use crate::state::AppState;
use async_graphql::{
    Context, EmptyMutation, EmptySubscription, Enum, InputObject, Json, Object, Request, Schema, SimpleObject,
    Variables,
};
use async_graphql_axum::GraphQLResponse;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use db::{Scope, Tx};
use identity::Permission;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use storage::{Bucket, Storage};
use time::{Date, Duration, OffsetDateTime};
use uuid::Uuid;

pub type GqlSchema = Schema<Query, EmptyMutation, EmptySubscription>;

pub fn build_schema(production: bool) -> GqlSchema {
    let builder = Schema::build(Query, EmptyMutation, EmptySubscription).limit_depth(8).limit_complexity(500);
    if production { builder.disable_introspection().finish() } else { builder.finish() }
}

/// A GraphQL POST body. `documentId` names a persisted document (see `persisted`).
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphqlBody {
    document_id: Option<String>,
    query: Option<String>,
    operation_name: Option<String>,
    #[serde(default)]
    variables: Variables,
}

pub async fn handler(
    State(state): State<AppState>,
    ctx: TenantContext,
    body: Result<axum::Json<GraphqlBody>, JsonRejection>,
) -> Result<GraphQLResponse, ApiError> {
    let axum::Json(body) = body.map_err(|rejection| ApiError::bad_request(rejection.body_text()))?;
    let mut request = resolve_request(&body, state.production)?.variables(body.variables);
    if let Some(name) = body.operation_name {
        request = request.operation_name(name);
    }
    Ok(state.schema.execute(request.data(state.pool.clone()).data(state.storage.clone()).data(ctx)).await.into())
}

/// The request to run. A `documentId` runs the stored document, already parsed; a raw `query` runs only outside
/// production, where a raw `query` sent beside an id (the development client does) wins, so a document that is
/// not yet in the generated file still works.
fn resolve_request(body: &GraphqlBody, production: bool) -> Result<Request, ApiError> {
    match (&body.document_id, &body.query) {
        (_, Some(query)) if !production => Ok(Request::new(query)),
        (Some(id), query) => {
            let document =
                persisted::documents().get(id).ok_or_else(|| ApiError::bad_request("PersistedQueryNotFound"))?;
            if query.as_ref().is_some_and(|query| *query != document.text) {
                return Err(ApiError::bad_request("the query does not match the persisted document"));
            }
            let mut request = Request::new(&document.text);
            request.set_parsed_query(document.parsed.clone());
            Ok(request)
        }
        (None, _) => Err(ApiError::bad_request("production accepts only persisted queries")),
    }
}

/// Logs a database error and hides it from the client, like `ApiError` does for REST.
/// Resolvers map every database call through this.
fn internal(err: sqlx::Error) -> async_graphql::Error {
    tracing::error!(error = %err, "database error");
    async_graphql::Error::new("Internal error")
}

#[derive(SimpleObject)]
pub struct PropertyNode {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub timezone: String,
    pub base_currency: String,
    /// `HH:MM`, local time.
    pub check_in_time: String,
    /// `HH:MM`, local time.
    pub check_out_time: String,
    /// `HH:MM`, local time: when the night audit closes the business date if nobody has.
    pub audit_time: String,
    pub business_date: Date,
    /// Send as `If-Match: "<version>"` when updating.
    pub version: i32,
}

#[derive(SimpleObject)]
pub struct BedNode {
    pub kind: String,
    pub count: i32,
}

#[derive(SimpleObject)]
pub struct RoomTypeNode {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub base_occupancy: i32,
    pub max_adults: i32,
    pub max_children: i32,
    pub max_occupancy: i32,
    /// Rooms of this type that may be sold beyond the physical count.
    pub overbooking: i32,
    pub beds: Vec<BedNode>,
    pub amenities: Vec<String>,
    pub sort_order: i32,
    pub active: bool,
    pub version: i32,
    /// The gallery, in order, with permanent public URLs.
    pub media: Vec<MediaNode>,
}

impl From<rooms::RoomType> for RoomTypeNode {
    fn from(t: rooms::RoomType) -> Self {
        Self {
            id: t.id,
            code: t.code,
            name: t.name,
            base_occupancy: t.base_occupancy,
            max_adults: t.max_adults,
            max_children: t.max_children,
            max_occupancy: t.max_occupancy,
            overbooking: t.overbooking,
            beds: t.bed_config.into_iter().map(|bed| BedNode { kind: bed.kind, count: bed.count }).collect(),
            amenities: t.amenities,
            sort_order: t.sort_order,
            active: t.active,
            version: t.version,
            media: Vec::new(),
        }
    }
}

#[derive(SimpleObject)]
pub struct RoomNode {
    pub id: Uuid,
    pub room_type_id: Uuid,
    pub number: String,
    pub floor: Option<String>,
    pub section_id: Option<Uuid>,
    pub active: bool,
    pub sort_order: i32,
    pub version: i32,
}

impl From<rooms::Room> for RoomNode {
    fn from(r: rooms::Room) -> Self {
        Self {
            id: r.id,
            room_type_id: r.room_type_id,
            number: r.number,
            floor: r.floor,
            section_id: r.section_id,
            active: r.active,
            sort_order: r.sort_order,
            version: r.version,
        }
    }
}

#[derive(SimpleObject)]
pub struct SectionNode {
    pub id: Uuid,
    pub name: String,
    pub version: i32,
}

#[derive(Enum, Clone, Copy, PartialEq, Eq)]
#[graphql(name = "BlockKind")]
pub enum BlockKindNode {
    /// Out of inventory: reduces availability.
    OutOfOrder,
    /// Still sellable; shown only.
    OutOfService,
}

impl From<rooms::BlockKind> for BlockKindNode {
    fn from(kind: rooms::BlockKind) -> Self {
        match kind {
            rooms::BlockKind::OutOfOrder => BlockKindNode::OutOfOrder,
            rooms::BlockKind::OutOfService => BlockKindNode::OutOfService,
        }
    }
}

#[derive(SimpleObject)]
pub struct BlockReasonNode {
    pub id: Uuid,
    pub code: String,
    pub label: String,
    pub default_kind: BlockKindNode,
    pub active: bool,
    pub version: i32,
}

/// A room blocked for `[from, to)`: `to` is the first day it is back.
#[derive(SimpleObject)]
pub struct BlockNode {
    pub id: Uuid,
    pub room_id: Uuid,
    pub from: Date,
    pub to: Date,
    pub kind: BlockKindNode,
    pub reason_id: Uuid,
    pub note: String,
    pub version: i32,
}

/// One room type on one day. `available = physical - sold - outOfOrder`; `sellable = available +` the room
/// type's overbooking allowance, the true figure front desk may still sell.
#[derive(SimpleObject)]
pub struct InventoryDayNode {
    pub date: Date,
    pub room_type_id: Uuid,
    pub physical: i32,
    pub sold: i32,
    pub out_of_order: i32,
    pub available: i32,
    pub sellable: i32,
}

/// A GraphQL enum mirroring one of a module's enums, with conversions both ways.
macro_rules! mirror_enum {
    ($(#[$meta:meta])* $node:ident as $name:literal from $source:path { $($variant:ident),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Enum, Clone, Copy, PartialEq, Eq)]
        #[graphql(name = $name)]
        pub enum $node {
            $($variant),+
        }

        impl From<$source> for $node {
            fn from(value: $source) -> Self {
                match value {
                    $(<$source>::$variant => $node::$variant),+
                }
            }
        }

        impl From<$node> for $source {
            fn from(value: $node) -> Self {
                match value {
                    $($node::$variant => <$source>::$variant),+
                }
            }
        }
    };
}

mirror_enum!(PlanKindNode as "PlanKind" from rates::PlanKind { Standard, Derived, Custom });
mirror_enum!(SegmentNode as "Segment" from rates::Segment { FitF, FitL, Ota, Ta, Ibe });
mirror_enum!(ResidencyNode as "Residency" from rates::Residency { Resident, NonResident });
mirror_enum!(ChangeModeNode as "ChangeMode" from rates::ChangeMode { Percent, Amount });
mirror_enum!(
    /// Why a stay has no room.
    NeedsRoomReasonNode as "NeedsRoomReason" from reservations::NeedsRoomReason { Overbooked, NoSingleRoom }
);
mirror_enum!(MealPlanNode as "MealPlan" from rates::MealPlan { Ro, Bb, Hb, Fb });
mirror_enum!(PriceChangeModeNode as "PriceChangeMode" from rates::PriceChangeMode { Percent, Amount, Set });
mirror_enum!(PenaltyKindNode as "PenaltyKind" from rates::PenaltyKind { Nights, Percent, Amount });
mirror_enum!(ViolationKindNode as "ViolationKind" from rates::ViolationKind {
    InvalidStay,
    Inactive,
    Residency,
    RoomTypeNotSold,
    Occupancy,
    MealPlanNotAllowed,
    NoPrice,
    NoMealSupplement,
    Closed,
    MinStay,
    MaxStay,
    ClosedToArrival,
    ClosedToDeparture,
});

mirror_enum!(
    /// Where an uploaded file is: uploading, processing, then ready or failed.
    MediaStatusNode as "MediaStatus" from media::AssetStatus { Uploading, Processing, Ready, Failed }
);
mirror_enum!(
    /// A delivered image's size: `THUMB` is a 320 px square, the others are long edges of 640 to 2400 px.
    RenditionSizeNode as "RenditionSize" from media_types::Size { Thumb, Sm, Md, Lg, Xl }
);
mirror_enum!(ImageFormatNode as "ImageFormat" from media_types::Format { Avif, Webp });
mirror_enum!(DocumentKindNode as "DocumentKind" from media::DocumentKind { Passport, Nic, DrivingLicence, Other });

/// One delivered image of a photo.
#[derive(SimpleObject)]
pub struct RenditionNode {
    pub size: RenditionSizeNode,
    pub format: ImageFormatNode,
    pub width: i32,
    pub height: i32,
    pub url: String,
}

/// A photo. Its renditions exist once it is ready.
#[derive(SimpleObject)]
pub struct MediaNode {
    pub asset_id: Uuid,
    pub status: MediaStatusNode,
    /// A gallery photo's caption, if it has one.
    pub caption: Option<String>,
    /// Why processing failed, for a `FAILED` photo.
    pub error: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub renditions: Vec<RenditionNode>,
}

/// How long an issue photo's URLs work.
const ISSUE_PHOTO_LINK_TTL: std::time::Duration = std::time::Duration::from_secs(60 * 60);

fn rendition_node(rendition: media_types::RenditionOut, url: String) -> RenditionNode {
    RenditionNode {
        size: rendition.size.into(),
        format: rendition.format.into(),
        width: i32::try_from(rendition.width).unwrap_or(i32::MAX),
        height: i32::try_from(rendition.height).unwrap_or(i32::MAX),
        url,
    }
}

/// A gallery photo, with public URLs: only gallery renditions are ever written to the public bucket.
fn gallery_node(storage: &Storage, item: media::GalleryItem) -> MediaNode {
    MediaNode {
        asset_id: item.asset_id,
        status: item.status.into(),
        caption: Some(item.caption).filter(|caption| !caption.is_empty()),
        error: item.error,
        width: item.width,
        height: item.height,
        renditions: item
            .renditions
            .into_iter()
            .map(|r| {
                let url = storage.public_url(&r.key).into();
                rendition_node(r, url)
            })
            .collect(),
    }
}

/// An issue photo, with URLs signed now for [`ISSUE_PHOTO_LINK_TTL`].
fn issue_photo_node(storage: &Storage, asset: media::AssetRow) -> MediaNode {
    let url = |key: &str| match asset.visibility {
        media::Visibility::Private => storage.presign_get(Bucket::Private, key, ISSUE_PHOTO_LINK_TTL).into(),
        media::Visibility::Public => storage.public_url(key).into(),
    };
    let renditions = asset.renditions.iter().map(|r| rendition_node(r.clone(), url(&r.key))).collect();
    MediaNode {
        asset_id: asset.id,
        status: asset.status.into(),
        caption: None,
        error: asset.error,
        width: asset.width,
        height: asset.height,
        renditions,
    }
}

/// A guest's ID document. It carries no URL: each link comes from the audited REST call.
#[derive(SimpleObject)]
pub struct GuestDocumentNode {
    pub id: Uuid,
    pub kind: DocumentKindNode,
    pub status: MediaStatusNode,
    pub mime: Option<String>,
    pub uploaded_at: OffsetDateTime,
    pub uploaded_by: Option<Uuid>,
    /// The uploader's display name; null once their account is gone.
    pub uploaded_by_name: Option<String>,
}

impl From<media::GuestDocumentRow> for GuestDocumentNode {
    fn from(document: media::GuestDocumentRow) -> Self {
        Self {
            id: document.id,
            kind: document.kind.into(),
            status: document.status.into(),
            mime: document.mime,
            uploaded_at: document.uploaded_at,
            uploaded_by: document.uploaded_by,
            uploaded_by_name: document.uploaded_by_name,
        }
    }
}

/// A media rule the query broke, as a GraphQL error; database errors stay hidden.
fn media_error(err: media::MediaError) -> async_graphql::Error {
    match err {
        media::MediaError::Database(db_err) => internal(db_err),
        other => async_graphql::Error::new(other.to_string()),
    }
}

/// A rate plan. Amounts are minor units in its currency.
#[derive(SimpleObject)]
pub struct RatePlanNode {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub kind: PlanKindNode,
    pub segment: SegmentNode,
    /// `null`: any guest.
    pub residency: Option<ResidencyNode>,
    pub currency: String,
    pub parent_id: Option<Uuid>,
    /// Levels of plans above this one; the list is in tree order, so this indents it.
    pub depth: i32,
    pub derive_mode: Option<ChangeModeNode>,
    pub derive_value: Option<i64>,
    pub rounding_step: i64,
    pub extra_adult_amount: i64,
    pub inherit_restrictions: bool,
    pub allowed_meal_plans: Vec<MealPlanNode>,
    pub cancellation_policy_id: Option<Uuid>,
    pub room_type_ids: Vec<Uuid>,
    pub active: bool,
    pub version: i32,
}

impl From<rates::RatePlan> for RatePlanNode {
    fn from(p: rates::RatePlan) -> Self {
        Self {
            id: p.id,
            code: p.code,
            name: p.name,
            kind: p.kind.into(),
            segment: p.segment.into(),
            residency: p.residency.map(Into::into),
            currency: p.currency,
            parent_id: p.parent_id,
            depth: p.depth,
            derive_mode: p.derive_mode.map(Into::into),
            derive_value: p.derive_value,
            rounding_step: p.rounding_step,
            extra_adult_amount: p.extra_adult_amount,
            inherit_restrictions: p.inherit_restrictions,
            allowed_meal_plans: p.allowed_meal_plans.into_iter().map(Into::into).collect(),
            cancellation_policy_id: p.cancellation_policy_id,
            room_type_ids: p.room_type_ids,
            active: p.active,
            version: p.version,
        }
    }
}

/// A price for `occupancy` adults, in minor units of the plan's currency.
#[derive(SimpleObject)]
pub struct RatePriceNode {
    pub room_type_id: Uuid,
    pub date: Date,
    pub occupancy: i32,
    pub amount: i64,
}

#[derive(SimpleObject)]
pub struct RestrictionNode {
    pub room_type_id: Uuid,
    pub date: Date,
    pub closed: bool,
    pub min_stay: Option<i32>,
    pub max_stay: Option<i32>,
    pub closed_to_arrival: bool,
    pub closed_to_departure: bool,
}

/// A plan's resolved prices and restrictions for a date range. Days without a row have none.
#[derive(SimpleObject)]
pub struct RateGridNode {
    pub prices: Vec<RatePriceNode>,
    pub restrictions: Vec<RestrictionNode>,
}

/// One price a bulk change would change: `before` is `null` where `SET` adds a price.
#[derive(SimpleObject)]
pub struct PriceChangeCellNode {
    pub room_type_id: Uuid,
    pub date: Date,
    pub occupancy: i32,
    pub before: Option<i64>,
    pub after: i64,
}

#[derive(SimpleObject)]
pub struct BulkPreviewNode {
    /// How many of the plan's prices would change.
    pub total: i64,
    /// The first 50, by date, room type and occupancy.
    pub cells: Vec<PriceChangeCellNode>,
}

/// Per person per night on top of the room price, for the nights from `from` until `to` (`null`: open-ended).
#[derive(SimpleObject)]
pub struct MealSupplementNode {
    pub id: Uuid,
    pub meal_plan: MealPlanNode,
    pub currency: String,
    pub adult_amount: i64,
    pub child_amount: i64,
    pub from: Date,
    pub to: Option<Date>,
    pub version: i32,
}

#[derive(SimpleObject)]
pub struct PenaltyNode {
    pub kind: PenaltyKindNode,
    pub value: i64,
}

impl From<rates::Penalty> for PenaltyNode {
    fn from(penalty: rates::Penalty) -> Self {
        Self { kind: penalty.kind.into(), value: penalty.value }
    }
}

#[derive(SimpleObject)]
pub struct CancellationRuleNode {
    pub days_before_arrival: i32,
    pub penalty: PenaltyNode,
}

#[derive(SimpleObject)]
pub struct CancellationPolicyNode {
    pub id: Uuid,
    pub name: String,
    /// Furthest from arrival first.
    pub rules: Vec<CancellationRuleNode>,
    pub no_show: PenaltyNode,
    pub version: i32,
}

#[derive(SimpleObject)]
pub struct QuoteNightNode {
    pub date: Date,
    pub room: i64,
    pub meal: i64,
}

#[derive(SimpleObject)]
pub struct ViolationNode {
    pub kind: ViolationKindNode,
    pub date: Option<Date>,
    pub message: String,
}

/// What a stay costs, in minor units of `currency`. `restrictionsOk` is true when nothing stops the sale.
#[derive(SimpleObject)]
pub struct QuoteNode {
    pub nights: Vec<QuoteNightNode>,
    pub total: i64,
    pub currency: String,
    pub restrictions_ok: bool,
    pub violations: Vec<ViolationNode>,
}

mirror_enum!(RoomStatusNode as "RoomStatus" from domain::RoomStatus {
    Tentative,
    Confirmed,
    CheckedIn,
    CheckedOut,
    Cancelled,
    NoShow,
});
mirror_enum!(SourceNode as "Source" from reservations::Source { FrontDesk, Ibe, Channel, Phone, Email });
mirror_enum!(IdDocTypeNode as "IdDocType" from reservations::IdDocType { Passport, Nic, DrivingLicence, Other });
mirror_enum!(
    /// What the reservations list is sorted by; ties go by the room's id.
    ReservationSortFieldNode as "ReservationSortField" from reservations::SortField {
        Arrival,
        Confirmation,
        Guest,
        Created,
    }
);
mirror_enum!(SortDirectionNode as "SortDirection" from reservations::SortDirection { Asc, Desc });

/// One way to sell a room type for the stay: a rate plan and meal plan, priced. Unsellable offers carry the
/// reasons in `violations`.
#[derive(SimpleObject)]
pub struct OfferNode {
    pub rate_plan_id: Uuid,
    pub rate_plan_code: String,
    pub meal_plan: MealPlanNode,
    pub total: i64,
    pub currency: String,
    pub restrictions_ok: bool,
    pub violations: Vec<ViolationNode>,
    pub nights: Vec<QuoteNightNode>,
}

/// An active room type: the fewest rooms free on any night of the stay (negative when overbooked) and its
/// offers, by plan code then meal plan.
#[derive(SimpleObject)]
pub struct RoomTypeAvailabilityNode {
    pub room_type_id: Uuid,
    pub code: String,
    pub name: String,
    pub free: i32,
    pub offers: Vec<OfferNode>,
}

/// A guest. The ID number is only ever shown masked, such as `•••• 1234`.
#[derive(SimpleObject)]
pub struct GuestNode {
    pub id: Uuid,
    pub first_name: String,
    pub last_name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub country: Option<String>,
    pub residency: ResidencyNode,
    pub id_doc_type: Option<IdDocTypeNode>,
    pub id_doc_masked: Option<String>,
    pub notes: String,
    pub version: i32,
}

mirror_enum!(AccountKindNode as "AccountKind" from reservations::AccountKind { Company, TravelAgent });

/// How to reach an account: every field optional, filled in as known.
#[derive(SimpleObject)]
pub struct AccountContactNode {
    pub email: Option<String>,
    pub phone: Option<String>,
    pub address: Option<String>,
    pub contact_name: Option<String>,
}

impl From<reservations::AccountContact> for AccountContactNode {
    fn from(c: reservations::AccountContact) -> Self {
        Self { email: c.email, phone: c.phone, address: c.address, contact_name: c.contact_name }
    }
}

/// A company or travel agent a reservation can be billed to.
#[derive(SimpleObject)]
pub struct AccountNode {
    pub id: Uuid,
    pub kind: AccountKindNode,
    pub name: String,
    pub contact: AccountContactNode,
    /// In minor units of `currency`; `null` for no limit.
    pub credit_limit: Option<i64>,
    pub currency: String,
    pub active: bool,
    pub version: i32,
}

impl From<reservations::Account> for AccountNode {
    fn from(a: reservations::Account) -> Self {
        Self {
            id: a.id,
            kind: a.kind.into(),
            name: a.name,
            contact: a.contact.into(),
            credit_limit: a.credit_limit,
            currency: a.currency,
            active: a.active,
            version: a.version,
        }
    }
}

/// The account a reservation is billed to.
#[derive(SimpleObject)]
pub struct AccountRefNode {
    pub id: Uuid,
    pub name: String,
    pub kind: AccountKindNode,
}

impl From<reservations::AccountRef> for AccountRefNode {
    fn from(a: reservations::AccountRef) -> Self {
        Self { id: a.id, name: a.name, kind: a.kind.into() }
    }
}

impl From<reservations::Guest> for GuestNode {
    fn from(g: reservations::Guest) -> Self {
        Self {
            id: g.id,
            first_name: g.first_name,
            last_name: g.last_name,
            email: g.email,
            phone: g.phone,
            country: g.country,
            residency: g.residency.into(),
            id_doc_type: g.id_doc_type.map(Into::into),
            id_doc_masked: g.id_doc_masked,
            notes: g.notes,
            version: g.version,
        }
    }
}

/// Which reservation rooms to list. Left out, a field does not filter; an empty `statuses` or `sources`
/// matches nothing.
#[derive(InputObject)]
#[graphql(name = "ReservationFilter")]
pub struct ReservationFilterInput {
    pub arrival_from: Option<Date>,
    /// Inclusive.
    pub arrival_to: Option<Date>,
    pub statuses: Option<Vec<RoomStatusNode>>,
    pub sources: Option<Vec<SourceNode>>,
    /// The start of a confirmation number, in any case, or a guest's name, typos included.
    pub text: Option<String>,
}

#[derive(InputObject)]
#[graphql(name = "ReservationSort")]
pub struct ReservationSortInput {
    pub field: ReservationSortFieldNode,
    #[graphql(default_with = "SortDirectionNode::Asc")]
    pub direction: SortDirectionNode,
}

/// One room of a reservation in the list. `total` is the stay's price in minor units of `currency`.
#[derive(SimpleObject)]
pub struct ReservationRoomRowNode {
    pub id: Uuid,
    pub reservation_id: Uuid,
    pub confirmation_no: String,
    /// The primary guest's name.
    pub guest_name: String,
    pub arrival: Date,
    pub departure: Date,
    pub nights: i32,
    pub room_type_code: String,
    pub room_number: Option<String>,
    pub status: RoomStatusNode,
    pub source: SourceNode,
    pub total: i64,
    pub currency: String,
    pub version: i32,
    /// The account the reservation is billed to, if any; `null` when it is billed to the guest.
    pub account_name: Option<String>,
}

#[derive(SimpleObject)]
pub struct PageInfo {
    /// Pass as `after` for the next page; `null` on an empty page.
    pub end_cursor: Option<String>,
    pub has_next_page: bool,
}

#[derive(SimpleObject)]
pub struct ReservationRoomConnection {
    pub nodes: Vec<ReservationRoomRowNode>,
    pub page_info: PageInfo,
    /// Every room the filter matches, on every page.
    pub total_count: i64,
}

#[derive(SimpleObject)]
pub struct TotalNode {
    pub currency: String,
    pub amount: i64,
}

#[derive(SimpleObject)]
pub struct RoomTypeRefNode {
    pub id: Uuid,
    pub code: String,
    pub name: String,
}

#[derive(SimpleObject)]
pub struct RoomRefNode {
    pub id: Uuid,
    pub number: String,
}

#[derive(SimpleObject)]
pub struct RatePlanRefNode {
    pub id: Uuid,
    pub code: String,
}

/// The cancellation policy a room was booked under.
#[derive(SimpleObject)]
pub struct CancellationTermsNode {
    pub rules: Vec<CancellationRuleNode>,
    pub no_show: PenaltyNode,
}

/// A booked room. Amounts are minor units of `currency`.
#[derive(SimpleObject)]
pub struct ReservationRoomNode {
    pub id: Uuid,
    /// Send as `If-Match: "<version>"` with the room's commands.
    pub version: i32,
    pub status: RoomStatusNode,
    pub room_type: RoomTypeRefNode,
    /// `null` until a room is assigned.
    pub room: Option<RoomRefNode>,
    pub check_in: Date,
    pub check_out: Date,
    pub adults: i32,
    pub children: i32,
    pub rate_plan: RatePlanRefNode,
    pub meal_plan: MealPlanNode,
    pub primary_guest: GuestNode,
    /// Other guests staying in the room, besides the primary guest, masked the same way.
    pub occupants: Vec<GuestNode>,
    /// Each night's price as booked.
    pub nights: Vec<QuoteNightNode>,
    pub total: i64,
    pub currency: String,
    /// `null` when the plan had no cancellation policy.
    pub cancellation_terms: Option<CancellationTermsNode>,
    /// What cancelling on the business date would cost; `null` when the room can't be cancelled.
    pub cancellation_penalty: Option<i64>,
    pub cancelled_at: Option<OffsetDateTime>,
    /// The penalty recorded when the room was cancelled.
    pub recorded_penalty: Option<i64>,
    pub checked_in_at: Option<OffsetDateTime>,
    pub checked_in_business_date: Option<Date>,
    pub checked_out_at: Option<OffsetDateTime>,
    /// Whether the check-in command would accept this room right now, computed server-side (status, business
    /// date, room assignment) the same way the command itself checks it. It describes the room, not the
    /// caller: the command also needs `FrontDeskCheckIn`, which the client checks from the session.
    pub can_check_in: bool,
    /// As `canCheckIn`, for undoing a same-day check-in.
    pub can_undo_check_in: bool,
    /// As `canCheckIn`, for checking the room out.
    pub can_check_out: bool,
}

/// Something done to the reservation or one of its rooms.
#[derive(SimpleObject)]
pub struct HistoryEntryNode {
    /// Such as `reservation.created` or `reservation_room.assigned`.
    pub action: String,
    pub at: OffsetDateTime,
    /// `null` once the user is deleted.
    pub actor_name: Option<String>,
    pub data: Json<serde_json::Value>,
}

#[derive(SimpleObject)]
pub struct ReservationNode {
    pub id: Uuid,
    pub confirmation_no: String,
    /// Derived from the rooms' statuses.
    pub status: RoomStatusNode,
    pub source: SourceNode,
    pub notes: String,
    pub created_at: OffsetDateTime,
    /// Moves with every change to the reservation or its rooms.
    pub version: i32,
    pub booker: GuestNode,
    /// The company or travel agent this reservation is billed to; `null` if it is billed to the guest.
    pub account: Option<AccountRefNode>,
    /// What the rooms that are not cancelled cost, per currency.
    pub totals: Vec<TotalNode>,
    /// In the order they were booked.
    pub rooms: Vec<ReservationRoomNode>,
    /// Newest first.
    pub history: Vec<HistoryEntryNode>,
}

impl ReservationNode {
    fn new(r: reservations::ReservationDetail, history: Vec<reservations::HistoryEntry>) -> Self {
        Self {
            id: r.id,
            confirmation_no: r.confirmation_no,
            status: r.status.into(),
            source: r.source.into(),
            notes: r.notes,
            created_at: r.created_at,
            version: r.version,
            booker: r.booker.into(),
            account: r.account.map(AccountRefNode::from),
            totals: r.totals.into_iter().map(|t| TotalNode { currency: t.currency, amount: t.amount }).collect(),
            rooms: r
                .rooms
                .into_iter()
                .map(|room| ReservationRoomNode {
                    id: room.id,
                    version: room.version,
                    status: room.status.into(),
                    room_type: RoomTypeRefNode {
                        id: room.room_type.id,
                        code: room.room_type.code,
                        name: room.room_type.name,
                    },
                    room: room.room.map(|assigned| RoomRefNode { id: assigned.id, number: assigned.number }),
                    check_in: room.check_in,
                    check_out: room.check_out,
                    adults: room.adults,
                    children: room.children,
                    rate_plan: RatePlanRefNode { id: room.rate_plan.id, code: room.rate_plan.code },
                    meal_plan: room.meal_plan.into(),
                    primary_guest: room.primary_guest.into(),
                    occupants: room.occupants.into_iter().map(GuestNode::from).collect(),
                    nights: room
                        .nights
                        .into_iter()
                        .map(|n| QuoteNightNode { date: n.date, room: n.room, meal: n.meal })
                        .collect(),
                    total: room.total,
                    currency: room.currency,
                    cancellation_terms: room.cancellation_terms.map(|terms| CancellationTermsNode {
                        rules: terms.rules.into_iter().map(CancellationRuleNode::from).collect(),
                        no_show: terms.no_show.into(),
                    }),
                    cancellation_penalty: room.cancellation_penalty,
                    cancelled_at: room.cancelled_at,
                    recorded_penalty: room.recorded_penalty,
                    checked_in_at: room.checked_in_at,
                    checked_in_business_date: room.checked_in_business_date,
                    checked_out_at: room.checked_out_at,
                    can_check_in: room.can_check_in,
                    can_undo_check_in: room.can_undo_check_in,
                    can_check_out: room.can_check_out,
                })
                .collect(),
            history: history
                .into_iter()
                .map(|h| HistoryEntryNode { action: h.action, at: h.at, actor_name: h.actor_name, data: Json(h.data) })
                .collect(),
        }
    }
}

/// A room a stay could be assigned.
#[derive(SimpleObject)]
pub struct FreeRoomNode {
    pub id: Uuid,
    pub number: String,
    /// The housekeeping section's name.
    pub section: Option<String>,
}

/// A stay in one of the tape chart's rooms.
#[derive(SimpleObject)]
pub struct TapeStayNode {
    pub id: Uuid,
    pub reservation_id: Uuid,
    pub room_id: Uuid,
    pub room_type_id: Uuid,
    pub start: Date,
    pub end: Date,
    pub status: RoomStatusNode,
    /// The primary guest as "Silva, A.".
    pub guest_name: String,
    pub account_name: Option<String>,
    /// Send as `If-Match: "<version>"` with the stay's commands.
    pub version: i32,
}

/// A room block in one of the tape chart's rooms.
#[derive(SimpleObject)]
pub struct TapeBlockNode {
    pub id: Uuid,
    pub room_id: Uuid,
    pub start: Date,
    pub end: Date,
    /// The block reason's label.
    pub reason: String,
}

#[derive(SimpleObject)]
pub struct TapeWindowNode {
    /// By room, then arrival.
    pub stays: Vec<TapeStayNode>,
    /// By room, then start.
    pub blocks: Vec<TapeBlockNode>,
}

/// A confirmed stay that has no room yet.
#[derive(SimpleObject)]
pub struct UnassignedStayNode {
    pub id: Uuid,
    pub reservation_id: Uuid,
    pub room_type_id: Uuid,
    pub start: Date,
    pub end: Date,
    /// Always `CONFIRMED`: check-in needs a room, so only confirmed stays can be unassigned.
    pub status: RoomStatusNode,
    pub guest_name: String,
    pub reason: NeedsRoomReasonNode,
    /// Send as `If-Match: "<version>"` with the stay's commands.
    pub version: i32,
}

/// How ready a room is for a guest.
#[derive(Enum, Clone, Copy, PartialEq, Eq)]
#[graphql(name = "RoomCondition")]
pub enum RoomConditionNode {
    Dirty,
    Cleaning,
    Clean,
    Inspected,
}

impl From<housekeeping::Condition> for RoomConditionNode {
    fn from(condition: housekeeping::Condition) -> Self {
        match condition {
            housekeeping::Condition::Dirty => RoomConditionNode::Dirty,
            housekeeping::Condition::Cleaning => RoomConditionNode::Cleaning,
            housekeeping::Condition::Clean => RoomConditionNode::Clean,
            housekeeping::Condition::Inspected => RoomConditionNode::Inspected,
        }
    }
}

/// What a room's stays make of it on the business date.
#[derive(Enum, Clone, Copy, PartialEq, Eq)]
#[graphql(name = "Occupancy")]
pub enum OccupancyNode {
    Vacant,
    StayOver,
    DueOut,
    DueIn,
    /// A guest leaves and another arrives the same day.
    Turnover,
}

impl From<housekeeping::Occupancy> for OccupancyNode {
    fn from(occupancy: housekeeping::Occupancy) -> Self {
        match occupancy {
            housekeeping::Occupancy::Vacant => OccupancyNode::Vacant,
            housekeeping::Occupancy::StayOver => OccupancyNode::StayOver,
            housekeeping::Occupancy::DueOut => OccupancyNode::DueOut,
            housekeeping::Occupancy::DueIn => OccupancyNode::DueIn,
            housekeeping::Occupancy::Turnover => OccupancyNode::Turnover,
        }
    }
}

/// How urgent a room issue is.
#[derive(Enum, Clone, Copy, PartialEq, Eq)]
#[graphql(name = "IssueSeverity")]
pub enum IssueSeverityNode {
    Low,
    Medium,
    High,
}

impl From<housekeeping::Severity> for IssueSeverityNode {
    fn from(severity: housekeeping::Severity) -> Self {
        match severity {
            housekeeping::Severity::Low => IssueSeverityNode::Low,
            housekeeping::Severity::Medium => IssueSeverityNode::Medium,
            housekeeping::Severity::High => IssueSeverityNode::High,
        }
    }
}

/// Whether a room issue still needs work.
#[derive(Enum, Clone, Copy, PartialEq, Eq)]
#[graphql(name = "IssueStatus")]
pub enum IssueStatusNode {
    Open,
    Resolved,
}

impl From<housekeeping::IssueStatus> for IssueStatusNode {
    fn from(status: housekeeping::IssueStatus) -> Self {
        match status {
            housekeeping::IssueStatus::Open => IssueStatusNode::Open,
            housekeeping::IssueStatus::Resolved => IssueStatusNode::Resolved,
        }
    }
}

impl From<IssueStatusNode> for housekeeping::IssueStatus {
    fn from(status: IssueStatusNode) -> Self {
        match status {
            IssueStatusNode::Open => housekeeping::IssueStatus::Open,
            IssueStatusNode::Resolved => housekeeping::IssueStatus::Resolved,
        }
    }
}

/// A user who can be assigned rooms.
#[derive(SimpleObject)]
pub struct HousekeeperNode {
    pub user_id: Uuid,
    pub display_name: String,
}

impl From<housekeeping::Housekeeper> for HousekeeperNode {
    fn from(housekeeper: housekeeping::Housekeeper) -> Self {
        Self { user_id: housekeeper.user_id, display_name: housekeeper.display_name }
    }
}

/// A room on the housekeeping board, with its state for the business date.
#[derive(SimpleObject)]
pub struct BoardRoomNode {
    pub room_id: Uuid,
    pub number: String,
    pub floor: Option<String>,
    pub section_id: Option<Uuid>,
    pub room_type_id: Uuid,
    pub condition: RoomConditionNode,
    /// Send as `If-Match: "<version>"` with a condition change; 0 for a room with no condition row.
    pub condition_version: i32,
    pub occupancy: OccupancyNode,
    /// Who cleans the room on the business date.
    pub assignee: Option<HousekeeperNode>,
    pub open_issues: i32,
    /// The kind of the block covering the business date, if any.
    pub block_kind: Option<BlockKindNode>,
}

impl From<housekeeping::BoardRoom> for BoardRoomNode {
    fn from(room: housekeeping::BoardRoom) -> Self {
        Self {
            room_id: room.room_id,
            number: room.number,
            floor: room.floor,
            section_id: room.section_id,
            room_type_id: room.room_type_id,
            condition: room.condition.into(),
            condition_version: room.condition_version,
            occupancy: room.occupancy.into(),
            assignee: room.assignee.map(Into::into),
            open_issues: room.open_issues,
            block_kind: room.block_kind.map(Into::into),
        }
    }
}

/// A problem found in a room.
#[derive(SimpleObject)]
pub struct RoomIssueNode {
    pub id: Uuid,
    pub room_id: Uuid,
    pub severity: IssueSeverityNode,
    pub description: String,
    pub status: IssueStatusNode,
    pub reported_by: Uuid,
    pub reported_at: OffsetDateTime,
    pub resolved_by: Option<Uuid>,
    pub resolved_at: Option<OffsetDateTime>,
    /// Send as `If-Match: "<version>"` when resolving.
    pub version: i32,
    /// Oldest first, with URLs that work for 60 minutes from the read.
    pub photos: Vec<MediaNode>,
}

impl From<housekeeping::Issue> for RoomIssueNode {
    fn from(issue: housekeeping::Issue) -> Self {
        Self {
            id: issue.id,
            room_id: issue.room_id,
            severity: issue.severity.into(),
            description: issue.description,
            status: issue.status.into(),
            reported_by: issue.reported_by,
            reported_at: issue.reported_at,
            resolved_by: issue.resolved_by,
            resolved_at: issue.resolved_at,
            version: issue.version,
            photos: Vec::new(),
        }
    }
}

impl From<rates::CancellationRule> for CancellationRuleNode {
    fn from(rule: rates::CancellationRule) -> Self {
        Self { days_before_arrival: rule.days_before_arrival, penalty: rule.penalty.into() }
    }
}

impl From<rates::QuoteNight> for QuoteNightNode {
    fn from(n: rates::QuoteNight) -> Self {
        Self { date: n.date, room: n.room, meal: n.meal }
    }
}

impl From<rates::Violation> for ViolationNode {
    fn from(v: rates::Violation) -> Self {
        Self { kind: v.kind.into(), date: v.date, message: v.message }
    }
}

mirror_enum!(
    /// Whose charges a folio holds: one stay's, an account's (the city ledger) or a reservation's deposits.
    FolioKindNode as "FolioKind" from folio::FolioKind { Guest, Account, Deposit }
);
mirror_enum!(
    /// An invoiced folio takes no lines or payments until a credit note reopens it.
    FolioStatusNode as "FolioStatus" from folio::FolioStatus { Open, Invoiced }
);
mirror_enum!(
    /// What posted a line.
    LineSourceNode as "LineSource" from folio::Source {
        Room,
        MealPlan,
        Laundry,
        Pos,
        Manual,
        CancellationFee,
        NoShowFee,
        Adjustment,
    }
);
mirror_enum!(TaxKindNode as "TaxKind" from tax::Kind { Tax, Levy, ServiceCharge });
mirror_enum!(
    /// `ON_BILL` taxes are added to the line's total; `ABSORBED` ones are calculated but borne by the property.
    TaxPresentationNode as "TaxPresentation" from tax::Presentation { OnBill, Absorbed }
);
mirror_enum!(InvoiceKindNode as "InvoiceKind" from media_types::InvoiceKind { Invoice, CreditNote });

/// A stored text value (a line tax's kind or presentation) as its `tax` enum.
fn tax_enum<T: serde::de::DeserializeOwned>(value: &str) -> async_graphql::Result<T> {
    serde_json::from_value(serde_json::Value::String(value.to_owned())).map_err(|_| {
        tracing::error!(value, "an unknown tax value is stored");
        async_graphql::Error::new("Internal error")
    })
}

/// One tax, levy or service charge of a line, as calculated when it was posted. Amounts are minor units of the
/// line's currency.
#[derive(SimpleObject)]
pub struct LineTaxNode {
    pub code: String,
    pub name: String,
    pub kind: TaxKindNode,
    pub rate_bp: i32,
    pub base: i64,
    pub amount: i64,
    pub presentation: TaxPresentationNode,
}

impl TryFrom<folio::LineTax> for LineTaxNode {
    type Error = async_graphql::Error;

    fn try_from(tax: folio::LineTax) -> async_graphql::Result<Self> {
        Ok(Self {
            kind: tax_enum::<tax::Kind>(&tax.kind)?.into(),
            presentation: tax_enum::<tax::Presentation>(&tax.presentation)?.into(),
            code: tax.code,
            name: tax.name,
            rate_bp: tax.rate_bp,
            base: tax.base,
            amount: tax.amount,
        })
    }
}

/// A charge on a folio. Lines never change: a correction is a reversal (`reverses`) or a transfer (`transferId`).
/// Amounts are minor units of `currency`; `baseTotal` is in the property's base currency at `rate`.
#[derive(SimpleObject)]
pub struct FolioLineNode {
    pub id: Uuid,
    pub source: LineSourceNode,
    pub category: String,
    pub description: String,
    pub quantity: i32,
    /// Before taxes.
    pub net: i64,
    /// Net plus the on-bill taxes.
    pub total: i64,
    pub currency: String,
    pub base_total: i64,
    /// Base currency per one unit of `currency`, as a decimal string.
    pub rate: String,
    /// The date the rate was published.
    pub rate_date: Date,
    pub business_date: Date,
    pub posting_key: Option<String>,
    /// The line this one reverses.
    pub reverses: Option<Uuid>,
    pub reason: Option<String>,
    /// Shared by the two lines of a transfer, and by a charge to account's line and payment.
    pub transfer_id: Option<Uuid>,
    pub posted_by: Option<Uuid>,
    pub posted_at: OffsetDateTime,
    /// By code.
    pub taxes: Vec<LineTaxNode>,
}

impl TryFrom<folio::LineWithTaxes> for FolioLineNode {
    type Error = async_graphql::Error;

    fn try_from(entry: folio::LineWithTaxes) -> async_graphql::Result<Self> {
        let line = entry.line;
        Ok(Self {
            id: line.id,
            source: line.source.into(),
            category: line.category,
            description: line.description,
            quantity: line.quantity,
            net: line.net,
            total: line.total,
            currency: line.currency,
            base_total: line.base_total,
            rate: line.rate.to_string(),
            rate_date: line.rate_date,
            business_date: line.business_date,
            posting_key: line.posting_key,
            reverses: line.reverses,
            reason: line.reason,
            transfer_id: line.transfer_id,
            posted_by: line.posted_by,
            posted_at: line.posted_at,
            taxes: entry.taxes.into_iter().map(LineTaxNode::try_from).collect::<async_graphql::Result<_>>()?,
        })
    }
}

/// A payment on a folio: positive when taken, negative for a refund (`refundOf`). Amounts are minor units of
/// `currency`; `baseAmount` is in the property's base currency.
#[derive(SimpleObject)]
pub struct PaymentNode {
    pub id: Uuid,
    /// `cash`, `card_terminal`, `bank_transfer`, `account` or `transfer` (a credit moving between two folios).
    pub method: String,
    pub amount: i64,
    pub currency: String,
    pub base_amount: i64,
    /// Base currency per one unit of `currency`, as a decimal string.
    pub rate: String,
    pub rate_date: Date,
    /// A card's `approval`, `brand` and `last4`, a bank transfer's `reference`, a refund's `reason`, or a transfer's
    /// `to_folio` / `from_folio`.
    pub details: Json<serde_json::Value>,
    pub evidence: Option<String>,
    pub refund_of: Option<Uuid>,
    pub business_date: Date,
    pub transfer_id: Option<Uuid>,
    pub taken_by: Option<Uuid>,
    pub taken_at: OffsetDateTime,
}

impl From<folio::PaymentRow> for PaymentNode {
    fn from(p: folio::PaymentRow) -> Self {
        Self {
            id: p.id,
            method: p.method,
            amount: p.amount,
            currency: p.currency,
            base_amount: p.base_amount,
            rate: p.rate.to_string(),
            rate_date: p.rate_date,
            details: Json(p.details),
            evidence: p.evidence,
            refund_of: p.refund_of,
            business_date: p.business_date,
            transfer_id: p.transfer_id,
            taken_by: p.taken_by,
            taken_at: p.taken_at,
        }
    }
}

/// What a folio holds in one currency: charges less payments, and the same in the base currency.
#[derive(SimpleObject)]
pub struct BalanceNode {
    pub currency: String,
    pub amount: i64,
    pub base_amount: i64,
}

/// An invoice or credit note. Its PDF is reached only through the audited REST link, once `pdfReady`.
#[derive(SimpleObject)]
pub struct InvoiceNode {
    pub id: Uuid,
    pub kind: InvoiceKindNode,
    pub serial: String,
    /// Whole rupees; negative for a credit note.
    pub total_lkr: i64,
    pub issued_at: OffsetDateTime,
    pub issued_by: Option<Uuid>,
    /// The invoice a credit note credits.
    pub credits: Option<Uuid>,
    pub pdf_ready: bool,
}

impl From<folio::InvoiceSummary> for InvoiceNode {
    fn from(i: folio::InvoiceSummary) -> Self {
        Self {
            id: i.id,
            kind: i.kind.into(),
            serial: i.serial,
            total_lkr: i.total_lkr,
            issued_at: i.issued_at,
            issued_by: i.issued_by,
            credits: i.credits,
            pdf_ready: i.pdf_ready,
        }
    }
}

#[derive(SimpleObject)]
pub struct FolioNode {
    pub id: Uuid,
    pub kind: FolioKindNode,
    pub status: FolioStatusNode,
    /// A guest folio's stay.
    pub reservation_room_id: Option<Uuid>,
    /// A deposit folio's reservation.
    pub reservation_id: Option<Uuid>,
    /// An account folio's account.
    pub account_id: Option<Uuid>,
    pub created_at: OffsetDateTime,
    /// Moves when the folio is invoiced or reopened.
    pub version: i32,
    /// Oldest first: at most the newest 500 (see `truncated`); empty when `linesOmitted`.
    pub lines: Vec<FolioLineNode>,
    /// Oldest first: at most the newest 500 (see `truncated`); empty when `linesOmitted`.
    pub payments: Vec<PaymentNode>,
    /// One per currency the folio has lines or payments in, by currency code, over all of them, whatever was read.
    pub balances: Vec<BalanceNode>,
    /// The lines and payments were left out: an account folio read through a reservation. Read it by `accountId`.
    pub lines_omitted: bool,
    /// The folio has more than 500 lines or payments; only the newest 500 of each are here.
    pub truncated: bool,
    /// Oldest first.
    pub invoices: Vec<InvoiceNode>,
}

impl TryFrom<folio::FolioView> for FolioNode {
    type Error = async_graphql::Error;

    fn try_from(view: folio::FolioView) -> async_graphql::Result<Self> {
        let folio = view.folio;
        Ok(Self {
            id: folio.id,
            kind: folio.kind.into(),
            status: folio.status.into(),
            reservation_room_id: folio.reservation_room_id,
            reservation_id: folio.reservation_id,
            account_id: folio.account_id,
            created_at: folio.created_at,
            version: folio.version,
            lines: view.lines.into_iter().map(FolioLineNode::try_from).collect::<async_graphql::Result<_>>()?,
            payments: view.payments.into_iter().map(PaymentNode::from).collect(),
            balances: view
                .balances
                .into_iter()
                .map(|b| BalanceNode { currency: b.currency, amount: b.amount, base_amount: b.base_amount })
                .collect(),
            invoices: Vec::new(),
            lines_omitted: view.lines_omitted,
            truncated: view.truncated,
        })
    }
}

/// A tax, levy or service charge of the property, in force over `[effectiveFrom, effectiveTo)`.
#[derive(SimpleObject)]
pub struct TaxRuleNode {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub kind: TaxKindNode,
    /// Basis points: 1000 is 10%.
    pub rate_bp: i32,
    /// `*`, `room`, `meal_plan`, `laundry`, `misc`, `fee`, `pos:*` or `pos:<OUTLET>`.
    pub applies_to: Vec<String>,
    /// `net` and the codes of rules calculated earlier.
    pub base: Vec<String>,
    pub sequence: i32,
    pub presentation: TaxPresentationNode,
    /// `{"type": "always"}`, `{"type": "vat_registered"}`, `{"type": "residency", "resident": true}` or
    /// `{"type": "payment_currency", "currencies": ["USD"]}`.
    pub condition: Json<serde_json::Value>,
    pub effective_from: Date,
    /// The first day the rule no longer applies; `null` while open-ended.
    pub effective_to: Option<Date>,
    pub version: i32,
}

impl From<folio::TaxRuleRow> for TaxRuleNode {
    fn from(r: folio::TaxRuleRow) -> Self {
        Self {
            id: r.id,
            code: r.code,
            name: r.name,
            kind: r.kind.into(),
            rate_bp: i32::try_from(r.rate_bp).unwrap_or(i32::MAX),
            applies_to: r.applies_to,
            base: r.base,
            sequence: r.sequence,
            presentation: r.presentation.into(),
            condition: Json(serde_json::json!(r.condition)),
            effective_from: r.effective_from,
            effective_to: r.effective_to,
            version: r.version,
        }
    }
}

/// A POS outlet. Its bills' category is `pos:<code>`.
#[derive(SimpleObject)]
pub struct OutletNode {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    /// Whether the property's service charge applies to the outlet's bills.
    pub service_charge: bool,
    pub active: bool,
    pub version: i32,
}

impl From<folio::OutletRow> for OutletNode {
    fn from(o: folio::OutletRow) -> Self {
        Self {
            id: o.id,
            code: o.code,
            name: o.name,
            service_charge: o.service_charge,
            active: o.active,
            version: o.version,
        }
    }
}

/// A stored exchange rate: base currency per one unit of `currency`, in force from `date` until the next one.
#[derive(SimpleObject)]
pub struct FxRateNode {
    pub date: Date,
    pub currency: String,
    /// A decimal string, such as `"301.250000"`.
    pub rate: String,
    /// `cbsl` (the Central Bank's TT selling rate) or `manual`.
    pub source: String,
    /// The day the source published it: earlier than `date` over a weekend or holiday.
    pub source_date: Date,
    /// Who set a manual rate.
    pub set_by: Option<Uuid>,
    pub set_at: OffsetDateTime,
}

impl From<fx::RateRow> for FxRateNode {
    fn from(r: fx::RateRow) -> Self {
        Self {
            date: r.date,
            currency: r.currency,
            rate: r.rate.to_string(),
            source: r.source,
            source_date: r.source_date,
            set_by: r.set_by,
            set_at: r.set_at,
        }
    }
}

/// Why the last exchange-rate import left the property without complete, current rates; shown until one does.
#[derive(SimpleObject)]
pub struct FxAlertNode {
    pub message: String,
    pub raised_at: OffsetDateTime,
}

/// What the property's invoices and taxes depend on.
#[derive(SimpleObject)]
pub struct TaxSettingsNode {
    pub vat_registered: bool,
    pub tin: Option<String>,
    /// The tourism development levy's small-turnover band, used when the preset is applied.
    pub tdl_small_turnover: bool,
    /// The supplier's name on invoices; the property's name when `null`.
    pub legal_name: Option<String>,
    pub address: Option<String>,
    pub base_currency: String,
    pub accepted_currencies: Vec<String>,
    /// The property's version: send it as `If-Match` when updating.
    pub version: i32,
}

impl From<property::TaxSettings> for TaxSettingsNode {
    fn from(s: property::TaxSettings) -> Self {
        Self {
            vat_registered: s.vat_registered,
            tin: s.tin,
            tdl_small_turnover: s.tdl_small_turnover,
            legal_name: s.legal_name,
            address: s.address,
            base_currency: s.base_currency,
            accepted_currencies: s.accepted_currencies,
            version: s.version,
        }
    }
}

/// A stay due by the business date and still tentative or confirmed.
#[derive(SimpleObject)]
pub struct AuditArrivalNode {
    pub reservation_room_id: Uuid,
    pub reservation_id: Uuid,
    pub confirmation: String,
    pub guest: String,
    pub room_number: Option<String>,
    pub arrival: Date,
    pub departure: Date,
    /// `TENTATIVE` (delay or cancel only) or `CONFIRMED`.
    pub status: RoomStatusNode,
    /// The stay's room and meal amounts, all nights, in minor units of `currency`.
    pub total: i64,
    pub currency: String,
    /// What marking it a no-show would cost under its booked terms, in minor units of `currency`.
    pub no_show_penalty: i64,
    /// The reservation room's version: send it as `If-Match`.
    pub version: i32,
}

impl From<audit::ArrivalRow> for AuditArrivalNode {
    fn from(a: audit::ArrivalRow) -> Self {
        Self {
            reservation_room_id: a.reservation_room,
            reservation_id: a.reservation_id,
            confirmation: a.confirmation,
            guest: a.guest,
            room_number: a.room_number,
            arrival: a.arrival,
            departure: a.departure,
            status: a.status.into(),
            total: a.total,
            currency: a.currency,
            no_show_penalty: a.no_show_penalty,
            version: a.version,
        }
    }
}

/// An amount in one currency, in its minor units.
#[derive(SimpleObject)]
pub struct CurrencyAmountNode {
    pub currency: String,
    pub amount: i64,
}

/// A stay due out by the business date and still checked in.
#[derive(SimpleObject)]
pub struct AuditDepartureNode {
    pub reservation_room_id: Uuid,
    pub reservation_id: Uuid,
    pub confirmation: String,
    pub guest: String,
    pub room_number: Option<String>,
    pub departure: Date,
    /// The guest folio's balance per currency, by currency code.
    pub balances: Vec<CurrencyAmountNode>,
    /// The reservation room's version: send it as `If-Match`.
    pub version: i32,
}

impl From<audit::DepartureRow> for AuditDepartureNode {
    fn from(d: audit::DepartureRow) -> Self {
        Self {
            reservation_room_id: d.reservation_room,
            reservation_id: d.reservation_id,
            confirmation: d.confirmation,
            guest: d.guest,
            room_number: d.room_number,
            departure: d.departure,
            balances: d
                .balances
                .into_iter()
                .map(|(currency, amount)| CurrencyAmountNode { currency, amount })
                .collect(),
            version: d.version,
        }
    }
}

/// A POS bill of the business date.
#[derive(SimpleObject)]
pub struct AuditPosBillNode {
    pub id: Uuid,
    pub bill_no: String,
    /// The bill's own business date: before the review's for a bill posted late, which counts on the day it posted.
    pub bill_date: Date,
    pub currency: String,
    pub total: i64,
    pub total_base: i64,
    /// The room of the stay it posted to; `null` when settled at the outlet.
    pub room_number: Option<String>,
    pub settled: bool,
    /// Posted to a stay now checked out whose guest folio is not settled.
    pub highlight: bool,
}

/// One outlet's POS bills of the business date.
#[derive(SimpleObject)]
pub struct AuditOutletBillsNode {
    /// The outlet's name.
    pub outlet: String,
    pub bills: Vec<AuditPosBillNode>,
    pub total_base: i64,
}

impl From<audit::OutletBills> for AuditOutletBillsNode {
    fn from(o: audit::OutletBills) -> Self {
        Self {
            outlet: o.outlet,
            bills: o
                .bills
                .into_iter()
                .map(|b| AuditPosBillNode {
                    id: b.id,
                    bill_no: b.bill_no,
                    bill_date: b.bill_date,
                    currency: b.currency,
                    total: b.total,
                    total_base: b.total_base,
                    room_number: b.room_number,
                    settled: b.settled,
                    highlight: b.highlight,
                })
                .collect(),
            total_base: o.total_base,
        }
    }
}

/// Tonight's room and meal-plan charges of one in-house stay, as posting them would go.
#[derive(SimpleObject)]
pub struct NightPreviewNode {
    pub reservation_room_id: Uuid,
    pub room_number: Option<String>,
    pub date: Date,
    pub room_amount: i64,
    pub meal_amount: i64,
    pub currency: String,
    /// Why posting would fail, such as a missing exchange rate; `null` when it would post.
    pub problem: Option<String>,
    /// How many of the night's charges are already posted: 0, 1 or 2.
    pub already_posted: i32,
}

/// The business date's manual and fee lines in one category.
#[derive(SimpleObject)]
pub struct CategoryTotalNode {
    pub category: String,
    pub lines: i32,
    pub total_base: i64,
}

#[derive(SimpleObject)]
pub struct BookingBillsNode {
    pub nights: Vec<NightPreviewNode>,
    pub posted: Vec<CategoryTotalNode>,
}

impl From<audit::BookingBills> for BookingBillsNode {
    fn from(b: audit::BookingBills) -> Self {
        Self {
            nights: b
                .nights
                .into_iter()
                .map(|n| NightPreviewNode {
                    reservation_room_id: n.reservation_room,
                    room_number: n.room_number,
                    date: n.date,
                    room_amount: n.room_amount,
                    meal_amount: n.meal_amount,
                    currency: n.currency,
                    problem: n.problem,
                    already_posted: i32::from(n.already_posted),
                })
                .collect(),
            posted: b
                .posted
                .into_iter()
                .map(|c| CategoryTotalNode {
                    category: c.category,
                    lines: i32::try_from(c.lines).unwrap_or(i32::MAX),
                    total_base: c.total_base,
                })
                .collect(),
        }
    }
}

/// The business date's payments in one method and currency, refunds netted into the amounts.
#[derive(SimpleObject)]
pub struct PaymentGroupNode {
    /// `cash`, `card_terminal`, `bank_transfer`, or `account` (a charge to account: the city ledger).
    pub method: String,
    pub currency: String,
    /// The payments, not the refunds.
    pub count: i32,
    pub amount: i64,
    pub base_amount: i64,
    /// The refunds' sum, at most 0.
    pub refunds: i64,
    /// LKR payments with no evidence on the folio of a stay sold to non-residents.
    pub flagged: Vec<Uuid>,
}

impl From<audit::PaymentGroup> for PaymentGroupNode {
    fn from(p: audit::PaymentGroup) -> Self {
        Self {
            method: p.method,
            currency: p.currency,
            count: i32::try_from(p.count).unwrap_or(i32::MAX),
            amount: p.amount,
            base_amount: p.base_amount,
            refunds: p.refunds,
            flagged: p.flagged,
        }
    }
}

/// The review of the property's business date before it closes.
#[derive(SimpleObject)]
pub struct NightAuditNode {
    pub business_date: Date,
    /// Unresolved arrivals plus unresolved departures: the close waits until it is 0.
    pub blocking: i32,
    pub arrivals: Vec<AuditArrivalNode>,
    pub departures: Vec<AuditDepartureNode>,
    pub pos_bills: Vec<AuditOutletBillsNode>,
    pub booking_bills: BookingBillsNode,
    pub payments: Vec<PaymentGroupNode>,
}

impl From<audit::NightAudit> for NightAuditNode {
    fn from(n: audit::NightAudit) -> Self {
        Self {
            business_date: n.business_date,
            blocking: i32::try_from(n.blocking).unwrap_or(i32::MAX),
            arrivals: n.arrivals.into_iter().map(AuditArrivalNode::from).collect(),
            departures: n.departures.into_iter().map(AuditDepartureNode::from).collect(),
            pos_bills: n.pos_bills.into_iter().map(AuditOutletBillsNode::from).collect(),
            booking_bills: n.booking_bills.into(),
            payments: n.payments.into_iter().map(PaymentGroupNode::from).collect(),
        }
    }
}

mirror_enum!(AuditModeNode as "AuditMode" from audit::Mode { Manual, Auto });
mirror_enum!(AuditRunStatusNode as "AuditRunStatus" from audit::RunStatus { Running, Failed, Completed });
mirror_enum!(
    /// The steps of a close, in the order they run.
    AuditStepNode as "AuditStep" from audit::Step {
        Resolve,
        PostCharges,
        LockFx,
        SnapshotStats,
        AdvanceBusinessDate,
    }
);

/// A step a run has done.
#[derive(SimpleObject)]
pub struct AuditRunStepNode {
    pub step: AuditStepNode,
    pub completed_at: OffsetDateTime,
    /// The step's report, such as the charges it posted.
    pub detail: Json<serde_json::Value>,
}

/// The close of one business date.
#[derive(SimpleObject)]
pub struct AuditRunNode {
    pub id: Uuid,
    pub business_date: Date,
    pub mode: AuditModeNode,
    pub status: AuditRunStatusNode,
    /// Who started it; `null` for the scheduled close.
    pub started_by: Option<Uuid>,
    pub attempts: i32,
    pub error: Option<String>,
    /// The failed step's report: `{"step": ...}`, with `"stays": [{reservation_room, room, error}]` when stays
    /// failed.
    pub failure: Option<Json<serde_json::Value>>,
    pub started_at: OffsetDateTime,
    pub finished_at: Option<OffsetDateTime>,
    /// The steps done, in step order.
    pub steps: Vec<AuditRunStepNode>,
}

impl From<audit::AuditRun> for AuditRunNode {
    fn from(r: audit::AuditRun) -> Self {
        Self {
            id: r.id,
            business_date: r.business_date,
            mode: r.mode.into(),
            status: r.status.into(),
            started_by: r.started_by,
            attempts: r.attempts,
            error: r.error,
            failure: r.failure.map(Json),
            started_at: r.started_at,
            finished_at: r.finished_at,
            steps: r
                .steps
                .into_iter()
                .map(|s| AuditRunStepNode { step: s.step.into(), completed_at: s.completed_at, detail: Json(s.detail) })
                .collect(),
        }
    }
}

/// A base-currency amount under a key, such as a tax code or a payment method.
#[derive(SimpleObject)]
pub struct KeyedAmountNode {
    pub key: String,
    pub amount: i64,
}

/// One key of a statistics dimension on a closed day.
#[derive(SimpleObject)]
pub struct StatsBreakdownNode {
    /// `segment`, `source`, `residency`, `room_type` or `outlet`.
    pub dimension: String,
    /// The dimension's value; `unattributed` for revenue no stay accounts for.
    pub key: String,
    pub room_nights: i32,
    pub room_revenue: i64,
    pub revenue: i64,
}

/// The statistics of a closed business date, in base-currency minor units, revenue net of tax.
#[derive(SimpleObject)]
pub struct DailyStatsNode {
    pub business_date: Date,
    pub rooms_total: i32,
    pub rooms_out_of_order: i32,
    pub rooms_available: i32,
    pub rooms_sold: i32,
    pub complimentary_rooms: i32,
    /// Occupancy in basis points: 10000 is full.
    pub occupancy_bp: i32,
    pub adults: i32,
    pub children: i32,
    pub arrivals: i32,
    pub departures: i32,
    pub no_shows: i32,
    pub cancellations: i32,
    pub walk_ins: i32,
    pub room_revenue: i64,
    pub meal_plan_revenue: i64,
    pub pos_revenue: i64,
    pub fee_revenue: i64,
    pub other_revenue: i64,
    pub adr: i64,
    pub revpar: i64,
    /// By tax code.
    pub taxes: Vec<KeyedAmountNode>,
    /// By payment method, refunds included.
    pub payments: Vec<KeyedAmountNode>,
    /// Each dimension's rows sum to the day's totals.
    pub breakdown: Vec<StatsBreakdownNode>,
}

impl From<audit::DailyStats> for DailyStatsNode {
    fn from(d: audit::DailyStats) -> Self {
        let keyed = |map: std::collections::BTreeMap<String, i64>| {
            map.into_iter().map(|(key, amount)| KeyedAmountNode { key, amount }).collect()
        };
        Self {
            business_date: d.business_date,
            rooms_total: d.rooms_total,
            rooms_out_of_order: d.rooms_out_of_order,
            rooms_available: d.rooms_available,
            rooms_sold: d.rooms_sold,
            complimentary_rooms: d.complimentary_rooms,
            occupancy_bp: d.occupancy_bp,
            adults: d.adults,
            children: d.children,
            arrivals: d.arrivals,
            departures: d.departures,
            no_shows: d.no_shows,
            cancellations: d.cancellations,
            walk_ins: d.walk_ins,
            room_revenue: d.room_revenue,
            meal_plan_revenue: d.meal_plan_revenue,
            pos_revenue: d.pos_revenue,
            fee_revenue: d.fee_revenue,
            other_revenue: d.other_revenue,
            adr: d.adr,
            revpar: d.revpar,
            taxes: keyed(d.taxes),
            payments: keyed(d.payments),
            breakdown: d
                .breakdown
                .into_iter()
                .map(|b| StatsBreakdownNode {
                    dimension: b.dimension,
                    key: b.key,
                    room_nights: b.room_nights,
                    room_revenue: b.room_revenue,
                    revenue: b.revenue,
                })
                .collect(),
        }
    }
}

mirror_enum!(OpsAlertKindNode as "OpsAlertKind" from audit::AlertKind { Audit, Drift });

/// A problem a person must look at: an automatic close that failed its third attempt (`AUDIT`), or inventory
/// counters that drifted from the stays they count (`DRIFT`). Shown until what raised it is resolved.
#[derive(SimpleObject)]
pub struct OpsAlertNode {
    pub kind: OpsAlertKindNode,
    pub message: String,
    pub raised_at: OffsetDateTime,
}

/// A night-audit rule the query broke, as a GraphQL error; database errors stay hidden.
fn audit_error(err: audit::AuditError) -> async_graphql::Error {
    match err {
        audit::AuditError::Database(db_err) => internal(db_err),
        audit::AuditError::Folio(folio_err) => folio_error(folio_err),
        audit::AuditError::Reservations(reservations_err) => reservations_error(reservations_err),
        audit::AuditError::Interrupted => {
            tracing::error!("the night-audit close was interrupted");
            async_graphql::Error::new("Internal error")
        }
        other => async_graphql::Error::new(other.to_string()),
    }
}

/// A folio rule the query broke, as a GraphQL error; database errors stay hidden.
fn folio_error(err: folio::FolioError) -> async_graphql::Error {
    match err {
        folio::FolioError::Database(db_err) => internal(db_err),
        other => async_graphql::Error::new(other.to_string()),
    }
}

/// Most days one `fxRates` query spans.
const MAX_FX_RATE_DAYS: i64 = 366;

/// A reservations rule the query broke, as a GraphQL error; database errors stay hidden.
fn reservations_error(err: reservations::ReservationsError) -> async_graphql::Error {
    match err {
        reservations::ReservationsError::Database(db_err) => internal(db_err),
        other => async_graphql::Error::new(other.to_string()),
    }
}

/// Whether the field being resolved selects `name`. async-graphql drops selections left out by `@skip` or
/// `@include` (on fields and fragments) before resolving, so `totalCount @include(if: false)` is not selected;
/// the unit test below keeps that true across upgrades.
fn selected(ctx: &Context<'_>, name: &str) -> async_graphql::Result<bool> {
    Ok(ctx.look_ahead().field(name).exists())
}

/// Most accounts one `accounts` query returns.
const MAX_ACCOUNT_QUERY: i64 = 100;

/// Search text is at most 100 characters.
fn check_search(text: Option<&str>) -> async_graphql::Result<()> {
    if text.is_some_and(|text| text.chars().count() > 100) {
        return Err(async_graphql::Error::new("search text is at most 100 characters"));
    }
    Ok(())
}

/// A rates rule the query broke, as a GraphQL error; database errors stay hidden.
fn rates_error(err: rates::RatesError) -> async_graphql::Error {
    match err {
        rates::RatesError::Database(db_err) => internal(db_err),
        other => async_graphql::Error::new(other.to_string()),
    }
}

/// Checks `permission` for `property` and opens a transaction in the caller's tenant.
async fn scoped(ctx: &Context<'_>, permission: Permission, property: Uuid) -> async_graphql::Result<Tx> {
    scoped_any(ctx, &[permission], property).await
}

/// Checks that the caller holds one of `permissions` for `property` and opens a transaction in their tenant.
async fn scoped_any(ctx: &Context<'_>, permissions: &[Permission], property: Uuid) -> async_graphql::Result<Tx> {
    let pool = ctx.data::<PgPool>()?;
    let tenant = ctx.data::<TenantContext>()?;
    if !permissions.iter().any(|permission| tenant.require(*permission, Some(property)).is_ok()) {
        return Err(async_graphql::Error::new("you do not have permission for this property"));
    }
    db::begin(pool, Scope::tenant(tenant.tenant)).await.map_err(internal)
}

/// `[from, to)` must span 1 to `max_days` days.
fn check_range(from: Date, to: Date, max_days: i64) -> async_graphql::Result<()> {
    if to > from && to - from <= Duration::days(max_days) {
        Ok(())
    } else {
        Err(async_graphql::Error::new(format!("the range must be 1 to {max_days} days")))
    }
}

pub struct Query;

#[Object]
impl Query {
    /// Properties the current user can see, ordered by code.
    async fn properties(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<PropertyNode>> {
        let pool = ctx.data::<PgPool>()?;
        let tenant = ctx.data::<TenantContext>()?;
        let visible = tenant.visible_properties();
        let mut tx = db::begin(pool, Scope::tenant(tenant.tenant)).await.map_err(internal)?;
        let properties = property::list_properties(&mut tx, visible.as_deref()).await.map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(properties
            .into_iter()
            .map(|p| PropertyNode {
                id: p.id,
                code: p.code,
                name: p.name,
                timezone: p.timezone,
                base_currency: p.base_currency,
                check_in_time: p.check_in_time,
                check_out_time: p.check_out_time,
                audit_time: p.audit_time,
                business_date: p.business_date,
                version: p.version,
            })
            .collect())
    }

    /// The property's room types, active or not, in display order.
    async fn room_types(&self, ctx: &Context<'_>, property_id: Uuid) -> async_graphql::Result<Vec<RoomTypeNode>> {
        let mut tx = scoped(ctx, Permission::RoomsView, property_id).await?;
        let types = rooms::list_room_types(&mut tx, property_id).await.map_err(internal)?;
        let mut nodes: Vec<RoomTypeNode> = types.into_iter().map(RoomTypeNode::from).collect();
        if selected(ctx, "media")? {
            let ids: Vec<Uuid> = nodes.iter().map(|node| node.id).collect();
            let mut galleries = media::galleries(&mut tx, &ids).await.map_err(media_error)?;
            let storage = ctx.data::<Arc<Storage>>()?;
            for node in &mut nodes {
                let gallery = galleries.remove(&node.id).unwrap_or_default();
                node.media = gallery.into_iter().map(|item| gallery_node(storage, item)).collect();
            }
        }
        tx.commit().await.map_err(internal)?;
        Ok(nodes)
    }

    /// The property's rooms, active or not, in display order; only one type's if `roomTypeId` is given.
    async fn rooms(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
        room_type_id: Option<Uuid>,
    ) -> async_graphql::Result<Vec<RoomNode>> {
        let mut tx = scoped(ctx, Permission::RoomsView, property_id).await?;
        let rooms = rooms::list_rooms(&mut tx, property_id, room_type_id).await.map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(rooms.into_iter().map(RoomNode::from).collect())
    }

    /// Housekeeping sections, by name.
    async fn sections(&self, ctx: &Context<'_>, property_id: Uuid) -> async_graphql::Result<Vec<SectionNode>> {
        let mut tx = scoped(ctx, Permission::RoomsView, property_id).await?;
        let sections = rooms::list_sections(&mut tx, property_id).await.map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(sections.into_iter().map(|s| SectionNode { id: s.id, name: s.name, version: s.version }).collect())
    }

    /// Reasons a room can be blocked for, active or not, by code.
    async fn block_reasons(&self, ctx: &Context<'_>, property_id: Uuid) -> async_graphql::Result<Vec<BlockReasonNode>> {
        let mut tx = scoped(ctx, Permission::RoomsView, property_id).await?;
        let reasons = rooms::list_block_reasons(&mut tx, property_id).await.map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(reasons
            .into_iter()
            .map(|r| BlockReasonNode {
                id: r.id,
                code: r.code,
                label: r.label,
                default_kind: r.default_kind.into(),
                active: r.active,
                version: r.version,
            })
            .collect())
    }

    /// Active room blocks overlapping `[from, to)` (at most 400 days), by start date.
    async fn blocks(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
        from: Date,
        to: Date,
    ) -> async_graphql::Result<Vec<BlockNode>> {
        check_range(from, to, 400)?;
        let mut tx = scoped(ctx, Permission::InventoryView, property_id).await?;
        let blocks = rooms::list_blocks(&mut tx, property_id, from, to).await.map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(blocks
            .into_iter()
            .map(|b| BlockNode {
                id: b.id,
                room_id: b.room_id,
                from: b.from,
                to: b.to,
                kind: b.kind.into(),
                reason_id: b.reason_id,
                note: b.note,
                version: b.version,
            })
            .collect())
    }

    /// Counts per room type per day for `[from, to)` (at most 93 days), by date then room type. Days before
    /// the business date that were never counted, and days past the 730-day window, have no rows.
    async fn inventory(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
        from: Date,
        to: Date,
    ) -> async_graphql::Result<Vec<InventoryDayNode>> {
        check_range(from, to, 93)?;
        let mut tx = scoped(ctx, Permission::InventoryView, property_id).await?;
        let days = rooms::list_inventory(&mut tx, property_id, from, to).await.map_err(internal)?;
        let types = rooms::list_room_types(&mut tx, property_id).await.map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        let overbooking: HashMap<Uuid, i32> = types.into_iter().map(|t| (t.id, t.overbooking)).collect();
        Ok(days
            .into_iter()
            .map(|d| {
                let available = d.available();
                let sellable = available + overbooking.get(&d.room_type_id).copied().unwrap_or(0);
                InventoryDayNode {
                    date: d.date,
                    room_type_id: d.room_type_id,
                    physical: d.physical,
                    sold: d.sold,
                    out_of_order: d.out_of_order,
                    available,
                    sellable,
                }
            })
            .collect())
    }

    /// The property's rate plans in tree order: each standard or custom plan, by code, followed by the plans
    /// derived from it, depth first.
    async fn rate_plans(&self, ctx: &Context<'_>, property_id: Uuid) -> async_graphql::Result<Vec<RatePlanNode>> {
        let mut tx = scoped(ctx, Permission::RatesView, property_id).await?;
        let plans = rates::list_rate_plans(&mut tx, property_id).await.map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(plans.into_iter().map(RatePlanNode::from).collect())
    }

    /// A plan's prices and restrictions for `[from, to)` (at most 93 days), by date, room type and occupancy.
    async fn rate_grid(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
        rate_plan_id: Uuid,
        from: Date,
        to: Date,
    ) -> async_graphql::Result<RateGridNode> {
        check_range(from, to, 93)?;
        let mut tx = scoped(ctx, Permission::RatesView, property_id).await?;
        let prices = rates::list_prices(&mut tx, property_id, rate_plan_id, from, to).await.map_err(internal)?;
        let restrictions =
            rates::list_restrictions(&mut tx, property_id, rate_plan_id, from, to).await.map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(RateGridNode {
            prices: prices
                .into_iter()
                .map(|p| RatePriceNode {
                    room_type_id: p.room_type_id,
                    date: p.date,
                    occupancy: p.occupancy,
                    amount: p.amount,
                })
                .collect(),
            restrictions: restrictions
                .into_iter()
                .map(|r| RestrictionNode {
                    room_type_id: r.room_type_id,
                    date: r.date,
                    closed: r.closed,
                    min_stay: r.min_stay,
                    max_stay: r.max_stay,
                    closed_to_arrival: r.closed_to_arrival,
                    closed_to_departure: r.closed_to_departure,
                })
                .collect(),
        })
    }

    /// What a bulk change would do to a standard or custom plan's prices on `[from, to)`, without doing it.
    /// Left out, `weekdays` (ISO: 1 = Monday), `roomTypeIds` and `occupancies` mean all.
    #[allow(clippy::too_many_arguments)]
    async fn bulk_change_preview(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
        rate_plan_id: Uuid,
        from: Date,
        to: Date,
        #[graphql(default)] weekdays: Vec<u8>,
        #[graphql(default)] room_type_ids: Vec<Uuid>,
        #[graphql(default)] occupancies: Vec<i32>,
        mode: PriceChangeModeNode,
        value: i64,
    ) -> async_graphql::Result<BulkPreviewNode> {
        if weekdays.iter().any(|day| !(1..=7).contains(day)) {
            return Err(async_graphql::Error::new("weekdays are 1 (Monday) to 7 (Sunday)"));
        }
        check_range(from, to, 366)?;
        let change = rates::BulkChange {
            from,
            to,
            weekdays: weekdays.iter().map(|day| time::Weekday::Monday.nth_next(day - 1)).collect(),
            room_type_ids,
            occupancies,
            change: rates::PriceChange { mode: mode.into(), value },
        };
        let mut tx = scoped(ctx, Permission::RatesView, property_id).await?;
        let preview =
            rates::preview_bulk_change(&mut tx, property_id, rate_plan_id, &change, 50).await.map_err(rates_error)?;
        tx.commit().await.map_err(internal)?;
        Ok(BulkPreviewNode {
            total: preview.total,
            cells: preview
                .cells
                .into_iter()
                .map(|c| PriceChangeCellNode {
                    room_type_id: c.room_type_id,
                    date: c.date,
                    occupancy: c.occupancy,
                    before: c.before,
                    after: c.after,
                })
                .collect(),
        })
    }

    /// Meal supplements by currency, meal plan and start.
    async fn meal_supplements(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
    ) -> async_graphql::Result<Vec<MealSupplementNode>> {
        let mut tx = scoped(ctx, Permission::RatesView, property_id).await?;
        let supplements = rates::list_meal_supplements(&mut tx, property_id).await.map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(supplements
            .into_iter()
            .map(|s| MealSupplementNode {
                id: s.id,
                meal_plan: s.meal_plan.into(),
                currency: s.currency,
                adult_amount: s.adult_amount,
                child_amount: s.child_amount,
                from: s.from,
                to: s.to,
                version: s.version,
            })
            .collect())
    }

    /// Cancellation policies by name.
    async fn cancellation_policies(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
    ) -> async_graphql::Result<Vec<CancellationPolicyNode>> {
        let mut tx = scoped(ctx, Permission::RatesView, property_id).await?;
        let policies = rates::list_cancellation_policies(&mut tx, property_id).await.map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(policies
            .into_iter()
            .map(|p| CancellationPolicyNode {
                id: p.id,
                name: p.name,
                rules: p.rules.into_iter().map(CancellationRuleNode::from).collect(),
                no_show: p.no_show.into(),
                version: p.version,
            })
            .collect())
    }

    /// Prices a stay of `[checkIn, checkOut)` (at most 90 nights) and lists every reason it cannot be sold.
    #[allow(clippy::too_many_arguments)]
    async fn quote(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
        room_type_id: Uuid,
        rate_plan_id: Uuid,
        meal_plan: MealPlanNode,
        check_in: Date,
        check_out: Date,
        adults: i32,
        children: i32,
        residency: ResidencyNode,
    ) -> async_graphql::Result<QuoteNode> {
        if check_out > check_in && check_out - check_in > Duration::days(90) {
            return Err(async_graphql::Error::new("a quote is for at most 90 nights"));
        }
        let request = rates::QuoteRequest {
            room_type_id,
            rate_plan_id,
            meal_plan: meal_plan.into(),
            check_in,
            check_out,
            adults,
            children,
            residency: residency.into(),
        };
        let mut tx = scoped(ctx, Permission::RatesView, property_id).await?;
        let quote = rates::load_quote(&mut tx, property_id, &request).await.map_err(rates_error)?;
        tx.commit().await.map_err(internal)?;
        Ok(QuoteNode {
            nights: quote.nights.into_iter().map(QuoteNightNode::from).collect(),
            total: quote.total,
            currency: quote.currency,
            restrictions_ok: quote.restrictions_ok,
            violations: quote.violations.into_iter().map(ViolationNode::from).collect(),
        })
    }

    /// Every active room type, in display order, with its free rooms and every offer for a stay of
    /// `[checkIn, checkOut)` (at most 30 nights) inside the booking window.
    #[allow(clippy::too_many_arguments)]
    async fn availability(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
        check_in: Date,
        check_out: Date,
        adults: i32,
        children: i32,
        residency: ResidencyNode,
    ) -> async_graphql::Result<Vec<RoomTypeAvailabilityNode>> {
        check_range(check_in, check_out, reservations::MAX_AVAILABILITY_NIGHTS)?;
        let request =
            reservations::AvailabilityRequest { check_in, check_out, adults, children, residency: residency.into() };
        let mut tx = scoped(ctx, Permission::ReservationsView, property_id).await?;
        let types = reservations::availability(&mut tx, property_id, &request).await.map_err(reservations_error)?;
        tx.commit().await.map_err(internal)?;
        Ok(types
            .into_iter()
            .map(|t| RoomTypeAvailabilityNode {
                room_type_id: t.room_type_id,
                code: t.code,
                name: t.name,
                free: t.free,
                offers: t
                    .offers
                    .into_iter()
                    .map(|o| OfferNode {
                        rate_plan_id: o.rate_plan_id,
                        rate_plan_code: o.rate_plan_code,
                        meal_plan: o.meal_plan.into(),
                        total: o.quote.total,
                        currency: o.quote.currency,
                        restrictions_ok: o.quote.restrictions_ok,
                        violations: o.quote.violations.into_iter().map(ViolationNode::from).collect(),
                        nights: o.quote.nights.into_iter().map(QuoteNightNode::from).collect(),
                    })
                    .collect(),
            })
            .collect())
    }

    /// The property's reservation rooms, one node per room, `first` (1 to 100) at a time after the cursor
    /// `after`. Sorted by arrival unless `sort` says otherwise; a cursor works only under the sort it came from.
    async fn reservations(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
        filter: Option<ReservationFilterInput>,
        sort: Option<ReservationSortInput>,
        #[graphql(desc = "Rows per page; 50 when left out or null.")] first: Option<i64>,
        after: Option<String>,
    ) -> async_graphql::Result<ReservationRoomConnection> {
        let first = first.unwrap_or(50);
        if !(1..=reservations::MAX_PAGE_SIZE).contains(&first) {
            return Err(async_graphql::Error::new(format!("first is 1 to {}", reservations::MAX_PAGE_SIZE)));
        }
        let filter = filter.map_or_else(reservations::ListFilter::default, |f| reservations::ListFilter {
            arrival_from: f.arrival_from,
            arrival_to: f.arrival_to,
            statuses: f.statuses.map(|statuses| statuses.into_iter().map(Into::into).collect()),
            sources: f.sources.map(|sources| sources.into_iter().map(Into::into).collect()),
            text: f.text,
        });
        check_search(filter.text.as_deref())?;
        let request = reservations::ListRequest {
            filter,
            sort: sort.map_or_else(reservations::Sort::default, |s| reservations::Sort {
                field: s.field.into(),
                direction: s.direction.into(),
            }),
            first,
            after,
            count: selected(ctx, "totalCount")?,
        };
        let mut tx = scoped(ctx, Permission::ReservationsView, property_id).await?;
        let page =
            reservations::list_reservation_rooms(&mut tx, property_id, &request).await.map_err(reservations_error)?;
        tx.commit().await.map_err(internal)?;
        Ok(ReservationRoomConnection {
            nodes: page
                .rows
                .into_iter()
                .map(|r| ReservationRoomRowNode {
                    id: r.id,
                    reservation_id: r.reservation_id,
                    confirmation_no: r.confirmation_no,
                    guest_name: r.guest_name,
                    arrival: r.arrival,
                    departure: r.departure,
                    nights: r.nights,
                    room_type_code: r.room_type_code,
                    room_number: r.room_number,
                    status: r.status.into(),
                    source: r.source.into(),
                    total: r.total,
                    currency: r.currency,
                    version: r.version,
                    account_name: r.account_name,
                })
                .collect(),
            page_info: PageInfo { end_cursor: page.end_cursor, has_next_page: page.has_next_page },
            total_count: page.total_count.unwrap_or(0),
        })
    }

    /// One reservation with its rooms and, newest first, its history.
    async fn reservation(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
        id: Uuid,
    ) -> async_graphql::Result<ReservationNode> {
        let mut tx = scoped(ctx, Permission::ReservationsView, property_id).await?;
        let detail = reservations::get_reservation(&mut tx, property_id, id).await.map_err(reservations_error)?;
        let history = if selected(ctx, "history")? {
            reservations::reservation_history(&mut tx, property_id, id).await.map_err(internal)?
        } else {
            Vec::new()
        };
        tx.commit().await.map_err(internal)?;
        Ok(ReservationNode::new(detail, history))
    }

    /// Up to `first` (1 to 50) of the tenant's guests whose name is like `search`, typos included, or whose
    /// email or phone is exactly `search`, closest first; without `search`, the newest guests.
    async fn guests(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
        search: Option<String>,
        #[graphql(desc = "20 when left out or null.")] first: Option<i64>,
    ) -> async_graphql::Result<Vec<GuestNode>> {
        let first = first.unwrap_or(20);
        if !(1..=reservations::MAX_GUEST_SEARCH).contains(&first) {
            return Err(async_graphql::Error::new(format!("first is 1 to {}", reservations::MAX_GUEST_SEARCH)));
        }
        check_search(search.as_deref())?;
        let mut tx = scoped(ctx, Permission::ReservationsView, property_id).await?;
        // Guests belong to the tenant: reach them only through one of its properties.
        let property = property::list_properties(&mut tx, Some(&[property_id])).await.map_err(internal)?;
        let guests = if property.is_empty() {
            Vec::new()
        } else {
            reservations::search_guests(&mut tx, search.as_deref().unwrap_or(""), first).await.map_err(internal)?
        };
        tx.commit().await.map_err(internal)?;
        Ok(guests.into_iter().map(GuestNode::from).collect())
    }

    /// Up to `first` (1 to 100) of the tenant's accounts whose name contains `search` (case-insensitive), by
    /// name; inactive accounts included only when `includeInactive`.
    async fn accounts(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
        search: Option<String>,
        #[graphql(default)] include_inactive: bool,
        #[graphql(desc = "100 when left out or null.")] first: Option<i64>,
    ) -> async_graphql::Result<Vec<AccountNode>> {
        let first = first.unwrap_or(MAX_ACCOUNT_QUERY);
        if !(1..=MAX_ACCOUNT_QUERY).contains(&first) {
            return Err(async_graphql::Error::new(format!("first is 1 to {MAX_ACCOUNT_QUERY}")));
        }
        check_search(search.as_deref())?;
        let mut tx = scoped(ctx, Permission::ReservationsView, property_id).await?;
        // Accounts belong to the tenant, like guests: reach them only through one of its properties.
        let property = property::list_properties(&mut tx, Some(&[property_id])).await.map_err(internal)?;
        let accounts = if property.is_empty() {
            Vec::new()
        } else {
            reservations::list_accounts(&mut tx, search.as_deref().unwrap_or(""), include_inactive, first)
                .await
                .map_err(internal)?
        };
        tx.commit().await.map_err(internal)?;
        Ok(accounts.into_iter().map(AccountNode::from).collect())
    }

    /// Active rooms of the type that no stay holds and no block covers on any night of `[checkIn, checkOut)`
    /// (at most the 730-night counter window), in display order: the rooms a stay on those nights could be
    /// assigned.
    async fn free_rooms(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
        room_type_id: Uuid,
        check_in: Date,
        check_out: Date,
    ) -> async_graphql::Result<Vec<FreeRoomNode>> {
        check_range(check_in, check_out, rooms::WINDOW_DAYS)?;
        let mut tx = scoped(ctx, Permission::ReservationsView, property_id).await?;
        let rooms = reservations::free_rooms(&mut tx, property_id, room_type_id, check_in, check_out)
            .await
            .map_err(reservations_error)?;
        tx.commit().await.map_err(internal)?;
        Ok(rooms.into_iter().map(|r| FreeRoomNode { id: r.id, number: r.number, section: r.section }).collect())
    }

    /// The stays and blocks of up to 10 rooms of the property that overlap `[from, to)` (at most 42 days),
    /// for the tape chart.
    async fn tape_window(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
        room_ids: Vec<Uuid>,
        from: Date,
        to: Date,
    ) -> async_graphql::Result<TapeWindowNode> {
        let mut tx = scoped(ctx, Permission::ReservationsView, property_id).await?;
        let window =
            reservations::tape_window(&mut tx, property_id, &room_ids, from, to).await.map_err(reservations_error)?;
        tx.commit().await.map_err(internal)?;
        Ok(TapeWindowNode {
            stays: window
                .stays
                .into_iter()
                .map(|s| TapeStayNode {
                    id: s.id,
                    reservation_id: s.reservation_id,
                    room_id: s.room_id,
                    room_type_id: s.room_type_id,
                    start: s.start,
                    end: s.end,
                    status: s.status.into(),
                    guest_name: s.guest_name,
                    account_name: s.account_name,
                    version: s.version,
                })
                .collect(),
            blocks: window
                .blocks
                .into_iter()
                .map(|b| TapeBlockNode { id: b.id, room_id: b.room_id, start: b.start, end: b.end, reason: b.reason })
                .collect(),
        })
    }

    /// Confirmed stays with no room that overlap `[from, to)` (at most 42 days), by arrival, each with why it
    /// has none.
    async fn unassigned_stays(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
        from: Date,
        to: Date,
    ) -> async_graphql::Result<Vec<UnassignedStayNode>> {
        let mut tx = scoped(ctx, Permission::ReservationsView, property_id).await?;
        let stays = reservations::unassigned_stays(&mut tx, property_id, from, to).await.map_err(reservations_error)?;
        tx.commit().await.map_err(internal)?;
        Ok(stays
            .into_iter()
            .map(|s| UnassignedStayNode {
                id: s.id,
                reservation_id: s.reservation_id,
                room_type_id: s.room_type_id,
                start: s.start,
                end: s.end,
                status: RoomStatusNode::Confirmed,
                guest_name: s.guest_name,
                reason: s.reason.into(),
                version: s.version,
            })
            .collect())
    }

    /// Every active room of the property with its condition, occupancy, assignee, open issues and block for the
    /// business date, in rail order.
    async fn housekeeping_board(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
    ) -> async_graphql::Result<Vec<BoardRoomNode>> {
        let mut tx = scoped(ctx, Permission::HousekeepingView, property_id).await?;
        let rooms = housekeeping::board(&mut tx, property_id).await.map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(rooms.into_iter().map(BoardRoomNode::from).collect())
    }

    /// The signed-in user's rooms for the business date: turnovers, departures, stay-overs, arrivals, then
    /// empty rooms, each group in room order.
    async fn my_rooms(&self, ctx: &Context<'_>, property_id: Uuid) -> async_graphql::Result<Vec<BoardRoomNode>> {
        let mut tx = scoped(ctx, Permission::HousekeepingView, property_id).await?;
        let user = ctx.data::<TenantContext>()?.user;
        let rooms = housekeeping::my_rooms(&mut tx, property_id, user).await.map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(rooms.into_iter().map(BoardRoomNode::from).collect())
    }

    /// The property's room issues, newest first, at most 500; only those with `status` if given.
    async fn room_issues(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
        status: Option<IssueStatusNode>,
    ) -> async_graphql::Result<Vec<RoomIssueNode>> {
        let mut tx = scoped(ctx, Permission::HousekeepingView, property_id).await?;
        let issues = housekeeping::list_issues(&mut tx, property_id, status.map(Into::into)).await.map_err(internal)?;
        let mut nodes: Vec<RoomIssueNode> = issues.into_iter().map(RoomIssueNode::from).collect();
        if selected(ctx, "photos")? {
            // One query for every issue's photos.
            let ids: Vec<Uuid> = nodes.iter().map(|node| node.id).collect();
            let mut photos = media::issue_photos(&mut tx, &ids).await.map_err(media_error)?;
            let storage = ctx.data::<Arc<Storage>>()?;
            for node in &mut nodes {
                let assets = photos.remove(&node.id).unwrap_or_default();
                node.photos = assets.into_iter().map(|asset| issue_photo_node(storage, asset)).collect();
            }
        }
        tx.commit().await.map_err(internal)?;
        Ok(nodes)
    }

    /// Users who hold a housekeeping role at the property and can be assigned rooms, by name.
    async fn housekeepers(&self, ctx: &Context<'_>, property_id: Uuid) -> async_graphql::Result<Vec<HousekeeperNode>> {
        let mut tx = scoped(ctx, Permission::HousekeepingView, property_id).await?;
        let housekeepers = housekeeping::housekeepers(&mut tx, property_id).await.map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(housekeepers.into_iter().map(HousekeeperNode::from).collect())
    }

    /// The guest's ID documents uploaded at the property, newest first, at most 50. Needs `GuestDocumentsManage` at
    /// the property. Links to the files come only from the audited REST call.
    async fn guest_documents(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
        guest_id: Uuid,
    ) -> async_graphql::Result<Vec<GuestDocumentNode>> {
        let mut tx = scoped(ctx, Permission::GuestDocumentsManage, property_id).await?;
        let documents = media::guest_documents(&mut tx, property_id, guest_id).await.map_err(media_error)?;
        tx.commit().await.map_err(internal)?;
        Ok(documents.into_iter().map(GuestDocumentNode::from).collect())
    }

    /// The folios of one reservation, stay or account in the property; name exactly one of `reservationId`,
    /// `reservationRoomId` and `accountId`. A reservation's are its deposit folio, each stay's guest folio and its
    /// account's current folio (the open one, else the newest), in that order. An account folio is the account's city
    /// ledger in the property, charges from every reservation billed to it included, as any staff member with
    /// `FolioView` may see; once invoiced, the account's next charge opens a fresh one. Read through a reservation it
    /// comes with its balances and invoices but no lines or payments (`linesOmitted`); a read by `accountId` gives
    /// every folio of the account, the open one first, then the invoiced ones newest first, each with its lines. Every
    /// folio gives at most its newest 500 lines and newest 500 payments (`truncated` when it has more); balances always
    /// cover all of them. Lines (with their taxes), payments, balances and invoices of every folio are read at once,
    /// whatever the number of folios.
    async fn folios(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
        reservation_id: Option<Uuid>,
        reservation_room_id: Option<Uuid>,
        account_id: Option<Uuid>,
    ) -> async_graphql::Result<Vec<FolioNode>> {
        let query = match (reservation_id, reservation_room_id, account_id) {
            (Some(reservation), None, None) => folio::FolioQuery::Reservation { property: property_id, reservation },
            (None, Some(reservation_room), None) => {
                folio::FolioQuery::ReservationRoom { property: property_id, reservation_room }
            }
            (None, None, Some(account)) => folio::FolioQuery::Account { property: property_id, account },
            _ => {
                return Err(async_graphql::Error::new(
                    "name exactly one of reservationId, reservationRoomId and accountId",
                ));
            }
        };
        let mut tx = scoped(ctx, Permission::FolioView, property_id).await?;
        let views = folio::folios_for(&mut tx, query).await.map_err(folio_error)?;
        let mut nodes = views.into_iter().map(FolioNode::try_from).collect::<async_graphql::Result<Vec<_>>>()?;
        if selected(ctx, "invoices")? && !nodes.is_empty() {
            let ids: Vec<Uuid> = nodes.iter().map(|node| node.id).collect();
            let mut by_folio: HashMap<Uuid, Vec<InvoiceNode>> = HashMap::new();
            for invoice in folio::invoice_summaries(&mut tx, &ids).await.map_err(internal)? {
                by_folio.entry(invoice.folio_id).or_default().push(invoice.into());
            }
            for node in &mut nodes {
                node.invoices = by_folio.remove(&node.id).unwrap_or_default();
            }
        }
        tx.commit().await.map_err(internal)?;
        Ok(nodes)
    }

    /// The categories of the reservation's stays that its account pays (its routing), sorted; empty when it has no
    /// account or routes nothing. Charges of every other category go to the stays' guest folios.
    async fn routing(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
        reservation_id: Uuid,
    ) -> async_graphql::Result<Vec<String>> {
        let mut tx = scoped(ctx, Permission::FolioView, property_id).await?;
        let categories = folio::routing(&mut tx, property_id, reservation_id).await.map_err(folio_error)?;
        tx.commit().await.map_err(internal)?;
        Ok(categories)
    }

    /// The property's tax rules, ended or not, in calculation order then by start date.
    async fn tax_rules(&self, ctx: &Context<'_>, property_id: Uuid) -> async_graphql::Result<Vec<TaxRuleNode>> {
        let mut tx = scoped(ctx, Permission::FolioView, property_id).await?;
        let rules = folio::list_tax_rules(&mut tx, property_id).await.map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(rules.into_iter().map(TaxRuleNode::from).collect())
    }

    /// The property's POS outlets, active or not, by code.
    async fn outlets(&self, ctx: &Context<'_>, property_id: Uuid) -> async_graphql::Result<Vec<OutletNode>> {
        let mut tx = scoped(ctx, Permission::FolioView, property_id).await?;
        let outlets = folio::list_outlets(&mut tx, property_id).await.map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(outlets.into_iter().map(OutletNode::from).collect())
    }

    /// The property's stored exchange rates dated in `[from, to)` (1 to 366 days), by date then currency. A rate is
    /// in force from its date until the next one.
    async fn fx_rates(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
        from: Date,
        to: Date,
    ) -> async_graphql::Result<Vec<FxRateNode>> {
        check_range(from, to, MAX_FX_RATE_DAYS)?;
        let mut tx = scoped(ctx, Permission::FolioView, property_id).await?;
        let rates = fx::list_rates(&mut tx, property_id, from, to).await.map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(rates.into_iter().map(FxRateNode::from).collect())
    }

    /// Why the last exchange-rate import left the property without complete, current rates; `null` when all is well.
    async fn fx_alert(&self, ctx: &Context<'_>, property_id: Uuid) -> async_graphql::Result<Option<FxAlertNode>> {
        let mut tx = scoped(ctx, Permission::FolioView, property_id).await?;
        let alert = fx::alert(&mut tx, property_id).await.map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(alert.map(|alert| FxAlertNode { message: alert.message, raised_at: alert.raised_at }))
    }

    /// The property's tax settings: VAT registration, TIN, the small-turnover levy band, the invoice name and
    /// address, and the accepted currencies.
    async fn property_tax_settings(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
    ) -> async_graphql::Result<TaxSettingsNode> {
        let mut tx = scoped(ctx, Permission::FolioView, property_id).await?;
        let settings = property::tax_settings(&mut tx, property_id).await.map_err(|err| match err {
            property::PropertyError::Database(db_err) => internal(db_err),
            other => async_graphql::Error::new(other.to_string()),
        })?;
        tx.commit().await.map_err(internal)?;
        Ok(settings.into())
    }

    /// The review of the property's business date: the arrivals not checked in, the departures not checked out, the
    /// day's POS bills by outlet, the booking bills and the payments, with how many stays block the close.
    async fn night_audit(&self, ctx: &Context<'_>, property_id: Uuid) -> async_graphql::Result<NightAuditNode> {
        let mut tx = scoped(ctx, Permission::NightAudit, property_id).await?;
        let review = audit::night_audit(&mut tx, property_id).await.map_err(audit_error)?;
        tx.commit().await.map_err(internal)?;
        Ok(review.into())
    }

    /// The property's closes, newest business date first, each with its steps: `first` of them (1 to 100) dated
    /// before `after`, or the newest when `after` is left out.
    async fn audit_runs(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
        #[graphql(default = 20)] first: i32,
        after: Option<Date>,
    ) -> async_graphql::Result<Vec<AuditRunNode>> {
        let first = u32::try_from(first).map_err(|_| async_graphql::Error::new("first is 1 to 100"))?;
        let mut tx = scoped_any(ctx, &[Permission::NightAudit, Permission::FolioView], property_id).await?;
        let runs = audit::list_runs(&mut tx, property_id, first, after).await.map_err(audit_error)?;
        tx.commit().await.map_err(internal)?;
        Ok(runs.into_iter().map(AuditRunNode::from).collect())
    }

    /// The statistics of the property's closed business dates from `from` to `to`, both included (at most 366 days),
    /// by date, each with its breakdowns. A day not yet closed has none.
    async fn daily_stats(
        &self,
        ctx: &Context<'_>,
        property_id: Uuid,
        from: Date,
        to: Date,
    ) -> async_graphql::Result<Vec<DailyStatsNode>> {
        let mut tx = scoped_any(ctx, &[Permission::NightAudit, Permission::FolioView], property_id).await?;
        let days = audit::daily_stats(&mut tx, property_id, from, to).await.map_err(audit_error)?;
        tx.commit().await.map_err(internal)?;
        Ok(days.into_iter().map(DailyStatsNode::from).collect())
    }

    /// The property's raised ops alerts, by kind; empty when all is well.
    async fn ops_alerts(&self, ctx: &Context<'_>, property_id: Uuid) -> async_graphql::Result<Vec<OpsAlertNode>> {
        let mut tx = scoped(ctx, Permission::PropertiesManage, property_id).await?;
        let alerts = audit::alerts::list(&mut tx, property_id).await.map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(alerts
            .into_iter()
            .map(|a| OpsAlertNode { kind: a.kind.into(), message: a.message, raised_at: a.raised_at })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::{audit_error, internal, selected};
    use async_graphql::{Context, EmptyMutation, EmptySubscription, Object, Request, Schema, Variables};
    use serde_json::json;

    #[test]
    fn database_errors_reach_clients_as_a_bare_internal_error() {
        let err = internal(sqlx::Error::Protocol("relation \"property\" does not exist".into()));

        assert_eq!(err.message, "Internal error");
    }

    #[test]
    fn night_audit_errors_hide_database_text_however_nested() {
        let leak = || sqlx::Error::Protocol("relation \"audit_run\" does not exist".into());
        for err in [
            audit::AuditError::Database(leak()),
            audit::AuditError::Folio(folio::FolioError::Database(leak())),
            audit::AuditError::Reservations(reservations::ReservationsError::Database(leak())),
            audit::AuditError::Interrupted,
        ] {
            assert_eq!(audit_error(err).message, "Internal error");
        }
        let refused = audit_error(audit::AuditError::Reservations(reservations::ReservationsError::Conflict(
            "Room 101 is taken".into(),
        )));
        assert_eq!(refused.message, "Room 101 is taken");
    }

    struct Probe(bool);

    #[Object]
    impl Probe {
        async fn total(&self) -> i32 {
            0
        }

        /// Whether the parent resolver saw `total` as selected.
        async fn counted(&self) -> bool {
            self.0
        }
    }

    struct ProbeQuery;

    #[Object]
    impl ProbeQuery {
        async fn probe(&self, ctx: &Context<'_>) -> async_graphql::Result<Probe> {
            Ok(Probe(selected(ctx, "total")?))
        }
    }

    async fn counted(query: &str, variables: serde_json::Value) -> bool {
        let schema = Schema::new(ProbeQuery, EmptyMutation, EmptySubscription);
        let response = schema.execute(Request::new(query).variables(Variables::from_json(variables))).await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        response.data.into_json().unwrap()["probe"]["counted"].as_bool().unwrap()
    }

    #[tokio::test]
    async fn a_field_left_out_by_skip_or_include_is_not_selected() {
        let with = "query($with: Boolean!) { probe { counted total @include(if: $with) } }";
        let without = "query($without: Boolean!) { probe { counted total @skip(if: $without) } }";

        assert!(counted("{ probe { counted total } }", json!({})).await);
        assert!(!counted("{ probe { counted } }", json!({})).await);
        assert!(counted(with, json!({"with": true})).await);
        assert!(!counted(with, json!({"with": false})).await);
        assert!(counted(without, json!({"without": false})).await);
        assert!(!counted(without, json!({"without": true})).await);
        // Selected twice, once left out: still selected.
        assert!(counted("{ probe { counted total @include(if: false) again: total } }", json!({})).await);
    }
}
