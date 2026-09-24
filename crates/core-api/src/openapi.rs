use crate::routes::{CreatePropertyRequest, LoginRequest, SignupRequest, SwitchTenantRequest, UpdatePropertyRequest};
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    info(title = "GoodFolk PMS API", version = "1"),
    paths(
        crate::routes::auth::signup,
        crate::routes::auth::login,
        crate::routes::auth::logout,
        crate::routes::auth::me,
        crate::routes::auth::switch_tenant,
        crate::routes::properties::create,
        crate::routes::properties::update,
    ),
    components(schemas(
        SignupRequest,
        LoginRequest,
        SwitchTenantRequest,
        CreatePropertyRequest,
        UpdatePropertyRequest,
        identity::Profile,
        identity::TenantSummary,
        identity::Grant,
        identity::Role,
        property::Property,
    ))
)]
pub struct ApiDoc;
