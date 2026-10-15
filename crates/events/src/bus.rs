use crate::push::{PushEnvelope, PushMessage};
use anyhow::{Context, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures::future::BoxFuture;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Message {
    pub id: Uuid,
    pub topic: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, thiserror::Error)]
pub enum BusError {
    /// The receiver will never accept this message; retrying cannot help.
    #[error("refused: {0}")]
    Permanent(String),
    /// A failure worth retrying.
    #[error("{0}")]
    Retryable(String),
    #[error("{0}")]
    Decode(String),
}

#[async_trait::async_trait]
pub trait EventBus: Send + Sync + 'static {
    async fn publish(&self, msg: &Message) -> Result<(), BusError>;
}

/// Produces the full `Authorization` header value for a push, so a service-auth crate can plug in without this crate
/// depending on it.
pub type HeaderProvider = Arc<dyn Fn() -> BoxFuture<'static, Result<String, BusError>> + Send + Sync>;

/// reqwest is built without a bundled TLS provider, so one must be installed before any client is built. This
/// installs ring, which the tree already has; another component having installed a provider first is fine.
pub fn install_tls_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn client(timeout: Duration) -> anyhow::Result<reqwest::Client> {
    install_tls_provider();
    reqwest::Client::builder().timeout(timeout).build().context("building the HTTP client")
}

/// Never includes the URL: it may carry credentials or internal hostnames.
fn transport_error(error: reqwest::Error) -> BusError {
    BusError::Retryable(format!("request failed: {}", error.without_url()))
}

/// Delivers straight to the receiver's endpoint in Pub/Sub's push format. For local development and tests only.
/// The envelope's `subscription` is a synthetic `push/{topic}`; receivers must not parse it.
pub struct PushBus {
    endpoints: HashMap<String, Url>,
    http: reqwest::Client,
    auth: Option<HeaderProvider>,
}

impl PushBus {
    pub fn new(endpoints: HashMap<String, Url>, auth: Option<HeaderProvider>) -> anyhow::Result<Self> {
        Ok(Self { endpoints, http: client(Duration::from_secs(600))?, auth })
    }

    /// Reads `PUSH_ENDPOINT_<TOPIC_UPPER_SNAKE>` variables, e.g. `PUSH_ENDPOINT_MEDIA_PROCESS` for `media-process`.
    pub fn from_env(auth: Option<HeaderProvider>) -> anyhow::Result<Self> {
        let mut endpoints = HashMap::new();
        for (key, value) in std::env::vars() {
            if let Some(suffix) = key.strip_prefix("PUSH_ENDPOINT_") {
                let topic = suffix.to_ascii_lowercase().replace('_', "-");
                let url = Url::parse(&value).with_context(|| format!("{key} is not a URL"))?;
                endpoints.insert(topic, url);
            }
        }
        Self::new(endpoints, auth)
    }
}

#[async_trait::async_trait]
impl EventBus for PushBus {
    async fn publish(&self, msg: &Message) -> Result<(), BusError> {
        let url = self
            .endpoints
            .get(&msg.topic)
            .ok_or_else(|| BusError::Retryable(format!("no push endpoint configured for topic {}", msg.topic)))?;
        let envelope = PushEnvelope {
            message: PushMessage {
                data: STANDARD.encode(serde_json::to_vec(&msg.payload).map_err(|e| BusError::Decode(e.to_string()))?),
                message_id: msg.id.to_string(),
                attributes: BTreeMap::from([("message_id".to_owned(), msg.id.to_string())]),
            },
            subscription: format!("push/{}", msg.topic),
        };
        let mut request = self.http.post(url.clone()).json_body(&envelope)?;
        if let Some(auth) = &self.auth {
            request = request.header(reqwest::header::AUTHORIZATION, auth().await?);
        }
        let status = request.send().await.map_err(transport_error)?.status();
        if status.is_success() {
            Ok(())
        } else if status.is_client_error() && !matches!(status.as_u16(), 401 | 403 | 408 | 429) {
            // 401 and 403 are retried: a misconfigured or rotated service credential must not lose messages.
            Err(BusError::Permanent(format!("{} answered {status}", msg.topic)))
        } else {
            Err(BusError::Retryable(format!("{} answered {status}", msg.topic)))
        }
    }
}

/// `reqwest` is built without its `json` feature; this sets the body and content type by hand.
trait JsonBody: Sized {
    fn json_body<T: serde::Serialize>(self, body: &T) -> Result<Self, BusError>;
}

impl JsonBody for reqwest::RequestBuilder {
    fn json_body<T: serde::Serialize>(self, body: &T) -> Result<Self, BusError> {
        let bytes = serde_json::to_vec(body).map_err(|e| BusError::Decode(e.to_string()))?;
        Ok(self.header(reqwest::header::CONTENT_TYPE, "application/json").body(bytes))
    }
}

const METADATA_TOKEN_URL: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";
/// A cached token is renewed this long before it expires.
const TOKEN_MARGIN: Duration = Duration::from_secs(300);

