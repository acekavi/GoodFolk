//! S3 access for the media engine: presigned multipart uploads and reads, plus plain object operations.
//!
//! Production talks to Cloudflare R2; development and tests talk to RustFS (`deploy/dev/rustfs.sh`). Requests are
//! built and signed by `rusty-s3` and sent with `reqwest`.

use std::time::Duration;

use anyhow::{Context, bail};
use bytes::Bytes;
use reqwest::{Method as HttpMethod, StatusCode};
use rusty_s3::actions::{
    AbortMultipartUpload, CompleteMultipartUpload, CreateBucket, CreateMultipartUpload, DeleteObject, GetObject,
    HeadObject, PutObject, S3Action, UploadPart,
};
use rusty_s3::{Bucket as S3Bucket, Credentials, Method, UrlStyle, signing};
use url::Url;
use uuid::Uuid;

/// Size of every multipart part but the last.
pub const PART_SIZE: u64 = 8 * 1024 * 1024;
/// The largest object [`Storage::get`] will buffer.
pub const MAX_OBJECT_BYTES: u64 = 32 * 1024 * 1024;
/// S3 allows at most this many parts in one multipart upload, numbered from 1.
const MAX_PARTS: usize = 10_000;
/// Cache-Control for objects whose key never changes content.
pub const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// How long a signed request made by this client stays valid.
const REQUEST_TTL: Duration = Duration::from_secs(60);
/// Origins allowed to PUT and GET against the private bucket in development.
const DEV_CORS_ORIGINS: &str = "http://localhost:5173,http://localhost:4173";

/// reqwest is built without a bundled TLS provider, so one must be installed before any client is built. This
/// installs ring, which the tree already has; another component having installed a provider first is fine.
/// [`Storage::new`] calls it, so only code that builds its own reqwest client needs to.
pub fn install_tls_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[derive(Clone)]
pub struct StorageConfig {
    pub endpoint: Url,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    pub public_bucket: String,
    pub private_bucket: String,
    pub path_style: bool,
    pub public_base_url: Url,
}

impl std::fmt::Debug for StorageConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageConfig")
            .field("endpoint", &self.endpoint.as_str())
            .field("region", &self.region)
            .field("access_key", &"<redacted>")
            .field("secret_key", &"<redacted>")
            .field("public_bucket", &self.public_bucket)
            .field("private_bucket", &self.private_bucket)
            .field("path_style", &self.path_style)
            .field("public_base_url", &self.public_base_url.as_str())
            .finish()
    }
}

