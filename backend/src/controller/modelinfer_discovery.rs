//! Periodic discovery of ModelInfer deployments into the gateway catalog.
/// ModelInfer may be installed after this application; check on every tick.
pub(super) fn start() {
    if std::env::var_os("VOS_APP_VERSION").is_none() {
        return;
    }
    tokio::spawn(async move {
        loop {
            crate::modelinfer::sync().await;
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    });
}
