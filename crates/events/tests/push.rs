use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use events::{BusError, EventBus, HeaderProvider, Message, PubSubBus, PushBus, PushEnvelope};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use url::Url;
use uuid::Uuid;

type Seen = Arc<Mutex<Vec<(Option<String>, serde_json::Value)>>>;

async fn serve(status: StatusCode) -> (Url, Seen) {
    let seen = Seen::default();
    let app = Router::new()
        .route(
            "/work",
            post(move |State(seen): State<Seen>, headers: HeaderMap, body: String| async move {
                let auth = headers.get("authorization").map(|v| v.to_str().unwrap().to_owned());
                seen.lock().unwrap().push((auth, serde_json::from_str(&body).unwrap()));
                status
            }),
        )
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = Url::parse(&format!("http://{}/work", listener.local_addr().unwrap())).unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, seen)
}

fn message() -> Message {
    Message { id: Uuid::now_v7(), topic: "media-process".to_owned(), payload: json!({"asset": "a1", "n": 3}) }
}

fn push_bus(url: Url, auth: Option<HeaderProvider>) -> PushBus {
    PushBus::new(HashMap::from([("media-process".to_owned(), url)]), auth).unwrap()
}

#[tokio::test]
async fn push_bus_posts_pubsub_envelope() {
    let (url, seen) = serve(StatusCode::NO_CONTENT).await;
    let auth: HeaderProvider = Arc::new(|| Box::pin(async { Ok("Bearer secret-token".to_owned()) }));
    let msg = message();
    push_bus(url, Some(auth)).publish(&msg).await.unwrap();

    let seen = seen.lock().unwrap();
    let (header, body) = &seen[0];
    assert_eq!(header.as_deref(), Some("Bearer secret-token"));
    assert_eq!(body["message"]["messageId"], msg.id.to_string());
    assert_eq!(body["message"]["attributes"]["message_id"], msg.id.to_string());
    let envelope: PushEnvelope = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(envelope.decode::<serde_json::Value>().unwrap(), msg.payload);
    assert_eq!(envelope.dedupe_id(), msg.id.to_string());
}

#[tokio::test]
async fn push_bus_classifies_statuses() {
    for (status, permanent) in [
        (StatusCode::BAD_REQUEST, true),
        (StatusCode::NOT_FOUND, true),
        (StatusCode::REQUEST_TIMEOUT, false),
        (StatusCode::TOO_MANY_REQUESTS, false),
        (StatusCode::INTERNAL_SERVER_ERROR, false),
    ] {
        let (url, _) = serve(status).await;
        let error = push_bus(url, None).publish(&message()).await.unwrap_err();
        assert_eq!(matches!(error, BusError::Permanent(_)), permanent, "{status}");
    }
}

#[tokio::test]
async fn push_bus_without_endpoint_is_retryable() {
    let bus = PushBus::new(HashMap::new(), None).unwrap();
    assert!(matches!(bus.publish(&message()).await, Err(BusError::Retryable(_))));
}

#[tokio::test]
async fn pubsub_bus_publishes_with_metadata_token() {
    let seen = Seen::default();
    let app = Router::new()
        .route(
            "/v1/projects/p1/topics/media-process:publish",
            post(|State(seen): State<Seen>, headers: HeaderMap, body: String| async move {
                let auth = headers.get("authorization").map(|v| v.to_str().unwrap().to_owned());
                seen.lock().unwrap().push((auth, serde_json::from_str(&body).unwrap()));
                "{\"messageIds\":[\"1\"]}"
            }),
        )
        .route(
            "/token",
            axum::routing::get(|headers: HeaderMap| async move {
                assert_eq!(headers.get("metadata-flavor").unwrap(), "Google");
                "{\"access_token\":\"tok\",\"expires_in\":3599}"
            }),
        )
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let bus = PubSubBus::with_token_url("p1", &base, Some(&format!("{base}/token"))).unwrap();
    let msg = message();
    bus.publish(&msg).await.unwrap();

    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].0.as_deref(), Some("Bearer tok"));
    let published = &seen[0].1["messages"][0];
    assert_eq!(published["attributes"]["message_id"], msg.id.to_string());
    assert!(published["data"].is_string());
}

/// Runs against a Pub/Sub emulator: `PUBSUB_EMULATOR_HOST=localhost:8085 cargo test -p events -- --ignored`.
#[tokio::test]
#[ignore = "needs PUBSUB_EMULATOR_HOST"]
async fn pubsub_emulator_round_trip() {
    let host = std::env::var("PUBSUB_EMULATOR_HOST").expect("PUBSUB_EMULATOR_HOST");
    let project = "goodfolk-test";
    let topic = format!("rt-{}", Uuid::now_v7().simple());
    let http = reqwest::Client::new();
    let base = format!("http://{host}/v1/projects/{project}");
    let put = |url: String, body: serde_json::Value| {
        let http = http.clone();
        async move {
            let response = http.put(url).header("content-type", "application/json").body(body.to_string()).send().await;
            assert!(response.unwrap().status().is_success());
        }
    };
    put(format!("{base}/topics/{topic}"), json!({})).await;
    put(format!("{base}/subscriptions/{topic}-sub"), json!({"topic": format!("projects/{project}/topics/{topic}")}))
        .await;

    let msg = Message { id: Uuid::now_v7(), topic: topic.clone(), payload: json!({"asset": "a1"}) };
    PubSubBus::emulator(project, &host).unwrap().publish(&msg).await.unwrap();

    let pulled = http
        .post(format!("{base}/subscriptions/{topic}-sub:pull"))
        .header("content-type", "application/json")
        .body(json!({"maxMessages": 1}).to_string())
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let pulled: serde_json::Value = serde_json::from_str(&pulled).unwrap();
    let received = &pulled["receivedMessages"][0]["message"];
    assert_eq!(received["attributes"]["message_id"], msg.id.to_string());
}
