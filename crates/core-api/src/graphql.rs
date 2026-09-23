//! Read-only GraphQL API. All writes go through REST commands.

use crate::auth::TenantContext;
use crate::error::ApiError;
use crate::state::AppState;
use async_graphql::{Context, EmptyMutation, EmptySubscription, Object, Schema, SimpleObject};
use async_graphql_axum::rejection::GraphQLRejection;
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