impl StorageConfig {
    /// Reads `S3_ENDPOINT`, `S3_REGION`, `S3_ACCESS_KEY`, `S3_SECRET_KEY`, `S3_PUBLIC_BUCKET`, `S3_PRIVATE_BUCKET`,
    /// `S3_PATH_STYLE` and `MEDIA_PUBLIC_BASE_URL`. All are required except `S3_PATH_STYLE` (default `false`).
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_lookup(|name| std::env::var(name).ok(), None)
    }

    /// Development and test configuration: the same variables as [`from_env`](Self::from_env), each falling back
    /// to the local RustFS values. Never use this in production, where a missing variable must be an error.
    pub fn dev() -> anyhow::Result<Self> {
        Self::from_lookup(|name| std::env::var(name).ok(), Some(()))
    }

    /// As [`from_env`](Self::from_env) in production and [`dev`](Self::dev) otherwise, reading variables through
    /// `get`.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>, production: bool) -> anyhow::Result<Self> {
        Self::from_lookup(get, (!production).then_some(()))
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>, dev: Option<()>) -> anyhow::Result<Self> {
        let var = |name: &str, fallback: &str| match (get(name), dev) {
            (Some(value), _) => Ok(value),
            (None, Some(())) => Ok(fallback.to_owned()),
            (None, None) => bail!("{name} is not set"),
        };
        let private_bucket = var("S3_PRIVATE_BUCKET", "goodfolk-private")?;
        let public_bucket = var("S3_PUBLIC_BUCKET", "goodfolk-public")?;
        let endpoint = var("S3_ENDPOINT", "http://localhost:9000")?;
        let fallback_base = format!("{}/{public_bucket}/", endpoint.trim_end_matches('/'));
        let path_style = match get("S3_PATH_STYLE") {
            Some(value) => value.parse::<bool>().context("S3_PATH_STYLE must be true or false")?,
            None => dev.is_some(),
        };
        Ok(Self {
            endpoint: endpoint.parse().context("S3_ENDPOINT is not a URL")?,
            region: var("S3_REGION", "us-east-1")?,
            access_key: var("S3_ACCESS_KEY", "goodfolk")?,
            secret_key: var("S3_SECRET_KEY", "goodfolk_dev_secret")?,
            public_bucket,
            private_bucket,
            path_style,
            public_base_url: var("MEDIA_PUBLIC_BASE_URL", &fallback_base)?
                .parse()
                .context("MEDIA_PUBLIC_BASE_URL is not a URL")?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bucket {
    Public,
    Private,
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("storage request failed: {0}")]
    Transport(reqwest::Error),
    #[error("storage answered {status}: {body}")]
    Status { status: StatusCode, body: String },
    #[error("storage answered something unreadable: {0}")]
    Unreadable(String),
    #[error("parts must be numbered 1..=n without gaps or repeats, at most 10000")]
    InvalidParts,
    #[error("object is larger than {limit} bytes")]
    TooLarge { limit: u64 },
}

impl From<reqwest::Error> for StorageError {
    /// A reqwest error prints the request URL, and for presigned requests that carries the credential and signature.
    fn from(error: reqwest::Error) -> Self {
        Self::Transport(error.without_url())
    }
}

pub struct Storage {
    public: S3Bucket,
    private: S3Bucket,
    credentials: Credentials,
    public_base_url: Url,
    http: reqwest::Client,
}

impl Storage {
    pub fn new(cfg: StorageConfig) -> anyhow::Result<Self> {
        install_tls_provider();
        let style = if cfg.path_style { UrlStyle::Path } else { UrlStyle::VirtualHost };
        let bucket = |name: String| {
            S3Bucket::new(cfg.endpoint.clone(), style, name, cfg.region.clone()).context("invalid S3 bucket")
        };
        let mut public_base_url = cfg.public_base_url;
        if !public_base_url.path().ends_with('/') {
            public_base_url.set_path(&format!("{}/", public_base_url.path()));
        }
        Ok(Self {
            public: bucket(cfg.public_bucket)?,
            private: bucket(cfg.private_bucket)?,
            credentials: Credentials::new(cfg.access_key, cfg.secret_key),
            public_base_url,
            http: reqwest::Client::builder().build().context("building the HTTP client")?,
        })
    }

    fn bucket(&self, bucket: Bucket) -> &S3Bucket {
        match bucket {
            Bucket::Public => &self.public,
            Bucket::Private => &self.private,
        }
    }

    /// Starts a multipart upload and returns its upload id.
    pub async fn create_multipart(&self, bucket: Bucket, key: &str) -> Result<String, StorageError> {
        let action = CreateMultipartUpload::new(self.bucket(bucket), Some(&self.credentials), key);
        let body = self.send(HttpMethod::POST, action.sign(REQUEST_TTL), &[], Bytes::new()).await?.text().await?;
        let parsed =
            CreateMultipartUpload::parse_response(&body).map_err(|e| StorageError::Unreadable(e.to_string()))?;
        Ok(parsed.upload_id().to_owned())
    }

    /// A URL the browser PUTs one part to. `part` counts from 1 and must be at most 10000.
    pub fn presign_part(&self, bucket: Bucket, key: &str, upload_id: &str, part: u16, ttl: Duration) -> Url {
        debug_assert!((1..=MAX_PARTS).contains(&usize::from(part)), "part numbers run from 1 to 10000");
        UploadPart::new(self.bucket(bucket), Some(&self.credentials), key, part, upload_id).sign(ttl)
    }

    /// Completes a multipart upload. `etags` are `(part number, ETag)` and must cover 1..=n exactly, in any order.
    pub async fn complete_multipart(
        &self,
        bucket: Bucket,
        key: &str,
        upload_id: &str,
        etags: &[(u16, String)],
    ) -> Result<(), StorageError> {
        let mut ordered: Vec<&(u16, String)> = etags.iter().collect();
        ordered.sort_by_key(|(number, _)| *number);
        let contiguous = ordered.iter().zip(1u16..).all(|((number, _), expected)| *number == expected);
        if ordered.is_empty() || ordered.len() > MAX_PARTS || !contiguous {
            return Err(StorageError::InvalidParts);
        }
        let action = CompleteMultipartUpload::new(
            self.bucket(bucket),
            Some(&self.credentials),
            key,
            upload_id,
            ordered.iter().map(|(_, etag)| etag.as_str()),
        );
        let url = action.sign(REQUEST_TTL);
        let body = action.body();
        let response = self.send(HttpMethod::POST, url, &[], Bytes::from(body)).await?;
        // S3 may answer 200 and still report a failure in the body.
        let text = response.text().await?;
        if text.contains("<Error>") {
            return Err(StorageError::Unreadable(text));
        }
        Ok(())
    }

    /// Cancels a multipart upload and discards its parts.
    pub async fn abort_multipart(&self, bucket: Bucket, key: &str, upload_id: &str) -> Result<(), StorageError> {
        let action = AbortMultipartUpload::new(self.bucket(bucket), Some(&self.credentials), key, upload_id);
        self.send(HttpMethod::DELETE, action.sign(REQUEST_TTL), &[], Bytes::new()).await?;
        Ok(())
    }

    /// A time-limited URL that reads an object.
    pub fn presign_get(&self, bucket: Bucket, key: &str, ttl: Duration) -> Url {
        GetObject::new(self.bucket(bucket), Some(&self.credentials), key).sign(ttl)
    }

    /// The anonymous URL of an object in the public bucket. `key` must be a plain object key: no leading `/`, and no
    /// `?` or `#`, which would be read as a query or fragment.
    pub fn public_url(&self, key: &str) -> Url {
        self.public_base_url.join(key).expect("an object key joins onto the public base URL")
    }

    pub async fn put(
        &self,
        bucket: Bucket,
        key: &str,
        body: Vec<u8>,
        content_type: &str,
        cache_control: Option<&str>,
    ) -> Result<(), StorageError> {
        let mut action = PutObject::new(self.bucket(bucket), Some(&self.credentials), key);
        action.headers_mut().insert("content-type", content_type);
        let mut headers = vec![("content-type", content_type)];
        if let Some(cache_control) = cache_control {
            action.headers_mut().insert("cache-control", cache_control);
            headers.push(("cache-control", cache_control));
        }
        self.send(HttpMethod::PUT, action.sign(REQUEST_TTL), &headers, Bytes::from(body)).await?;
        Ok(())
    }

    /// Reads an object of at most [`MAX_OBJECT_BYTES`]; a larger one is refused with [`StorageError::TooLarge`].
    pub async fn get(&self, bucket: Bucket, key: &str) -> Result<Bytes, StorageError> {
        self.get_capped(bucket, key, MAX_OBJECT_BYTES).await
    }

    async fn get_capped(&self, bucket: Bucket, key: &str, limit: u64) -> Result<Bytes, StorageError> {
        let action = GetObject::new(self.bucket(bucket), Some(&self.credentials), key);
        let mut response = self.send(HttpMethod::GET, action.sign(REQUEST_TTL), &[], Bytes::new()).await?;
        if response.content_length().is_some_and(|length| length > limit) {
            return Err(StorageError::TooLarge { limit });
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if u64::try_from(body.len() + chunk.len()).map_or(true, |total| total > limit) {
                return Err(StorageError::TooLarge { limit });
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Bytes::from(body))
    }

    /// The object's size, or `None` when it does not exist.
    pub async fn head(&self, bucket: Bucket, key: &str) -> Result<Option<u64>, StorageError> {
        let action = HeadObject::new(self.bucket(bucket), Some(&self.credentials), key);
        let response = self.http.head(action.sign(REQUEST_TTL)).send().await?;
        match response.status() {
            StatusCode::NOT_FOUND => Ok(None),
            status if status.is_success() => Ok(response
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse().ok())),
            status => Err(StorageError::Status { status, body: String::new() }),
        }
    }

    /// Deleting a missing object succeeds.
    pub async fn delete(&self, bucket: Bucket, key: &str) -> Result<(), StorageError> {
        let action = DeleteObject::new(self.bucket(bucket), Some(&self.credentials), key);
        self.send(HttpMethod::DELETE, action.sign(REQUEST_TTL), &[], Bytes::new()).await?;
        Ok(())
    }

    /// Dev/test only: creates both buckets if missing and makes the public one anonymously readable. The private
    /// bucket gets CORS so a browser can PUT parts to presigned URLs and read presigned GETs; the origins come from
    /// `DEV_CORS_ORIGINS` (comma-separated).
    pub async fn ensure_dev_buckets(&self) -> Result<(), StorageError> {
        for bucket in [&self.public, &self.private] {
            let action = CreateBucket::new(bucket, &self.credentials);
            let response = self.http.put(action.sign(REQUEST_TTL)).send().await?;
            let status = response.status();
            if !status.is_success() && status != StatusCode::CONFLICT {
                let body = response.text().await.unwrap_or_default();
                return Err(StorageError::Status { status, body });
            }
        }
        let name = self.public.name();
        let policy = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":["*"]}},"Action":["s3:GetObject"],"Resource":["arn:aws:s3:::{name}/*"]}}]}}"#
        );
        self.put_subresource(&self.public, "policy", "application/json", policy).await?;
        let origins = std::env::var("DEV_CORS_ORIGINS").unwrap_or_else(|_| DEV_CORS_ORIGINS.to_owned());
        self.put_subresource(&self.private, "cors", "application/xml", cors_xml(&origins)).await
    }

    /// `PUT /bucket?<subresource>`, signed in the query string like the other presigned requests.
    async fn put_subresource(
        &self,
        bucket: &S3Bucket,
        subresource: &str,
        content_type: &str,
        body: String,
    ) -> Result<(), StorageError> {
        let url = bucket.base_url().clone();
        let signed = signing::sign(
            &jiff::Timestamp::now(),
            Method::Put,
            url,
            self.credentials.key(),
            self.credentials.secret(),
            None,
            bucket.region(),
            REQUEST_TTL.as_secs(),
            std::iter::once((subresource, "")),
            std::iter::empty(),
        );
        self.send(HttpMethod::PUT, signed, &[("content-type", content_type)], Bytes::from(body)).await?;
        Ok(())
    }

    async fn send(
        &self,
        method: HttpMethod,
        url: Url,
        headers: &[(&str, &str)],
        body: Bytes,
    ) -> Result<reqwest::Response, StorageError> {
        let mut request = self.http.request(method, url).body(body);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = request.send().await?;
        let status = response.status();
        if status.is_success() {
            Ok(response)
        } else {
            Err(StorageError::Status { status, body: response.text().await.unwrap_or_default() })
        }
    }
}

fn cors_xml(origins: &str) -> String {
    let origins: String = origins
        .split(',')
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .map(|origin| format!("<AllowedOrigin>{}</AllowedOrigin>", xml_escape(origin)))
        .collect();
    format!(
        "<CORSConfiguration><CORSRule>{origins}<AllowedMethod>PUT</AllowedMethod><AllowedMethod>GET</AllowedMethod>\
         <AllowedHeader>*</AllowedHeader><ExposeHeader>ETag</ExposeHeader></CORSRule></CORSConfiguration>"
    )
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Where a raw upload lands, in the private bucket.
pub fn upload_key(tenant: Uuid, asset: Uuid) -> String {
    format!("t/{tenant}/a/{asset}/upload")
}

/// The lossless archive of an asset, in the private bucket.
pub fn archive_key(tenant: Uuid, asset: Uuid, ext: &str) -> String {
    format!("t/{tenant}/a/{asset}/archive.{ext}")
}

/// A delivered rendition: in the public bucket for a gallery photo, in the private bucket otherwise.
pub fn rendition_key(tenant: Uuid, asset: Uuid, size: &str, format: &str) -> String {
    format!("t/{tenant}/a/{asset}/{size}.{format}")
}

/// An invoice's or credit note's PDF, in the private bucket.
pub fn invoice_pdf_key(tenant: Uuid, invoice: Uuid) -> String {
    format!("t/{tenant}/invoices/{invoice}.pdf")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_deterministic_and_scoped_by_tenant() {
        let (t, a) = (Uuid::nil(), Uuid::max());
        assert_eq!(upload_key(t, a), format!("t/{t}/a/{a}/upload"));
        assert_eq!(archive_key(t, a, "jxl"), format!("t/{t}/a/{a}/archive.jxl"));
        assert_eq!(rendition_key(t, a, "md", "avif"), format!("t/{t}/a/{a}/md.avif"));
        assert_eq!(invoice_pdf_key(t, a), format!("t/{t}/invoices/{a}.pdf"));
    }

    #[test]
    fn debug_output_hides_credentials() {
        let cfg = StorageConfig::from_lookup(
            |name| match name {
                "S3_ACCESS_KEY" => Some("AKIDEXAMPLE".into()),
                "S3_SECRET_KEY" => Some("s3cr3t-value".into()),
                _ => None,
            },
            Some(()),
        )
        .unwrap();
        let shown = format!("{cfg:?}");
        assert!(!shown.contains("AKIDEXAMPLE") && !shown.contains("s3cr3t-value"), "{shown}");
    }

    #[tokio::test]
    async fn transport_errors_do_not_print_the_signed_url() {
        let mut cfg = StorageConfig::dev().unwrap();
        cfg.endpoint = "http://127.0.0.1:1".parse().unwrap();
        let storage = Storage::new(cfg).unwrap();
        let err = storage.get(Bucket::Private, "k").await.unwrap_err();
        assert!(matches!(err, StorageError::Transport(_)));
        let shown = format!("{err} {err:?}");
        assert!(!shown.contains("X-Amz-Signature") && !shown.contains("X-Amz-Credential"), "{shown}");
    }

    #[tokio::test]
    async fn get_refuses_objects_over_the_cap() {
        let storage = Storage::new(StorageConfig::dev().unwrap()).unwrap();
        storage.ensure_dev_buckets().await.expect("start RustFS: deploy/dev/rustfs.sh start");
        let key = format!("test/{}", Uuid::now_v7());
        storage.put(Bucket::Private, &key, vec![0; 100], "application/octet-stream", None).await.unwrap();
        let refused = storage.get_capped(Bucket::Private, &key, 10).await;
        let allowed = storage.get_capped(Bucket::Private, &key, 100).await;
        storage.delete(Bucket::Private, &key).await.unwrap();
        assert!(matches!(refused, Err(StorageError::TooLarge { limit: 10 })));
        assert_eq!(allowed.unwrap().len(), 100);
    }

    #[test]
    fn from_env_requires_every_variable_but_path_style() {
        let err = StorageConfig::from_lookup(|_| None, None).unwrap_err();
        assert!(err.to_string().contains("is not set"));
    }

    #[test]
    fn cors_lists_each_origin_once() {
        let xml = cors_xml("http://a.test, http://b.test,");
        assert_eq!(xml.matches("<AllowedOrigin>").count(), 2);
        assert!(xml.contains("<ExposeHeader>ETag</ExposeHeader>"));
    }
}
