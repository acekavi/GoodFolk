use crate::routes::{
    BedRequest, CreatePropertyRequest, CreateRoomRangeRequest, CreateRoomRequest, CreateRoomTypeRequest, LoginRequest,
    ReorderRequest, SectionRequest, SignupRequest, SwitchTenantRequest, UpdatePropertyRequest, UpdateRoomRequest,
    UpdateRoomTypeRequest,
};
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
        crate::routes::room_types::create,
        crate::routes::room_types::update,
        crate::routes::room_types::reorder,
        crate::routes::rooms::create,
        crate::routes::rooms::create_range,
        crate::routes::rooms::update,
        crate::routes::rooms::reorder,
        crate::routes::rooms::create_section,
        crate::routes::rooms::rename_section,
    ),
    components(schemas(
        SignupRequest,
        LoginRequest,
        SwitchTenantRequest,
        CreatePropertyRequest,
        UpdatePropertyRequest,
        BedRequest,
        CreateRoomTypeRequest,
        UpdateRoomTypeRequest,
        ReorderRequest,
        CreateRoomRequest,
        CreateRoomRangeRequest,
        UpdateRoomRequest,
        SectionRequest,
        identity::Profile,
        identity::TenantSummary,
        identity::Grant,
        identity::Role,
        property::Property,
        rooms::Bed,
        rooms::RoomType,
        rooms::Room,
        rooms::Section,
    ))
)]
pub struct ApiDoc;
