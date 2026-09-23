//! Read-only GraphQL API. All writes go through REST commands.

use crate::auth::TenantContext;
use crate::state::AppState;
use async_graphql::{Context, EmptyMutation, EmptySubscription, Object, Schema, SimpleObject};
use async_graphql_axum::{GraphQLRequest, GraphQLResponse};
use axum::extract::State;
use db::Scope;
use sqlx::PgPool;
use uuid::Uuid;

pub type GqlSchema = Schema<Query, EmptyMutation, EmptySubscription>;

pub fn build_schema(production: bool) -> GqlSchema {
    let builder = Schema::build(Query, EmptyMutation, EmptySubscription).limit_depth(8).limit_complexity(500);
    if production { builder.disable_introspection().finish() } else { builder.finish() }
}

pub async fn handler(State(state): State<AppState>, ctx: TenantContext, request: GraphQLRequest) -> GraphQLResponse {
    state.schema.execute(request.into_inner().data(state.pool.clone()).data(ctx)).await.into()
}

#[derive(SimpleObject)]
pub struct PropertyNode {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub timezone: String,
    pub base_currency: String,
}

pub struct Query;

#[Object]
impl Query {
    /// Properties the current user can see, ordered by code.
    async fn properties(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<PropertyNode>> {
        let pool = ctx.data::<PgPool>()?;
        let tenant = ctx.data::<TenantContext>()?;
        let visible = tenant.visible_properties();
        let mut tx = db::begin(pool, Scope::tenant(tenant.tenant)).await?;
        let properties = property::list_properties(&mut tx, visible.as_deref()).await?;
        tx.commit().await?;
        Ok(properties
            .into_iter()
            .map(|p| PropertyNode {
                id: p.id,
                code: p.code,
                name: p.name,
                timezone: p.timezone,
                base_currency: p.base_currency,
            })
            .collect())
    }
}
