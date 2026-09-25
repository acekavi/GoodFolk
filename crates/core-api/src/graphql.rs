//! Read-only GraphQL API. All writes go through REST commands.

use crate::auth::TenantContext;
use crate::error::ApiError;
use crate::state::AppState;
use async_graphql::{Context, EmptyMutation, EmptySubscription, Enum, Object, Schema, SimpleObject};
use async_graphql_axum::rejection::GraphQLRejection;
use async_graphql_axum::{GraphQLRequest, GraphQLResponse};
use axum::extract::State;
use db::{Scope, Tx};
use identity::Permission;
use sqlx::PgPool;
use time::{Date, Duration};
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
