//! SES: the SigV4 signature against AWS's published test vector, and `SendEmail` against a local server.

use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use mail::{Credentials, MailError, Mailer, Message, SesMailer};

fn credentials() -> Credentials {
    Credentials {
        access_key_id: "AKIDEXAMPLE".into(),
        secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
        session_token: None,
    }
}

/// aws-signing-test-suite/v4/get-vanilla.
#[test]
fn the_signature_matches_the_get_vanilla_test_vector() {
    let mut request = reqwest::Request::new(reqwest::Method::GET, "https://example.amazonaws.com/".parse().unwrap());
    // 2015-08-30T12:36:00Z
    let time = UNIX_EPOCH + Duration::from_secs(1_440_938_160);

    mail::sign(&mut request, &credentials(), "us-east-1", "service", time).unwrap();

    assert_eq!(request.headers()["x-amz-date"], "20150830T123600Z");
    assert_eq!(
        request.headers()["authorization"],
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
    );
}

#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<(String, String, serde_json::Value)>>>);

/// A stand-in for SES that records each call's `Authorization` and path and answers `status` (with `error_type`).
async fn server(status: StatusCode, error_type: Option<&'static str>) -> (String, Seen) {
    let seen = Seen::default();
    let recording = seen.clone();
    let app = axum::Router::new().fallback(move |uri: axum::http::Uri, headers: HeaderMap, body: axum::body::Bytes| {
        let recording = recording.clone();
        async move {
            let authorization = headers["authorization"].to_str().unwrap().to_owned();
            recording.0.lock().unwrap().push((
                uri.path().to_owned(),
                authorization,
                serde_json::from_slice(&body).unwrap(),
            ));
            let mut response = (status, "{}").into_response();
            if let Some(kind) = error_type {
                response.headers_mut().insert("x-amzn-ErrorType", kind.parse().unwrap());
            }
            response
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), seen)
}

fn message() -> Message {
    Message { to: "kamala@example.com".into(), subject: "Hi".into(), text: "Text".into(), html: "<p>Html</p>".into() }
}

fn mailer(endpoint: &str) -> SesMailer {
    SesMailer::new("ap-south-1", "hello@example.com", credentials()).unwrap().with_endpoint(endpoint)
}

#[tokio::test]
async fn send_email_posts_simple_content_signed_for_ses() {
    let (endpoint, seen) = server(StatusCode::OK, None).await;

    mailer(&endpoint).send(&message()).await.unwrap();

    let calls = seen.0.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    let (path, authorization, body) = &calls[0];
    assert_eq!(path, "/v2/email/outbound-emails");
    let (scope, rest) = authorization
        .strip_prefix("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/")
        .and_then(|rest| rest.split_once(", SignedHeaders=content-type;host;x-amz-date, Signature="))
        .unwrap_or_else(|| panic!("{authorization}"));
    let (date, scope) = scope.split_at(8);
    assert!(date.bytes().all(|b| b.is_ascii_digit()), "{authorization}");
    assert_eq!(scope, "/ap-south-1/ses/aws4_request");
    assert!(rest.len() == 64 && rest.bytes().all(|b| b.is_ascii_hexdigit()), "{authorization}");
    assert_eq!(
        body,
        &serde_json::json!({
            "FromEmailAddress": "hello@example.com",
            "Destination": { "ToAddresses": ["kamala@example.com"] },
            "Content": { "Simple": {
                "Subject": { "Data": "Hi", "Charset": "UTF-8" },
                "Body": {
                    "Text": { "Data": "Text", "Charset": "UTF-8" },
                    "Html": { "Data": "<p>Html</p>", "Charset": "UTF-8" },
                },
            } },
        })
    );
}

#[tokio::test]
async fn throttles_pauses_forbidden_and_server_errors_are_retried_and_other_refusals_are_not() {
    let cases = [
        (StatusCode::TOO_MANY_REQUESTS, Some("TooManyRequestsException:http://internal.amazon.com/"), true),
        (StatusCode::BAD_REQUEST, Some("ThrottlingException"), true),
        (StatusCode::SERVICE_UNAVAILABLE, None, true),
        (StatusCode::FORBIDDEN, None, true),
        (StatusCode::FORBIDDEN, Some("AccessDeniedException:http://internal.amazon.com/"), true),
        (StatusCode::BAD_REQUEST, Some("SendingPausedException:http://internal.amazon.com/"), true),
        (StatusCode::BAD_REQUEST, Some("LimitExceededException:http://internal.amazon.com/"), true),
        (StatusCode::BAD_REQUEST, Some("MessageRejected:http://internal.amazon.com/"), false),
        (StatusCode::BAD_REQUEST, Some("BadRequestException:http://internal.amazon.com/"), false),
    ];
    for (status, kind, retryable) in cases {
        let (endpoint, _) = server(status, kind).await;
        match mailer(&endpoint).send(&message()).await.unwrap_err() {
            MailError::Retryable(_) => assert!(retryable, "{status} {kind:?} was retried"),
            MailError::Permanent(detail) => {
                assert!(!retryable, "{status} {kind:?} was not retried");
                assert!(!detail.contains("Html"), "{detail}");
            }
        }
    }
}
