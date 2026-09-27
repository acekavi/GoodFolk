//! Read-only GraphQL API. All writes go through REST commands.

use crate::auth::TenantContext;
use crate::error::ApiError;
use crate::state::AppState;
use async_graphql::{Context, EmptyMutation, EmptySubscription, Enum, InputObject, Json, Object, Schema, SimpleObject};
use async_graphql_axum::rejection::GraphQLRejection;
use async_graphql_axum::{GraphQLRequest, GraphQLResponse};
use axum::extract::State;
use db::{Scope, Tx};
use identity::Permission;
use sqlx::PgPool;
use time::{Date, Duration, OffsetDateTime};
use uuid::Uuid;

pub type GqlSchema = Schema<Query, EmptyMutation, EmptySubscription>;

pub fn build_schema(production: bool) -> GqlSchema {
    let builder = Schema::build(Query, EmptyMutation, EmptySubscription).limit_depth(8).limit_complexity(500);
    if production { builder.disable_introspection().finish() } else { builder.finish() }
}

pub async fn handler(
    State(state): State<AppState>,
    ctx: TenantContext,
    request: Result<GraphQLRequest, GraphQLRejection>,
) -> Result<GraphQLResponse, ApiError> {
    let request = request.map_err(|rejection| ApiError::bad_request(rejection.0.to_string()))?;
    Ok(state.schema.execute(request.into_inner().data(state.pool.clone()).data(ctx)).await.into())
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
    pub beds: Vec<BedNode>,
    pub amenities: Vec<String>,
    pub sort_order: i32,
    pub active: bool,
    pub version: i32,
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
            beds: t.bed_config.into_iter().map(|bed| BedNode { kind: bed.kind, count: bed.count }).collect(),
            amenities: t.amenities,
            sort_order: t.sort_order,
            active: t.active,
            version: t.version,
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

/// One room type on one day. `available = physical - sold - outOfOrder`.
#[derive(SimpleObject)]
pub struct InventoryDayNode {
    pub date: Date,
    pub room_type_id: Uuid,
    pub physical: i32,
    pub sold: i32,
    pub out_of_order: i32,
    pub available: i32,
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

/// A reservations rule the query broke, as a GraphQL error; database errors stay hidden.
fn reservations_error(err: reservations::ReservationsError) -> async_graphql::Error {
    match err {
        reservations::ReservationsError::Database(db_err) => internal(db_err),
        other => async_graphql::Error::new(other.to_string()),
    }
}

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
    let pool = ctx.data::<PgPool>()?;
    let tenant = ctx.data::<TenantContext>()?;
    tenant
        .require(permission, Some(property))
        .map_err(|_| async_graphql::Error::new("you do not have permission for this property"))?;
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
                business_date: p.business_date,
                version: p.version,
            })
            .collect())
    }

    /// The property's room types, active or not, in display order.
    async fn room_types(&self, ctx: &Context<'_>, property_id: Uuid) -> async_graphql::Result<Vec<RoomTypeNode>> {
        let mut tx = scoped(ctx, Permission::RoomsView, property_id).await?;
        let types = rooms::list_room_types(&mut tx, property_id).await.map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(types.into_iter().map(RoomTypeNode::from).collect())
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
        tx.commit().await.map_err(internal)?;
        Ok(days
            .into_iter()
            .map(|d| InventoryDayNode {
                date: d.date,
                room_type_id: d.room_type_id,
                physical: d.physical,
                sold: d.sold,
                out_of_order: d.out_of_order,
                available: d.available(),
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
            count: ctx.look_ahead().field("totalCount").exists(),
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
        let history = if ctx.look_ahead().field("history").exists() {
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
}

#[cfg(test)]
mod tests {
    use super::internal;

    #[test]
    fn database_errors_reach_clients_as_a_bare_internal_error() {
        let err = internal(sqlx::Error::Protocol("relation \"property\" does not exist".into()));

        assert_eq!(err.message, "Internal error");
    }
}
