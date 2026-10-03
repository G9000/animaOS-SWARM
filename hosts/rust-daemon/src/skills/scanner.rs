//! The background rescan (spec §8.1): one at once (the startup scan), then
//! one every 60 seconds until `shutdown`. Only the real daemon runs it
//! (`app::serve_with_state`); the routes rescan on request anyway.

use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::warn;

use super::service::{SkillError, SkillService};
use super::SKILL_SCAN_INTERVAL_MS;

type Running = Option<(watch::Sender<bool>, JoinHandle<()>)>;

#[derive(Clone)]
pub(crate) struct SkillScanner {
    service: SkillService,
    interval: Duration,
    running: Arc<StdMutex<Running>>,
}

impl SkillScanner {
    pub(crate) fn new(service: SkillService) -> Self {
        Self {
            service,
            interval: Duration::from_millis(SKILL_SCAN_INTERVAL_MS),
            running: Arc::new(StdMutex::new(None)),
        }
    }

    /// A shorter period, so a test need not wait a minute.
    #[cfg(test)]
    pub(crate) fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Starts the loop; a second call is a no-op. Needs a Tokio runtime.
    pub(crate) fn start(&self) {
        let mut running = self
            .running
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if running.is_some() {
            return;
        }
        let (stop, mut stopping) = watch::channel(false);
        let (service, interval) = (self.service.clone(), self.interval);
        let join = tokio::spawn(async move {
            loop {
                match service.scan().await {
                    Ok(_) | Err(SkillError::NoWorkspace) => {}
                    Err(error) => warn!(error = %error.message(), "skills scan failed"),
                }
                tokio::select! {
                    biased;
                    _ = stopping.wait_for(|stop| *stop) => break,
                    () = tokio::time::sleep(interval) => {}
                }
            }
        });
        *running = Some((stop, join));
    }

    /// Stops the loop after the scan in progress, if any (each is bounded by
    /// `SKILL_IO_TIMEOUT_MS`).
    pub(crate) async fn shutdown(&self) {
        let handle = self
            .running
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some((stop, join)) = handle {
            let _ = stop.send(true);
            let _ = join.await;
        }
    }
}
