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
            "/api/v1/properties/{property}/block-reasons",
            "/api/v1/properties/{property}/block-reasons/{reason}",
            "/api/v1/properties/{property}/blocks/{block}",
            "/api/v1/properties/{property}/room-types",
            "/api/v1/properties/{property}/room-types/order",
            "/api/v1/properties/{property}/room-types/{room_type}",
            "/api/v1/properties/{property}/rooms",
            "/api/v1/properties/{property}/rooms/bulk",
            "/api/v1/properties/{property}/rooms/order",
            "/api/v1/properties/{property}/rooms/{room}",
            "/api/v1/properties/{property}/rooms/{room}/blocks",
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

/// Responses that return a versioned resource send its version as `ETag`, and the document says so.
#[test]
fn versioned_responses_declare_their_etag() {
    let doc = ApiDoc::openapi();
    let mut declared: Vec<String> = doc
        .paths
        .paths
        .values()
        .flat_map(|item| [&item.get, &item.put, &item.post, &item.delete, &item.patch])
        .flatten()
        .filter(|operation| {
            ["200", "201"].iter().any(|status| match operation.responses.responses.get(*status) {
                Some(utoipa::openapi::RefOr::T(response)) => response.headers.contains_key("ETag"),
                _ => false,
            })
        })
        .map(|operation| operation.operation_id.clone().expect("every operation has an id"))
        .collect();
    declared.sort();

    assert_eq!(
        declared,
        [
            "create_block",
            "create_block_reason",
            "create_property",
            "create_room",
            "create_room_type",
            "create_section",
            "rename_section",
            "shorten_block",
            "update_block_reason",
            "update_property",
            "update_room",
            "update_room_type",
        ]
    );
}
