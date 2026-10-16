use super::*;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, Write};
#[derive(Default, Clone)]
struct Aggregate {
    totals: Value,
    models: BTreeMap<String, Value>,
    upstreams: BTreeMap<String, Value>,
    users: BTreeMap<String, Value>,
}
pub(super) struct Usage {
    root: PathBuf,
    since: DateTime<Utc>,
    buckets: BTreeMap<String, Aggregate>,
    recent: VecDeque<Value>,
}
fn user_file(user: &str) -> String {
    let prefix = user
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .take(48)
        .collect::<String>();
    let prefix = prefix.trim_matches('.');
    let hash = format!("{:x}", Sha256::digest(user));
    format!(
        "{}-{}.jsonl",
        if prefix.is_empty() { "user" } else { prefix },
        &hash[..12]
    )
}
fn period(at: DateTime<Utc>, kind: &str) -> String {
    use chrono::Datelike;
    let day = at.date_naive();
    let start = match kind {
        "week" => day - chrono::Duration::days(day.weekday().num_days_from_monday() as i64),
        "month" => day.with_day(1).unwrap(),
        _ => day,
    };
    format!("{kind}:{start}")
}
fn add_stat(stats: &mut Value, record: &Value) {
    if !stats.is_object() {
        *stats = json!({"requests":0,"errors":0,"latency_ms_avg":0,"input_tokens":0,"output_tokens":0,"total_tokens":0,"cached_tokens":0,"reasoning_tokens":0,"usage_requests":0,"latency_sum":0});
    }
    for (key, amount) in [
        ("requests", 1),
        (
            "errors",
            if record["status"].as_u64().unwrap_or(200) >= 400 {
                1
            } else {
                0
            },
        ),
        (
            "usage_requests",
            if record["usage_reported"] == true {
                1
            } else {
                0
            },
        ),
        ("latency_sum", record["latency_ms"].as_i64().unwrap_or(0)),
    ] {
        stats[key] = json!(stats[key].as_i64().unwrap_or(0) + amount);
    }
    if record["usage_reported"] == true {
        for key in [
            "input_tokens",
            "output_tokens",
            "total_tokens",
            "cached_tokens",
            "reasoning_tokens",
        ] {
            stats[key] =
                json!(stats[key].as_i64().unwrap_or(0) + record[key].as_i64().unwrap_or(0));
        }
    }
    stats["latency_ms_avg"] =
        json!(stats["latency_sum"].as_i64().unwrap_or(0) / stats["requests"].as_i64().unwrap_or(1));
}
impl Usage {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if path.as_os_str().is_empty() {
            return Ok(Self {
                root: PathBuf::new(),
                since: Utc::now(),
                buckets: Default::default(),
                recent: Default::default(),
            });
        }
        let stem = if path.with_extension("") == path {
            PathBuf::from(format!("{}.d", path.display()))
        } else {
            path.with_extension("")
        };
        let root = stem.join("users");
        use std::os::unix::fs::DirBuilderExt;
        if !root.exists() {
            std::fs::create_dir_all(&stem)?;
            let stage = stem.join(format!(".users-migrating-{}", uuid::Uuid::new_v4()));
            std::fs::DirBuilder::new().mode(0o700).create(&stage)?;
            if path.exists() {
                for line in std::io::BufReader::new(std::fs::File::open(path)?).lines() {
                    let line = line?;
                    if let Ok(record) = serde_json::from_str::<Value>(&line) {
                        if record["time"].is_string() {
                            append(&stage.join(user_file(text(&record, "user"))), &record)?;
                        }
                    }
                }
            }
            std::fs::rename(&stage, &root)?;
            if path.exists() {
                std::fs::rename(path, stem.join("legacy.jsonl"))?;
            }
        }
        let mut result = Self {
            root,
            since: Utc::now(),
            buckets: Default::default(),
            recent: Default::default(),
        };
        let mut records = vec![];
        for entry in std::fs::read_dir(&result.root)? {
            let path = entry?.path();
            if path.extension().is_none_or(|v| v != "jsonl") {
                continue;
            }
            for line in std::io::BufReader::new(std::fs::File::open(path)?).lines() {
                let line = line?;
                if line.len() > 1 << 20 {
                    continue;
                }
                if let Ok(record) = serde_json::from_str::<Value>(&line) {
                    records.push(record);
                }
            }
        }
        records.sort_by(|a, b| text(a, "time").cmp(text(b, "time")));
        for record in records {
            result.add(record);
        }
        Ok(result)
    }
    fn add(&mut self, record: Value) {
        let Ok(at) = DateTime::parse_from_rfc3339(text(&record, "time")) else {
            return;
        };
        let at = at.with_timezone(&Utc);
        self.since = self.since.min(at);
        for bucket in [
            "all".into(),
            period(at, "day"),
            period(at, "week"),
            period(at, "month"),
        ] {
            let a = self.buckets.entry(bucket).or_default();
            add_stat(&mut a.totals, &record);
            add_stat(
                a.models
                    .entry(format!(
                        "{}/{}",
                        text(&record, "upstream_id"),
                        text(&record, "native_id")
                    ))
                    .or_insert(Value::Null),
                &record,
            );
            let u = a
                .upstreams
                .entry(text(&record, "upstream_id").into())
                .or_insert(Value::Null);
            add_stat(u, &record);
            u["name"] = record["upstream_name"].clone();
            add_stat(
                a.users
                    .entry(text(&record, "user").into())
                    .or_insert(Value::Null),
                &record,
            );
        }
        self.recent.push_front(record);
        self.recent.truncate(200);
    }
    pub fn record(&mut self, record: Value) {
        if !self.root.as_os_str().is_empty()
            && append(&self.root.join(user_file(text(&record, "user"))), &record).is_err()
        {
            eprintln!("gateway usage persistence failed");
        }
        self.add(record);
    }
    pub fn len(&self) -> i64 {
        self.buckets
            .get("all")
            .and_then(|a| a.totals["requests"].as_i64())
            .unwrap_or(0)
    }
    pub fn snapshot(&self, query: &str) -> Value {
        let selected = url_query(query, "period")
            .filter(|v| ["day", "week", "month", "all"].contains(&v.as_str()))
            .unwrap_or("day".into());
        let bucket = if selected == "all" {
            "all".into()
        } else {
            period(Utc::now(), &selected)
        };
        let a = self.buckets.get(&bucket).cloned().unwrap_or_default();
        json!({"period":selected,"since":self.since,"totals":if a.totals.is_null(){json!({"requests":0,"errors":0,"latency_ms_avg":0,"input_tokens":0,"output_tokens":0,"total_tokens":0,"usage_requests":0})}else{a.totals},"per_model":a.models,"per_upstream":a.upstreams,"per_user":a.users,"recent":self.recent})
    }
}
fn append(path: &Path, record: &Value) -> anyhow::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(&serde_json::to_vec(record)?)?;
    file.write_all(b"\n")?;
    Ok(())
}
pub(super) fn url_query(query: &str, key: &str) -> Option<String> {
    reqwest::Url::parse(&format!("http://localhost/?{query}"))
        .ok()?
        .query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}
