//! Runs against a real S3 server: RustFS from `deploy/dev/rustfs.sh`, or whatever the `S3_*` variables name.

use std::time::Duration;

use reqwest::StatusCode;
use storage::{Bucket, IMMUTABLE, PART_SIZE, Storage, StorageConfig, StorageError, install_tls_provider};
use uuid::Uuid;

const TTL: Duration = Duration::from_secs(60);

fn http() -> reqwest::Client {
    install_tls_provider();
    reqwest::Client::new()
}

async fn storage() -> Storage {
    let cfg = StorageConfig::dev().expect("storage configuration");
    let reachable = http().get(cfg.endpoint.clone()).timeout(Duration::from_secs(2)).send().await.is_ok();
    assert!(reachable, "start RustFS: deploy/dev/rustfs.sh start");
    let storage = Storage::new(cfg).unwrap();
    storage.ensure_dev_buckets().await.expect("buckets");
    storage
}

/// A key no other test (or earlier run) uses.
fn fresh_key() -> String {
    format!("test/{}", Uuid::now_v7())
}

#[tokio::test]
async fn multipart_upload_via_presigned_parts() {
    let storage = storage().await;
    let http = http();
    let key = fresh_key();
    let upload_id = storage.create_multipart(Bucket::Private, &key).await.unwrap();

    let sizes = [usize::try_from(PART_SIZE).unwrap(), 1024];
    let mut etags = Vec::new();
    for (index, size) in sizes.iter().enumerate() {
        let number = u16::try_from(index + 1).unwrap();
        let url = storage.presign_part(Bucket::Private, &key, &upload_id, number, TTL);
        let response = http.put(url).body(vec![number as u8; *size]).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let etag = response.headers()["etag"].to_str().unwrap().to_owned();
        etags.push((number, etag));
    }
    storage.complete_multipart(Bucket::Private, &key, &upload_id, &etags).await.unwrap();

    let expected = u64::try_from(sizes.iter().sum::<usize>()).unwrap();
    assert_eq!(storage.head(Bucket::Private, &key).await.unwrap(), Some(expected));
    storage.delete(Bucket::Private, &key).await.unwrap();
    assert_eq!(storage.head(Bucket::Private, &key).await.unwrap(), None);
}

#[tokio::test]
async fn complete_refuses_gaps_in_part_numbers() {
    let storage = storage().await;
    let err = storage
        .complete_multipart(Bucket::Private, &fresh_key(), "any", &[(1, "a".into()), (3, "b".into())])
        .await
        .unwrap_err();
    assert!(matches!(err, StorageError::InvalidParts));
}

#[tokio::test]
async fn put_and_get_round_trip() {
    let storage = storage().await;
    let key = fresh_key();
    storage.put(Bucket::Private, &key, b"hello".to_vec(), "text/plain", None).await.unwrap();
    assert_eq!(&storage.get(Bucket::Private, &key).await.unwrap()[..], b"hello");
    storage.delete(Bucket::Private, &key).await.unwrap();
    assert!(matches!(
        storage.get(Bucket::Private, &key).await,
        Err(StorageError::Status { status: StatusCode::NOT_FOUND, .. })
    ));
}

#[tokio::test]
async fn presigned_get_expires() {
    let storage = storage().await;
    let http = http();
    let key = fresh_key();
    storage.put(Bucket::Private, &key, b"secret".to_vec(), "text/plain", None).await.unwrap();

    let url = storage.presign_get(Bucket::Private, &key, Duration::from_secs(1));
    assert_eq!(http.get(url.clone()).send().await.unwrap().status(), StatusCode::OK);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(http.get(url).send().await.unwrap().status(), StatusCode::FORBIDDEN);

    storage.delete(Bucket::Private, &key).await.unwrap();
}

#[tokio::test]
async fn public_bucket_is_anonymous_private_is_not() {
    let storage = storage().await;
    let http = http();
    let key = fresh_key();
    for bucket in [Bucket::Public, Bucket::Private] {
        storage.put(bucket, &key, b"pixels".to_vec(), "image/webp", Some(IMMUTABLE)).await.unwrap();
    }

    let public = http.get(storage.public_url(&key)).send().await.unwrap();
    assert_eq!(public.status(), StatusCode::OK);
    assert_eq!(public.headers()["cache-control"], IMMUTABLE);

    // The same key, unsigned, in the private bucket: the public base URL names the public bucket, so build the
    // private URL from a presigned one with its signature removed.
    let mut private = storage.presign_get(Bucket::Private, &key, TTL);
    private.set_query(None);
    assert_eq!(http.get(private).send().await.unwrap().status(), StatusCode::FORBIDDEN);

    for bucket in [Bucket::Public, Bucket::Private] {
        storage.delete(bucket, &key).await.unwrap();
    }
}

#[tokio::test]
async fn abort_removes_parts() {
    let storage = storage().await;
    let http = http();
    let key = fresh_key();
    let upload_id = storage.create_multipart(Bucket::Private, &key).await.unwrap();
    let url = storage.presign_part(Bucket::Private, &key, &upload_id, 1, TTL);
    let etag = http.put(url).body(vec![1u8; 1024]).send().await.unwrap().headers()["etag"].to_str().unwrap().to_owned();

    storage.abort_multipart(Bucket::Private, &key, &upload_id).await.unwrap();

    let completed = storage.complete_multipart(Bucket::Private, &key, &upload_id, &[(1, etag)]).await;
    assert!(matches!(completed, Err(StorageError::Status { status: StatusCode::NOT_FOUND, .. })));
    assert_eq!(storage.head(Bucket::Private, &key).await.unwrap(), None);
}

#[tokio::test]
async fn browser_preflight_for_a_part_upload_is_allowed() {
    let storage = storage().await;
    let http = http();
    let key = fresh_key();
    let url = storage.presign_part(Bucket::Private, &key, "preflight", 1, TTL);
    let origin = "http://localhost:4173";

    let preflight = http
        .request(reqwest::Method::OPTIONS, url)
        .header("origin", origin)
        .header("access-control-request-method", "PUT")
        .header("access-control-request-headers", "content-type")
        .send()
        .await
        .unwrap();
    assert!(preflight.status().is_success(), "preflight answered {}", preflight.status());
    assert_eq!(preflight.headers()["access-control-allow-origin"], origin);
    let methods = preflight.headers()["access-control-allow-methods"].to_str().unwrap();
    assert!(methods.contains("PUT"), "allowed methods: {methods}");

    let stranger = http
        .request(reqwest::Method::OPTIONS, storage.presign_part(Bucket::Private, &key, "preflight", 1, TTL))
        .header("origin", "http://evil.example")
        .header("access-control-request-method", "PUT")
        .send()
        .await
        .unwrap();
    assert!(stranger.headers().get("access-control-allow-origin").is_none(), "an unlisted origin was allowed");
}

#[tokio::test]
async fn browser_can_read_the_etag_of_an_uploaded_part() {
    let storage = storage().await;
    let http = http();
    let key = fresh_key();
    let upload_id = storage.create_multipart(Bucket::Private, &key).await.unwrap();
    let url = storage.presign_part(Bucket::Private, &key, &upload_id, 1, TTL);

    let response = http.put(url).header("origin", "http://localhost:4173").body(vec![0u8; 16]).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let exposed = response.headers()["access-control-expose-headers"].to_str().unwrap().to_ascii_lowercase();
    assert!(exposed.contains("etag"), "exposed headers: {exposed}");

    storage.abort_multipart(Bucket::Private, &key, &upload_id).await.unwrap();
}
