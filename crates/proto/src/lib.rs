//! The gRPC contract between core-api and media-svc, plus the standard health service.

pub use tonic_health;

pub mod media {
    tonic::include_proto!("goodfolk.media.v1");
}

#[cfg(test)]
mod tests {
    use prost::Message;

    use super::media::{FinalizeResponse, Verified, finalize_response::Result as Outcome};

    #[test]
    fn finalize_response_round_trips() {
        let reply = FinalizeResponse {
            result: Some(Outcome::Verified(Verified {
                blake3: vec![7; 32],
                mime: "image/jpeg".into(),
                bytes: 8 * 1024 * 1024 + 1024,
                width: 4000,
                height: 3000,
            })),
        };
        let decoded = FinalizeResponse::decode(reply.encode_to_vec().as_slice()).unwrap();
        assert_eq!(decoded, reply);
    }
}
