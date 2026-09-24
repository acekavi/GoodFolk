use core_api::openapi::ApiDoc;
use utoipa::OpenApi;

#[test]
fn the_openapi_document_lists_every_rest_route() {
    let doc = ApiDoc::openapi();
    let paths: Vec<&str> = doc.paths.paths.keys().map(String::as_str).collect();

    assert_eq!(
        paths,
        vec![
            "/api/v1/auth/login",
            "/api/v1/auth/logout",
            "/api/v1/auth/signup",
            "/api/v1/me",
            "/api/v1/properties",
            "/api/v1/properties/{property}",
            "/api/v1/session/tenant",
        ]
    );
}
