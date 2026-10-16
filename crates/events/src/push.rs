use crate::bus::BusError;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Pub/Sub's push envelope, used by `PushBus` and by every receiver.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushEnvelope {
    pub message: PushMessage,
    pub subscription: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushMessage {
    /// Base64 of the JSON payload.
    pub data: String,
    pub message_id: String,
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
}

impl PushEnvelope {
    /// Decodes the base64 JSON payload.
    pub fn decode<T: DeserializeOwned>(&self) -> Result<T, BusError> {
        let bytes =
            STANDARD.decode(&self.message.data).map_err(|e| BusError::Decode(format!("data is not base64: {e}")))?;
        serde_json::from_slice(&bytes).map_err(|e| BusError::Decode(format!("data is not the expected JSON: {e}")))
    }

    /// The id receivers dedupe on: the outbox row id carried in the `message_id` attribute. Pub/Sub assigns a new
    /// `messageId` to every publish of the same row, so only the attribute is stable across redeliveries.
    pub fn dedupe_id(&self) -> &str {
        self.message.attributes.get("message_id").map_or(&self.message.message_id, |id| id)
    }
}
