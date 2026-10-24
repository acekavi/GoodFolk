use crate::events::LiveEvent;
use crate::graphql::{GqlSchema, build_schema};
use crate::persisted;
use anyhow::Context;
use db::crypto::GuestIdKeys;
use proto::jobs::jobs_client::JobsClient;
use proto::media::media_client::MediaClient;
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use storage::{Storage, StorageConfig};
use svc_auth::{TokenSource, Verifier};
use tokio::sync::broadcast;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use uuid::Uuid;

/// How long connecting to media-svc or jobs-svc may take.
const SVC_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// media-svc's gRPC address in development.
pub(crate) const LOCAL_MEDIA_SVC_URL: &str = "http://localhost:8090";
/// jobs-svc's gRPC address in development.
pub(crate) const LOCAL_JOBS_SVC_URL: &str = "http://localhost:8091";

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub schema: GqlSchema,
    /// Fan-out of database change events to this instance's SSE subscribers.
    pub events: broadcast::Sender<LiveEvent>,
    /// Production sets `Secure` cookies and disables GraphQL introspection.
    pub production: bool,
    /// Seals guest ID numbers under the current key; opens one sealed under it or a retired key.
    pub guest_id_keys: Arc<GuestIdKeys>,
    /// Object storage: uploads, archives and renditions. core-api signs links and deletes objects; it never moves
    /// a file's bytes.
    pub storage: Arc<Storage>,
    /// media-svc's gRPC client, over a channel that connects on first use.
    pub media: MediaClient<Channel>,
    /// The `Authorization` sent with every call to media-svc.
    pub media_auth: Arc<TokenSource>,
    /// Recognises media-svc's calls to `/internal/events`.
    pub internal_auth: Arc<Verifier>,
    /// jobs-svc's gRPC client, which starts and retries the night-audit close, over a channel that connects on first
    /// use.
    pub jobs: JobsClient<Channel>,
    /// The `Authorization` sent with every call to jobs-svc.
    pub jobs_auth: Arc<TokenSource>,
}

/// The media engine's services, as [`AppState::with_media`] takes them.
pub struct MediaServices {
    pub storage: Arc<Storage>,
    pub media: MediaClient<Channel>,
    pub media_auth: Arc<TokenSource>,
    pub internal_auth: Arc<Verifier>,
}

impl MediaServices {
    /// Reaches media-svc at `media_svc_url` (gRPC), connecting on first use. Must run inside a Tokio runtime.
    pub fn new(
        storage: Arc<Storage>,
        media_svc_url: &str,
        media_auth: Arc<TokenSource>,
        internal_auth: Arc<Verifier>,
    ) -> anyhow::Result<Self> {
        let channel = svc_channel(media_svc_url, "MEDIA_SVC_URL")?;
        Ok(Self { storage, media: MediaClient::new(channel), media_auth, internal_auth })
    }

    /// The local RustFS (or the `S3_*` variables) and media-svc on its local port, with a random shared secret both
    /// ways, so no other service can call in and calls out are refused. For tests and tools that never need media-svc.
    pub fn local() -> Self {
        let storage = Storage::new(StorageConfig::dev().expect("the local storage configuration"))
            .expect("the local storage client");
        let secret = format!("{}{}", Uuid::now_v7().simple(), Uuid::now_v7().simple());
        Self::new(
            Arc::new(storage),
            LOCAL_MEDIA_SVC_URL,
            Arc::new(TokenSource::SharedSecret(secret.clone())),
            Arc::new(Verifier::SharedSecret(secret)),
        )
        .expect("the local media-svc URL is valid")
    }
}

/// jobs-svc's client, as [`AppState::with_jobs`] takes it.
pub struct JobsServices {
    pub jobs: JobsClient<Channel>,
    pub jobs_auth: Arc<TokenSource>,
}

