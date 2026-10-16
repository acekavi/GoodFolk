mod common;

use common::{SECRET, Server, fixture, http, start};
use proto::media::finalize_response::Result as Outcome;
use proto::media::media_client::MediaClient;
use proto::media::{CompletedPart, CreateUploadRequest, FinalizeRequest, Purpose};
use proto::tonic_health::pb::health_check_response::ServingStatus;
use proto::tonic_health::pb::{HealthCheckRequest, health_client::HealthClient};
use sha2::{Digest, Sha256};
use storage::{Bucket, upload_key};
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;
use tonic::{Code, Request, Status};
use uuid::Uuid;

type Client = MediaClient<InterceptedService<Channel, fn(Request<()>) -> Result<Request<()>, Status>>>;

fn authorise(mut request: Request<()>) -> Result<Request<()>, Status> {
    request.metadata_mut().insert("authorization", format!("Bearer {SECRET}").parse().unwrap());
    Ok(request)
}

async fn channel(server: &Server) -> Channel {
    Channel::from_shared(format!("http://{}", server.addr)).unwrap().connect().await.unwrap()
}

async fn client(server: &Server) -> Client {
    MediaClient::with_interceptor(channel(server).await, authorise as fn(_) -> _)
}

fn create(tenant: Uuid, asset: Uuid, purpose: Purpose, bytes: u64) -> CreateUploadRequest {
    CreateUploadRequest { tenant_id: tenant.to_string(), asset_id: asset.to_string(), purpose: purpose.into(), bytes }
}

/// Uploads `body` through presigned part URLs and finalizes it, declaring `sha256`.
async fn upload(server: &Server, purpose: Purpose, body: &[u8], sha256: Vec<u8>) -> (Uuid, Uuid, Outcome) {
    let mut client = client(server).await;
    let (tenant, asset) = (Uuid::now_v7(), Uuid::now_v7());
    let created = client.create_upload(create(tenant, asset, purpose, body.len() as u64)).await.unwrap().into_inner();
    let mut parts = Vec::new();
    for (part, chunk) in created.parts.iter().zip(body.chunks(created.part_size as usize)) {
        let response = http().put(&part.url).body(chunk.to_vec()).send().await.unwrap();
        assert!(response.status().is_success(), "{}", response.status());
        let etag = response.headers()["etag"].to_str().unwrap().to_owned();
        parts.push(CompletedPart { number: part.number, etag });
    }
    let finalized = client
        .finalize(FinalizeRequest {
            tenant_id: tenant.to_string(),
            asset_id: asset.to_string(),
            purpose: purpose.into(),
            s3_upload_id: created.s3_upload_id,
            parts,
            declared_bytes: body.len() as u64,
            sha256,
        })
        .await
        .unwrap()
        .into_inner();
    (tenant, asset, finalized.result.unwrap())
}

fn sha(body: &[u8]) -> Vec<u8> {
    Sha256::digest(body).to_vec()
}

async fn refused(server: &Server, purpose: Purpose, body: &[u8], sha256: Vec<u8>, reason: &str) {
    let (tenant, asset, outcome) = upload(server, purpose, body, sha256).await;
    match outcome {
        Outcome::Refused(refused) => assert_eq!(refused.reason, reason),
        Outcome::Verified(verified) => panic!("verified {verified:?}"),
    }
    let left = server.storage.head(Bucket::Private, &upload_key(tenant, asset)).await.unwrap();
    assert_eq!(left, None, "a refused upload is deleted");
}

#[tokio::test]
async fn create_upload_refuses_oversize() {
    let server = start().await;
    let mut client = client(&server).await;
    let (tenant, asset) = (Uuid::now_v7(), Uuid::now_v7());
    for (purpose, bytes) in [
        (Purpose::Gallery, 26_214_401),
        (Purpose::IssuePhoto, 26_214_401),
        (Purpose::GuestDocument, 15_728_641),
        (Purpose::Gallery, 0),
    ] {
        let error = client.create_upload(create(tenant, asset, purpose, bytes)).await.unwrap_err();
        assert_eq!(error.code(), Code::InvalidArgument, "{purpose:?} {bytes}");
    }
    let error = client.create_upload(create(tenant, asset, Purpose::Unspecified, 10)).await.unwrap_err();
    assert_eq!(error.code(), Code::InvalidArgument);

    // At the limit: four 8 MiB parts, the last one short.
    let created = client.create_upload(create(tenant, asset, Purpose::Gallery, 26_214_400)).await.unwrap().into_inner();
    assert_eq!(created.parts.iter().map(|p| p.number).collect::<Vec<_>>(), [1, 2, 3, 4]);
    assert_eq!(created.part_size, 8 * 1024 * 1024);
    server.storage.abort_multipart(Bucket::Private, &upload_key(tenant, asset), &created.s3_upload_id).await.unwrap();
}

#[tokio::test]
async fn finalize_verifies_and_returns_blake3() {
    let server = start().await;
    let body = fixture("small.jpg");
    let (tenant, asset, outcome) = upload(&server, Purpose::Gallery, &body, sha(&body)).await;
    let Outcome::Verified(verified) = outcome else { panic!("refused: {outcome:?}") };
    assert_eq!(verified.blake3, blake3::hash(&body).as_bytes());
    assert_eq!(verified.mime, "image/jpeg");
    assert_eq!(verified.bytes, body.len() as u64);
    assert!(verified.width > 0 && verified.height > 0);
    let key = upload_key(tenant, asset);
    assert_eq!(server.storage.head(Bucket::Private, &key).await.unwrap(), Some(body.len() as u64));
    server.storage.delete(Bucket::Private, &key).await.unwrap();
}

