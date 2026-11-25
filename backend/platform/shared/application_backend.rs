use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

pub const PORT_MIN: u16 = 42000;
pub const PORT_MAX: u16 = 42999;
pub const PROJECT: &str = "scheduler-app-backend";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Ensure,
    Status,
    Remove,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Instance {
    pub id: String,
    pub host: Ipv4Addr,
    pub image: String,
    pub container_port: u16,
    pub host_port: u16,
    pub nvidia: bool,
    pub cpu_millicores: Option<u32>,
    pub memory_mib: Option<u32>,
    pub config_b64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_token: Option<String>,
    pub operation: Operation,
}

impl Instance {
    pub fn name(&self) -> String {
        format!("vs-backend-{}", self.id)
    }

    pub fn valid(&self) -> bool {
        self.id.len() == 32
            && self
                .id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            && self.container_port != 0
            && (PORT_MIN..=PORT_MAX).contains(&self.host_port)
            && image_repository(&self.image).is_some()
            && valid_limits(self.cpu_millicores, self.memory_mib)
            && self.channel_token.as_deref().is_none_or(valid_channel_token)
            && self.config_b64.as_ref().is_none_or(|value| {
                value.len() <= 10924
                    && value.len() % 4 == 0
                    && value
                        .trim_end_matches('=')
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
                    && value.bytes().rev().take_while(|&b| b == b'=').count() <= 2
            })
    }
}

pub fn valid_channel_token(token: &str) -> bool {
    token.len() == 64
        && token.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

pub fn valid_limits(cpu: Option<u32>, memory: Option<u32>) -> bool {
    cpu.is_none_or(|value| (100..=128_000).contains(&value))
        && memory.is_none_or(|value| (16..=1_048_576).contains(&value))
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeState {
    Pending,
    Running,
    Stopped,
    Failed,
    Absent,
}

fn repository_valid(repository: &str) -> bool {
    let Some((registry, path)) = repository.split_once('/') else {
        return false;
    };
    if !(registry.contains('.') || registry.contains(':') || registry == "localhost") {
        return false;
    }
    registry
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".:-".contains(&b))
        && path.split('/').all(|part| {
            part.bytes()
                .next()
                .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                && part
                    .bytes()
                    .last()
                    .is_some_and(|b| b.is_ascii_alphanumeric())
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
        })
}

pub fn image_repository(image: &str) -> Option<&str> {
    if image.len() > 512 {
        return None;
    }
    let (name, digest) = match image.split_once('@') {
        Some((name, hash)) => {
            let hash = hash.strip_prefix("sha256:")?;
            if hash.len() != 64
                || !hash
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            {
                return None;
            }
            (name, true)
        }
        None => (image, false),
    };
    let path = name.split_once('/')?.1;
    let repository = if let Some((_, tag)) = path.rsplit_once(':') {
        if tag.is_empty()
            || tag.len() > 128
            || !tag.as_bytes()[0].is_ascii_alphanumeric() && !tag.starts_with('_')
            || !tag
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return None;
        }
        &name[..name.len() - tag.len() - 1]
    } else if digest {
        name
    } else {
        return None;
    };
    repository_valid(repository).then_some(repository)
}

pub fn approved_repository(value: &str) -> bool {
    value.len() <= 383 && repository_valid(value)
}