impl JobsServices {
    /// Reaches jobs-svc at `jobs_svc_url` (gRPC), connecting on first use. Must run inside a Tokio runtime.
    pub fn new(jobs_svc_url: &str, jobs_auth: Arc<TokenSource>) -> anyhow::Result<Self> {
        let channel = svc_channel(jobs_svc_url, "JOBS_SVC_URL")?;
        Ok(Self { jobs: JobsClient::new(channel), jobs_auth })
    }

    /// jobs-svc on its local port with a random shared secret, so calls out are refused. For tests and tools that
    /// never need jobs-svc.
    pub fn local() -> Self {
        let secret = format!("{}{}", Uuid::now_v7().simple(), Uuid::now_v7().simple());
        Self::new(LOCAL_JOBS_SVC_URL, Arc::new(TokenSource::SharedSecret(secret)))
            .expect("the local jobs-svc URL is valid")
    }
}

/// A lazily connecting channel to the service at `url`, read from the variable `name`. An `https` address speaks TLS,
/// trusting the Mozilla roots: `from_shared` leaves TLS off whatever the scheme, which would send cleartext h2c to
/// port 443.
fn svc_channel(url: &str, name: &str) -> anyhow::Result<Channel> {
    let mut endpoint = Endpoint::from_shared(url.to_owned()).with_context(|| format!("{name} is not a URL"))?;
    if endpoint.uri().scheme() == Some(&axum::http::uri::Scheme::HTTPS) {
        endpoint = endpoint
            .tls_config(ClientTlsConfig::new().with_enabled_roots())
            .with_context(|| format!("{name}: the TLS configuration"))?;
    }
    Ok(endpoint.connect_timeout(SVC_CONNECT_TIMEOUT).connect_lazy())
}

impl AppState {
    /// The state with [`MediaServices::local`] and [`JobsServices::local`]: for tests and tools. `serve` uses
    /// [`AppState::with_media`] and [`AppState::with_jobs`].
    pub fn new(pool: PgPool, production: bool, guest_id_keys: GuestIdKeys) -> Self {
        Self::with_media(pool, production, guest_id_keys, MediaServices::local())
    }

    pub fn with_media(pool: PgPool, production: bool, guest_id_keys: GuestIdKeys, media: MediaServices) -> Self {
        persisted::load();
        let (events, _) = broadcast::channel(1024);
        let MediaServices { storage, media, media_auth, internal_auth } = media;
        let JobsServices { jobs, jobs_auth } = JobsServices::local();
        Self {
            schema: build_schema(production),
            pool,
            events,
            production,
            guest_id_keys: Arc::new(guest_id_keys),
            storage,
            media,
            media_auth,
            internal_auth,
            jobs,
            jobs_auth,
        }
    }

    /// The state calling jobs-svc through `jobs` (it is [`JobsServices::local`] until then).
    pub fn with_jobs(self, jobs: JobsServices) -> Self {
        Self { jobs: jobs.jobs, jobs_auth: jobs.jobs_auth, ..self }
    }
}

#[cfg(test)]
mod tests {
    use super::svc_channel;
    use proto::media::media_client::MediaClient;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    /// The first bytes a client sends to `scheme://localhost:<port>`, which must speak media-svc's gRPC.
    async fn first_bytes(scheme: &str) -> Vec<u8> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut media =
            MediaClient::new(svc_channel(&format!("{scheme}://localhost:{port}"), "MEDIA_SVC_URL").unwrap());
        tokio::spawn(async move { media.create_upload(proto::media::CreateUploadRequest::default()).await });
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = [0u8; 3];
        socket.read_exact(&mut bytes).await.unwrap();
        bytes.to_vec()
    }

    #[tokio::test]
    async fn an_https_media_svc_url_speaks_tls() {
        // A TLS handshake record: content type 22, protocol major version 3.
        assert_eq!(&first_bytes("https").await[..2], &[0x16, 0x03]);
    }

    #[tokio::test]
    async fn an_http_media_svc_url_speaks_cleartext_h2c() {
        assert_eq!(first_bytes("http").await, b"PRI");
    }
}