#[tokio::test]
async fn finalize_verifies_a_multipart_pdf_guest_document() {
    let server = start().await;
    // Over one part, so the parts are put back together: a PDF header followed by padding.
    let mut body = fixture("doc.pdf");
    body.resize(9 * 1024 * 1024, b' ');
    let (tenant, asset, outcome) = upload(&server, Purpose::GuestDocument, &body, sha(&body)).await;
    let Outcome::Verified(verified) = outcome else { panic!("refused: {outcome:?}") };
    assert_eq!((verified.mime.as_str(), verified.bytes), ("application/pdf", body.len() as u64));
    server.storage.delete(Bucket::Private, &upload_key(tenant, asset)).await.unwrap();
}

#[tokio::test]
async fn finalize_refuses_checksum_mismatch() {
    let server = start().await;
    let body = fixture("small.jpg");
    refused(&server, Purpose::Gallery, &body, sha(b"something else"), "The file does not match its checksum").await;
}

#[tokio::test]
async fn finalize_refuses_disguised_pdf_as_gallery() {
    let server = start().await;
    let body = fixture("fake.jpg");
    refused(&server, Purpose::Gallery, &body, sha(&body), "This file type is not accepted here").await;
}

#[tokio::test]
async fn finalize_refuses_heic() {
    let server = start().await;
    let body = fixture("photo.heic");
    refused(&server, Purpose::Gallery, &body, sha(&body), "This file type is not accepted here").await;
}

#[tokio::test]
async fn finalize_refuses_bomb_before_decode() {
    let server = start().await;
    let body = fixture("bomb.png");
    refused(&server, Purpose::Gallery, &body, sha(&body), "The image is larger than 100 megapixels").await;
}

#[tokio::test]
async fn grpc_rejects_unauthenticated() {
    let server = start().await;
    let mut client = MediaClient::new(channel(&server).await);
    let error = client.create_upload(create(Uuid::now_v7(), Uuid::now_v7(), Purpose::Gallery, 10)).await.unwrap_err();
    assert_eq!(error.code(), Code::Unauthenticated);
}

#[tokio::test]
async fn health_reports_serving() {
    let server = start().await;
    let mut health = HealthClient::new(channel(&server).await);
    let reply = health.check(HealthCheckRequest { service: String::new() }).await.unwrap().into_inner();
    assert_eq!(reply.status(), ServingStatus::Serving);

    let response = http().get(format!("http://{}/healthz", server.addr)).send().await.unwrap();
    assert_eq!(response.status(), 200);
}

/// Uploads `sent` for a file declared as `declared` and returns the refusal's reason, checking the upload is gone.
async fn refused_size(declared: Vec<u8>, sent: Vec<u8>) -> String {
    let server = start().await;
    let mut client = client(&server).await;
    let (tenant, asset) = (Uuid::now_v7(), Uuid::now_v7());
    let created = client
        .create_upload(create(tenant, asset, Purpose::Gallery, declared.len() as u64))
        .await
        .unwrap()
        .into_inner();
    let response = http().put(&created.parts[0].url).body(sent).send().await.unwrap();
    let etag = response.headers()["etag"].to_str().unwrap().to_owned();
    let finalized = client
        .finalize(FinalizeRequest {
            tenant_id: tenant.to_string(),
            asset_id: asset.to_string(),
            purpose: Purpose::Gallery.into(),
            s3_upload_id: created.s3_upload_id,
            parts: vec![CompletedPart { number: 1, etag }],
            declared_bytes: declared.len() as u64,
            sha256: sha(&declared),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(server.storage.head(Bucket::Private, &upload_key(tenant, asset)).await.unwrap(), None);
    match finalized.result.unwrap() {
        Outcome::Refused(refused) => refused.reason,
        Outcome::Verified(verified) => panic!("verified {verified:?}"),
    }
}

#[tokio::test]
async fn finalize_refuses_more_bytes_than_declared() {
    // The browser sends more than it declared; the size is checked with `head` before anything is read.
    let declared = fixture("small.jpg");
    let mut sent = declared.clone();
    sent.extend_from_slice(&[0; 4096]);
    assert_eq!(refused_size(declared, sent).await, "The file is larger than declared");
}

#[tokio::test]
async fn finalize_refuses_fewer_bytes_than_declared() {
    let declared = fixture("small.jpg");
    let sent = declared[..declared.len() / 2].to_vec();
    assert_eq!(refused_size(declared, sent).await, "The file is smaller than declared");
}

#[tokio::test]
async fn finalize_rejects_a_malformed_request_before_completing() {
    let server = start().await;
    let mut client = client(&server).await;
    let request = |sha256: Vec<u8>, parts: u32| FinalizeRequest {
        tenant_id: Uuid::now_v7().to_string(),
        asset_id: Uuid::now_v7().to_string(),
        purpose: Purpose::Gallery.into(),
        s3_upload_id: "never-started".to_owned(),
        parts: (1..=parts).map(|number| CompletedPart { number, etag: "\"e\"".to_owned() }).collect(),
        declared_bytes: 10,
        sha256,
    };
    let short_hash = client.finalize(request(vec![0; 31], 1)).await.unwrap_err();
    assert_eq!(short_hash.code(), Code::InvalidArgument);
    // A gallery file is at most 25 MiB: four 8 MiB parts.
    let too_many_parts = client.finalize(request(vec![0; 32], 5)).await.unwrap_err();
    assert_eq!(too_many_parts.code(), Code::InvalidArgument);
}
