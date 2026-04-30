use tracing::{info, warn};

pub(super) async fn shutdown_signal() {
    match tokio::signal::ctrl_c().await {
        Ok(()) => info!("shutdown signal received"),
        Err(error) => warn!("failed to install shutdown signal handler: {error}"),
    }
}