/// Access tokens from the GCE metadata server, cached until five minutes before they expire.
struct MetadataToken {
    http: reqwest::Client,
    url: String,
    cached: tokio::sync::Mutex<Option<(String, Instant)>>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

impl MetadataToken {
    fn new(url: &str) -> anyhow::Result<Self> {
        Ok(Self { http: client(Duration::from_secs(10))?, url: url.to_owned(), cached: tokio::sync::Mutex::new(None) })
    }

    async fn get(&self) -> Result<String, BusError> {
        let mut cached = self.cached.lock().await;
        if let Some((token, expires)) = cached.as_ref()
            && Instant::now() < *expires
        {
            return Ok(token.clone());
        }
        let response =
            self.http.get(&self.url).header("Metadata-Flavor", "Google").send().await.map_err(transport_error)?;
        if !response.status().is_success() {
            return Err(BusError::Retryable(format!("metadata server answered {}", response.status())));
        }
        let body = response.bytes().await.map_err(transport_error)?;
        let parsed: TokenResponse = serde_json::from_slice(&body)
            .map_err(|_| BusError::Retryable("metadata token was malformed".to_owned()))?;
        let lifetime = Duration::from_secs(parsed.expires_in).saturating_sub(TOKEN_MARGIN);
        *cached = Some((parsed.access_token.clone(), Instant::now() + lifetime));
        Ok(parsed.access_token)
    }
}

/// Publishes to Google Cloud Pub/Sub, or to the emulator when `PUBSUB_EMULATOR_HOST` is set.
pub struct PubSubBus {
    project: String,
    base_url: String,
    http: reqwest::Client,
    token: Option<MetadataToken>,
}

impl PubSubBus {
    /// Production: the Pub/Sub API, authenticated with the metadata server's access token.
    pub fn google(project: &str) -> anyhow::Result<Self> {
        Self::with_token_url(project, "https://pubsub.googleapis.com", Some(METADATA_TOKEN_URL))
    }

    /// The emulator at `host` (`host:port`), which takes no authentication.
    pub fn emulator(project: &str, host: &str) -> anyhow::Result<Self> {
        Self::with_token_url(project, &format!("http://{host}"), None)
    }

    /// `token_url` is the metadata endpoint to fetch tokens from, or `None` for no authentication.
    pub fn with_token_url(project: &str, base_url: &str, token_url: Option<&str>) -> anyhow::Result<Self> {
        Ok(Self {
            project: project.to_owned(),
            base_url: base_url.trim_end_matches('/').to_owned(),
            http: client(Duration::from_secs(30))?,
            token: token_url.map(MetadataToken::new).transpose()?,
        })
    }
}

#[async_trait::async_trait]
impl EventBus for PubSubBus {
    async fn publish(&self, msg: &Message) -> Result<(), BusError> {
        let url = format!("{}/v1/projects/{}/topics/{}:publish", self.base_url, self.project, msg.topic);
        let data = STANDARD.encode(serde_json::to_vec(&msg.payload).map_err(|e| BusError::Decode(e.to_string()))?);
        let body = serde_json::json!({
            "messages": [{ "data": data, "attributes": { "message_id": msg.id.to_string() } }]
        });
        let mut request = self.http.post(url).json_body(&body)?;
        if let Some(token) = &self.token {
            request = request.bearer_auth(token.get().await?);
        }
        let status = request.send().await.map_err(transport_error)?.status();
        if status.is_success() {
            Ok(())
        } else {
            // Every failure is retried: a missing topic or a revoked grant is an operator error that clears
            // once fixed, and dropping the message would lose it.
            Err(BusError::Retryable(format!("pubsub answered {status} for {}", msg.topic)))
        }
    }
}

/// `EVENT_BUS=push` (refused when `APP_ENV=production`, and needs `auth`) or `EVENT_BUS=pubsub`, which ignores `auth`, needs `PUBSUB_PROJECT` and uses
/// `PUBSUB_EMULATOR_HOST` when set.
pub fn bus_from_env(auth: Option<HeaderProvider>) -> anyhow::Result<Arc<dyn EventBus>> {
    let kind = std::env::var("EVENT_BUS").context("EVENT_BUS is required (push or pubsub)")?;
    match kind.as_str() {
        "push" => {
            if std::env::var("APP_ENV").is_ok_and(|env| env == "production") {
                bail!("EVENT_BUS=push is not allowed when APP_ENV=production");
            }
            let Some(auth) = auth else { bail!("EVENT_BUS=push needs service auth") };
            Ok(Arc::new(PushBus::from_env(Some(auth))?))
        }
        "pubsub" => {
            let project = std::env::var("PUBSUB_PROJECT").context("PUBSUB_PROJECT is required")?;
            match std::env::var("PUBSUB_EMULATOR_HOST") {
                Ok(host) if !host.is_empty() => Ok(Arc::new(PubSubBus::emulator(&project, &host)?)),
                _ => Ok(Arc::new(PubSubBus::google(&project)?)),
            }
        }
        other => bail!("EVENT_BUS must be push or pubsub, not {other}"),
    }
}
