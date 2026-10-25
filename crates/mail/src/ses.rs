use std::time::{Duration, SystemTime};

use anyhow::{Context, bail};
use aws_sigv4::http_request::{SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4;
use reqwest::header::{CONTENT_TYPE, HeaderName, HeaderValue};
use serde_json::json;

use crate::{MailError, Mailer, Message};

/// The service name SES v2 is signed for.
const SERVICE: &str = "ses";
const TIMEOUT: Duration = Duration::from_secs(30);

/// An AWS access key. `Debug` shows only the key id.
#[derive(Clone)]
pub struct Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    /// For temporary credentials.
    pub session_token: Option<String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials").field("access_key_id", &self.access_key_id).finish_non_exhaustive()
    }
}

/// Production: SES v2 `SendEmail` (`POST /v2/email/outbound-emails`) with `Content.Simple`, signed with SigV4.
#[derive(Debug)]
pub struct SesMailer {
    region: String,
    from: String,
    credentials: Credentials,
    /// `https://email.<region>.amazonaws.com`, or a test server.
    endpoint: String,
    http: reqwest::Client,
}

impl SesMailer {
    /// Sends from `from` (an address SES has verified) through `region`'s endpoint.
    pub fn new(region: &str, from: &str, credentials: Credentials) -> anyhow::Result<Self> {
        if region.is_empty() || !region.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
            bail!("AWS_REGION is not a region name");
        }
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder().timeout(TIMEOUT).build().context("building the HTTP client")?;
        Ok(Self {
            region: region.to_owned(),
            from: from.to_owned(),
            credentials,
            endpoint: format!("https://email.{region}.amazonaws.com"),
            http,
        })
    }

    /// The same, sending to `endpoint` (scheme, host and port) instead of AWS. For tests.
    pub fn with_endpoint(mut self, endpoint: &str) -> Self {
        endpoint.trim_end_matches('/').clone_into(&mut self.endpoint);
        self
    }
}

#[async_trait::async_trait]
impl Mailer for SesMailer {
    async fn send(&self, message: &Message) -> Result<(), MailError> {
        let content = |data: &str| json!({ "Data": data, "Charset": "UTF-8" });
        let body = json!({
            "FromEmailAddress": self.from,
            "Destination": { "ToAddresses": [message.to] },
            "Content": { "Simple": {
                "Subject": content(&message.subject),
                "Body": { "Text": content(&message.text), "Html": content(&message.html) },
            } },
        });
        let bytes = serde_json::to_vec(&body).map_err(|error| MailError::Permanent(error.to_string()))?;
        let mut request = self
            .http
            .post(format!("{}/v2/email/outbound-emails", self.endpoint))
            .header(CONTENT_TYPE, "application/json")
            .body(bytes)
            .build()
            .map_err(|error| MailError::Permanent(format!("building the request: {}", error.without_url())))?;
        sign(&mut request, &self.credentials, &self.region, SERVICE, SystemTime::now())?;
        let response = self
            .http
            .execute(request)
            .await
            .map_err(|error| MailError::Retryable(format!("SES request failed: {}", error.without_url())))?;
        let status = response.status();
        if status.is_success() {
            tracing::info!(to = %message.to, subject = %message.subject, "mail sent through SES");
            return Ok(());
        }
        // `x-amzn-ErrorType` is `<Name>:<namespace>`; the name is enough to tell a throttle or a pause.
        let kind = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(':').next())
            .unwrap_or("")
            .to_owned();
        let detail = format!("SES answered {status} {kind}");
        // A throttle, a daily sending limit reached or the account's sending paused all clear in time; a 403 is
        // usually credentials or permissions being fixed. Dropping an invitation for any of them would lose it.
        let throttled = status.as_u16() == 429
            || kind.contains("Throttl")
            || kind.contains("TooManyRequests")
            || kind.contains("LimitExceeded")
            || kind.contains("SendingPaused");
        if throttled || status == reqwest::StatusCode::FORBIDDEN || status.is_server_error() {
            Err(MailError::Retryable(detail))
        } else {
            Err(MailError::Permanent(detail))
        }
    }
}

/// Signs `request` with SigV4 for `service` in `region` at `time`: adds `x-amz-date`, `authorization` and, for
/// temporary credentials, `x-amz-security-token`. The host comes from the URL; every header already set is signed.
pub fn sign(
    request: &mut reqwest::Request,
    credentials: &Credentials,
    region: &str,
    service: &str,
    time: SystemTime,
) -> Result<(), MailError> {
    let failed = |error: &dyn std::fmt::Display| MailError::Permanent(format!("signing the request: {error}"));
    let identity = aws_credential_types::Credentials::new(
        &credentials.access_key_id,
        &credentials.secret_access_key,
        credentials.session_token.clone(),
        None,
        "goodfolk-mail",
    )
    .into();
    let params = v4::SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name(service)
        .time(time)
        .settings(SigningSettings::default())
        .build()
        .map_err(|error| failed(&error))?
        .into();
    let headers: Vec<(&str, &str)> = request
        .headers()
        .iter()
        .map(|(name, value)| Ok((name.as_str(), value.to_str()?)))
        .collect::<Result<_, reqwest::header::ToStrError>>()
        .map_err(|error| failed(&error))?;
    let body = request.body().and_then(|body| body.as_bytes()).unwrap_or_default();
    let signable = SignableRequest::new(
        request.method().as_str(),
        request.url().as_str(),
        headers.into_iter(),
        SignableBody::Bytes(body),
    )
    .map_err(|error| failed(&error))?;
    let (instructions, _signature) =
        aws_sigv4::http_request::sign(signable, &params).map_err(|error| failed(&error))?.into_parts();
    let (signed, _params) = instructions.into_parts();
    for header in signed {
        let name = HeaderName::from_bytes(header.name().as_bytes()).map_err(|error| failed(&error))?;
        let value = HeaderValue::from_str(header.value()).map_err(|error| failed(&error))?;
        request.headers_mut().insert(name, value);
    }
    Ok(())
}
