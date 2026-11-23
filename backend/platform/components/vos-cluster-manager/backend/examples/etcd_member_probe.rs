//! etcd-client crate member_add 行为探针（本地复现 / 排障用）
//!
//! 用法：`cargo run --example etcd_member_probe -- <endpoint> <peer_url> [timeout_s]`
//! 例：`cargo run --example etcd_member_probe -- http://127.0.0.1:12379 http://127.0.0.1:12390:2380 20`
//! 探针复刻 RealEtcdClusterClient::member_add 的完全调用路径，用于定位
//! "workflow 部署 etcd 后即时 member_add 卡 30s" 根因。

use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let endpoint = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "http://127.0.0.1:12379".to_string());
    let peer_url = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "http://127.0.0.1:12390:2380".to_string());
    let timeout_s: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(20);

    tracing_subscriber::fmt().with_max_level(tracing::Level::DEBUG).init();

    tracing::info!(%endpoint, "连接 etcd ...");
    let addrs: Vec<&str> = vec![endpoint.as_str()];
    let client = etcd_client::Client::connect(&addrs, None).await?;
    tracing::info!("connected，开始 member_list ...");

    let t0 = std::time::Instant::now();
    let list = tokio::time::timeout(Duration::from_secs(5), client.cluster_client().member_list()).await;
    tracing::info!("member_list: {:?} 耗时 {:?}", list.is_ok(), t0.elapsed());

    tracing::info!(%peer_url, "开始 member_add ...");
    let t1 = std::time::Instant::now();
    let out = tokio::time::timeout(
        Duration::from_secs(timeout_s),
        async move {
            let mut cc = client.cluster_client();
            let resp = cc.member_add(vec![peer_url.clone()], None).await?;
            let m = resp.member().unwrap().clone();
            Ok::<(u64, String), etcd_client::Error>((m.id(), m.name().to_string()))
        },
    )
    .await;
    tracing::info!("member_add result: {:?} 耗时 {:?}", out.map(|r| r.map_err(|e| e.to_string())), t1.elapsed());
    Ok(())
}