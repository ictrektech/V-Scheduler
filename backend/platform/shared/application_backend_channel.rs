use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

pub const MAX_CHUNK_BYTES: usize = 1024 * 1024;
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    CreateTransfer,
    ReadTransfer { transfer_id: String },
    WriteChunk { transfer_id: String, file_id: String, offset: u64 },
    CompleteTransfer { transfer_id: String },
    DeleteModel { model_id: String },
    Capabilities,
    ModelHub,
    Execute { attempt_id: String, action: Action },
    WriteReference { attempt_id: String, offset: u64 },
    ReadOutput { attempt_id: String, file_id: String, offset: u64 },

}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action { Start, Inspect, Cancel }

impl Action {
    pub fn segment(self) -> &'static str {
        match self { Self::Start => "start", Self::Inspect => "inspect", Self::Cancel => "cancel" }
    }
}

impl Operation {
    pub fn timeout_seconds(&self) -> u64 {
        if matches!(self, Self::CompleteTransfer { .. }) { 280 } else { 25 }
    }

    pub fn max_reply_bytes(&self) -> usize {
        if matches!(self, Self::ReadOutput{ .. }) { MAX_CHUNK_BYTES.div_ceil(3)*4 + 4096 } else { MAX_MANIFEST_BYTES }
    }

    pub fn request(&self, body_len: usize) -> Option<(&'static str, String, Option<&'static str>)> {
        match self {
            Self::CreateTransfer if (1..=MAX_MANIFEST_BYTES).contains(&body_len) => {
                Some(("POST", "/scheduler/transfers".into(), Some("application/json")))
            }
            Self::ReadTransfer { transfer_id } if identifier(transfer_id) && body_len == 0 => {
                Some(("GET", format!("/scheduler/transfers/{transfer_id}"), None))
            }
            Self::WriteChunk { transfer_id, file_id, offset }
                if identifier(transfer_id)
                    && identifier(file_id)
                    && (1..=MAX_CHUNK_BYTES).contains(&body_len)
                    && offset.checked_add(body_len as u64).is_some() =>
            {
                Some((
                    "PUT",
                    format!("/scheduler/transfers/{transfer_id}/files/{file_id}?offset={offset}"),
                    Some("application/octet-stream"),
                ))
            }
            Self::CompleteTransfer { transfer_id } if identifier(transfer_id) && body_len == 0 => {
                Some(("POST", format!("/scheduler/transfers/{transfer_id}/complete"), None))
            }
            Self::DeleteModel { model_id } if model_identifier(model_id) && body_len == 0 => {
                Some(("DELETE", format!("/scheduler/transfers/models/{model_id}"), None))
            }
            Self::Capabilities if body_len == 0 => Some(("GET", "/scheduler/capabilities".into(), None)),
            Self::ModelHub if body_len == 0 => Some(("GET", "/scheduler/model-hub".into(), None)),
            Self::Execute { attempt_id, action } if identifier(attempt_id)
                && (1..=MAX_MANIFEST_BYTES).contains(&body_len) => Some((
                    "POST", format!("/scheduler/executions/{attempt_id}/{}", action.segment()), Some("application/json"),
                )),
            Self::WriteReference { attempt_id, offset } if identifier(attempt_id)
                && (1..=MAX_CHUNK_BYTES).contains(&body_len)
                && offset.checked_add(body_len as u64).is_some_and(|end| end <= 20 * 1024 * 1024) => Some((
                    "PUT", format!("/scheduler/executions/{attempt_id}/inputs/reference?offset={offset}"),
                    Some("application/octet-stream"),
                )),
            Self::ReadOutput { attempt_id, file_id, offset } if identifier(attempt_id) && identifier(file_id)
                && body_len == 0 => Some(("GET", format!("/scheduler/executions/{attempt_id}/outputs/{file_id}?offset={offset}"), None)),
            _ => None,
        }
    }
}

pub fn identifier(value: &str) -> bool {
    (1..=96).contains(&value.len())
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

pub fn model_identifier(value: &str) -> bool {
    let valid = |part: &str| {
        identifier(part) && part.bytes().all(|byte| !byte.is_ascii_uppercase())
    };
    match value.split_once('@') {
        Some((model, variant)) => valid(model) && valid(variant),
        None => valid(value),
    }
}

// A private request sent only through the node managers' authenticated control plane.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Packet {
    pub deployment_id: String,
    pub host: Ipv4Addr,
    pub host_port: u16,
    pub token: String,
    pub operation: Operation,
    pub body_b64: String,
}

impl Packet {
    pub fn valid_target(&self) -> bool {
        let lower_hex = |value: &str, length| {
            value.len() == length && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        lower_hex(&self.deployment_id, 32)
            && (42000..=42999).contains(&self.host_port)
            && lower_hex(&self.token, 64)
            && self.body_b64.len() <= MAX_CHUNK_BYTES.div_ceil(3) * 4
    }
}

#[derive(Deserialize, Serialize)]
pub struct Reply {
    pub status: u16,
    pub data: serde_json::Value,
}

pub fn shared_unavailable(status: u16, body: &[u8]) -> serde_json::Value {
    if status == 409
        && serde_json::from_slice::<serde_json::Value>(body)
            .is_ok_and(|value| value["detail"] == "WEIGHT_SHARED_UNAVAILABLE")
    {
        serde_json::json!({"detail":"WEIGHT_SHARED_UNAVAILABLE"})
    } else {
        serde_json::Value::Null
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_model_only_addresses_a_literal_model_key() {
        let request = |model_id: &str, size| Operation::DeleteModel { model_id: model_id.into() }.request(size);
        assert_eq!(request("sd-turbo@fp16", 0), Some(("DELETE", "/scheduler/transfers/models/sd-turbo@fp16".into(), None)));
        assert!(request("sd-turbo@../other", 0).is_none());
        assert!(request("sd-turbo", 1).is_none());
    }

    #[test]
    fn permits_only_bounded_chunks_and_literal_resource_ids() {
        let chunk = |file_id: &str, offset| Operation::WriteChunk {
            transfer_id: "transfer-01".into(),
            file_id: file_id.into(),
            offset,
        };
        assert_eq!(
            chunk("blob-02", 16).request(MAX_CHUNK_BYTES),
            Some((
                "PUT",
                "/scheduler/transfers/transfer-01/files/blob-02?offset=16".into(),
                Some("application/octet-stream"),
            ))
        );
        assert!(chunk("blob-02", 0).request(MAX_CHUNK_BYTES + 1).is_none());
        assert!(chunk("blob-02", u64::MAX).request(1).is_none());
        for id in ["", "..", "../weights", "a%2fb", "a?offset=9", "a'\n"] {
            assert!(chunk(id, 0).request(1).is_none());
        }
    }

    #[test]
    fn rejects_bodies_on_read_and_completion() {
        for operation in [Operation::ReadTransfer { transfer_id: "t".into() },
            Operation::CompleteTransfer { transfer_id: "t".into() }] {
            assert!(operation.request(0).is_some());
            assert!(operation.request(1).is_none());
        }
        assert!(Operation::CreateTransfer.request(0).is_none());
        assert!(Operation::CreateTransfer.request(MAX_MANIFEST_BYTES + 1).is_none());
    }
}
