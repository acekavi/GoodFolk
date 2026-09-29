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
            "/api/v1/properties/{property}/accounts",
            "/api/v1/properties/{property}/accounts/{account}",
            "/api/v1/properties/{property}/block-reasons",
            "/api/v1/properties/{property}/block-reasons/{reason}",
            "/api/v1/properties/{property}/blocks/{block}",
            "/api/v1/properties/{property}/cancellation-policies",
            "/api/v1/properties/{property}/cancellation-policies/{policy}",
            "/api/v1/properties/{property}/guests",
            "/api/v1/properties/{property}/guests/{guest}",
            "/api/v1/properties/{property}/meal-supplements",
            "/api/v1/properties/{property}/meal-supplements/{supplement}",
            "/api/v1/properties/{property}/rate-plans",
            "/api/v1/properties/{property}/rate-plans/{plan}",
            "/api/v1/properties/{property}/rate-plans/{plan}/bulk-change",
            "/api/v1/properties/{property}/rate-plans/{plan}/prices",
            "/api/v1/properties/{property}/rate-plans/{plan}/restrictions",
            "/api/v1/properties/{property}/reservation-rooms/{room}/assign",
            "/api/v1/properties/{property}/reservation-rooms/{room}/cancel",
            "/api/v1/properties/{property}/reservation-rooms/{room}/check-in",
            "/api/v1/properties/{property}/reservation-rooms/{room}/check-out",
            "/api/v1/properties/{property}/reservation-rooms/{room}/guests",
            "/api/v1/properties/{property}/reservation-rooms/{room}/guests/{guest}",
            "/api/v1/properties/{property}/reservation-rooms/{room}/modify",
            "/api/v1/properties/{property}/reservation-rooms/{room}/unassign",
            "/api/v1/properties/{property}/reservation-rooms/{room}/undo-check-in",
            "/api/v1/properties/{property}/reservations",
            "/api/v1/properties/{property}/reservations/{reservation}",
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
            "add_reservation_room_guest",
            "assign_reservation_room",
            "cancel_reservation_room",
            "check_in_reservation_room",
            "check_out_reservation_room",
            "create_account",
            "create_block",
            "create_block_reason",
            "create_cancellation_policy",
            "create_guest",
            "create_meal_supplement",
            "create_property",
            "create_rate_plan",
            "create_reservation",
            "create_room",
            "create_room_type",
            "create_section",
            "modify_reservation_room",
            "remove_reservation_room_guest",
            "rename_section",
            "shorten_block",
            "unassign_reservation_room",
            "undo_check_in_reservation_room",
            "update_account",
            "update_block_reason",
            "update_cancellation_policy",
            "update_guest",
            "update_meal_supplement",
            "update_property",
            "update_rate_plan",
            "update_reservation",
            "update_room",
            "update_room_type",
        ]
    );
}

/// Enums are documented with the values the API sends and accepts, so generated types match the JSON.
#[test]
fn rate_enums_are_documented_with_their_json_values() {
    let doc = serde_json::to_value(ApiDoc::openapi()).unwrap();
    let values = |name: &str| doc["components"]["schemas"][name]["enum"].clone();

    assert_eq!(values("PlanKind"), serde_json::json!(["standard", "derived", "custom"]));
    assert_eq!(values("Segment"), serde_json::json!(["FIT_F", "FIT_L", "OTA", "TA", "IBE"]));
    assert_eq!(values("MealPlan"), serde_json::json!(["RO", "BB", "HB", "FB"]));
    assert_eq!(values("Residency"), serde_json::json!(["resident", "non_resident"]));
}
