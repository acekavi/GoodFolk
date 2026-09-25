use core_api::openapi::ApiDoc;
use std::collections::BTreeSet;
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
            "/api/v1/properties/{property}/room-types",
            "/api/v1/properties/{property}/room-types/order",
            "/api/v1/properties/{property}/room-types/{room_type}",
            "/api/v1/properties/{property}/rooms",
            "/api/v1/properties/{property}/rooms/bulk",
            "/api/v1/properties/{property}/rooms/order",
            "/api/v1/properties/{property}/rooms/{room}",
            "/api/v1/properties/{property}/sections",
            "/api/v1/properties/{property}/sections/{section}",
            "/api/v1/session/tenant",
        ]
    );
}

/// Generated TypeScript names operations by id, so a repeated id would silently merge two operations.
#[test]
fn every_operation_has_its_own_id() {
    let doc = ApiDoc::openapi();
    let ids: Vec<String> = doc
        .paths
        .paths
        .values()
        .flat_map(|item| [&item.get, &item.put, &item.post, &item.delete, &item.patch])
        .flatten()
        .map(|operation| operation.operation_id.clone().expect("every operation has an id"))
        .collect();

    let unique: BTreeSet<&String> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len(), "repeated operation ids in {ids:?}");
}
