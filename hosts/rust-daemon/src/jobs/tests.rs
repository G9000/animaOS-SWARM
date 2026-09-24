use super::*;
use crate::control_plane_store::{load_control_plane_snapshot, ControlPlaneStoreConfig};
use crate::state::DaemonState;
use tokio::sync::{RwLock, Semaphore};

#[tokio::test]
async fn job_timestamps_remain_valid_after_clock_moves_backwards() {
    let (service, agent, path) = setup();
    let mut job = service
        .create(&agent, "clock", "prompt", "clock")
        .await
        .unwrap();
    let future = now_ms() + 60_000;
    job.updated_at_ms = future;
    job.started_at_ms = Some(future);
    job.status = AgentJobStatus::Running;
    job.attempt = 1;
    advance(&mut job);
    assert!(job.updated_at_ms >= future);
    assert!(job.validate().is_ok());
    review(&mut job, "interrupted");
    assert!(job.validate().is_ok());
    std::fs::remove_file(path).unwrap();
}

struct PausedModel {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}
#[async_trait::async_trait]
impl anima_core::ModelAdapter for PausedModel {
    fn provider(&self) -> &str {
        "paused-test"
    }
    async fn generate(
        &self,
        config: &anima_core::AgentConfig,
        request: &anima_core::ModelGenerateRequest,
    ) -> Result<anima_core::ModelGenerateResponse, String> {
        self.entered.notify_one();
        self.release.notified().await;
        anima_core::ModelAdapter::generate(
            &crate::model::DeterministicModelAdapter,
            config,
            request,
        )
        .await
    }
}

#[tokio::test]
async fn claim_precedes_model_and_failed_completion_stays_uncertain_without_replay() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let path = std::env::temp_dir().join(format!("anima-job-commit-{}.json", uuid::Uuid::new_v4()));
    let blocked = path.with_extension("blocked");
    std::fs::create_dir(&blocked).unwrap();
    let mut state = DaemonState::with_model_adapter(Arc::new(PausedModel {
        entered: entered.clone(),
        release: release.clone(),
    }));
    state.set_control_plane_store(Some(ControlPlaneStoreConfig::Json(path.clone())));
    let agent = state
        .create_agent(
            serde_json::from_value(serde_json::json!({"name":"paused","model":"deterministic"}))
                .unwrap(),
        )
        .unwrap()
        .state
        .id;
    let state = Arc::new(RwLock::new(state));
    let runs = AgentRunCoordinator::new(state.clone(), Arc::new(Semaphore::new(4)));
    let service = JobService::new(state.clone(), runs);
    let job = service
        .create(&agent, "title", "prompt", "key")
        .await
        .unwrap();
    service.start().await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .unwrap();
    let saved = load_control_plane_snapshot(&ControlPlaneStoreConfig::Json(path.clone()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.jobs[0].status, AgentJobStatus::Running);
    assert_eq!(saved.jobs[0].attempt, 1);
    state
        .write()
        .await
        .set_control_plane_store(Some(ControlPlaneStoreConfig::Json(blocked.clone())));
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if state.read().await.jobs[&job.id].status == AgentJobStatus::NeedsReview {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    service.shutdown().await;
    let current = state.read().await.jobs[&job.id].clone();
    assert_eq!(current.attempt, 1);
    assert!(current.result.is_none());
    assert!(state.read().await.get_agent(&agent).is_some());
    {
        // The undurable final save failed the run and removed its turn.
        let guard = state.read().await;
        let run = guard.runs.for_agent(&agent)[0];
        assert_eq!(run.source, RunSource::Job);
        assert_eq!(run.status, crate::runs::RunStatus::Failed);
        assert_eq!(
            run.error.as_ref().map(|error| error.code.as_str()),
            Some("commit_failed")
        );
        let job_room = format!("job:{}", job.id);
        assert!(guard
            .get_agent(&agent)
            .unwrap()
            .messages
            .iter()
            .all(|message| message.room_id != job_room));
    }
    state
        .write()
        .await
        .set_control_plane_store(Some(ControlPlaneStoreConfig::Json(path.clone())));
    assert!(service
        .retry(&agent, &job.id, current.revision, false)
        .await
        .is_err());
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir(blocked);
}

struct GatedModel {
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
}
#[async_trait::async_trait]
impl anima_core::ModelAdapter for GatedModel {
    fn provider(&self) -> &str {
        "gated-test"
    }
    async fn generate(
        &self,
        config: &anima_core::AgentConfig,
        request: &anima_core::ModelGenerateRequest,
    ) -> Result<anima_core::ModelGenerateResponse, String> {
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        anima_core::ModelAdapter::generate(
            &crate::model::DeterministicModelAdapter,
            config,
            request,
        )
        .await
    }
}

#[tokio::test]
async fn a_chat_run_in_another_room_does_not_hold_back_the_agents_job() {
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let path = std::env::temp_dir().join(format!(
        "anima-job-cross-room-{}.json",
        uuid::Uuid::new_v4()
    ));
    let mut state = DaemonState::with_model_adapter(Arc::new(GatedModel {
        entered: entered.clone(),
        release: release.clone(),
    }));
    state.set_control_plane_store(Some(ControlPlaneStoreConfig::Json(path.clone())));
    let agent = state
        .create_agent(
            serde_json::from_value(serde_json::json!({"name":"busy","model":"deterministic"}))
                .unwrap(),
        )
        .unwrap()
        .state
        .id;
    let state = Arc::new(RwLock::new(state));
    let runs = AgentRunCoordinator::new(state.clone(), Arc::new(Semaphore::new(4)));
    let chat = {
        let runs = runs.clone();
        let agent = agent.clone();
        tokio::spawn(async move {
            runs.run(AgentRunRequest {
                agent_id: agent,
                content: Content {
                    text: "chat turn".into(),
                    attachments: None,
                    metadata: None,
                },
                room: RunRoom::Stable("chat".into()),
                idempotency_key: None,
                source: RunSource::Api,
                source_ref: None,
                parent: None,
            })
            .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();

    let service = JobService::new(state.clone(), runs);
    let job = service
        .create(&agent, "title", "prompt", "key")
        .await
        .unwrap();
    service.start().await.unwrap();
    let job_started = tokio::time::timeout(Duration::from_secs(3), entered.acquire()).await;
    release.add_permits(2);
    assert!(
        job_started.is_ok(),
        "the agent's job starts while its chat run is in another room"
    );
    wait_completed(&service, &job.id).await;
    chat.await.unwrap().unwrap();
    service.shutdown().await;
    let _ = std::fs::remove_file(path);
}

fn setup() -> (JobService, String, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("anima-jobs-{}.json", uuid::Uuid::new_v4()));
    let mut state = DaemonState::new();
    state.set_control_plane_store(Some(ControlPlaneStoreConfig::Json(path.clone())));
    let config =
        serde_json::from_value(serde_json::json!({"name":"worker", "model":"deterministic"}))
            .unwrap();
    let agent = state.create_agent(config).unwrap().state.id;
    let state = Arc::new(RwLock::new(state));
    let runs = AgentRunCoordinator::new(state.clone(), Arc::new(Semaphore::new(4)));
    (JobService::new(state, runs), agent, path)
}

#[tokio::test]
async fn capacity_and_payload_bounds_do_not_evict_history() {
    let (service, agent, path) = setup();
    assert!(service
        .create(&agent, "title", &"p".repeat(32769), "large")
        .await
        .is_err());
    assert!(service
        .create(&agent, &"界".repeat(161), "prompt", "title")
        .await
        .is_err());
    for index in 0..8 {
        service
            .create(&agent, "title", "prompt", &format!("key{index}"))
            .await
            .unwrap();
    }
    assert!(matches!(
        service.create(&agent, "title", "prompt", "overflow").await,
        Err(JobError::Conflict(_))
    ));
    assert_eq!(service.list(&agent).await.unwrap().len(), 8);
    // An exact repeat still succeeds at capacity.
    assert!(service
        .create(&agent, "title", "prompt", "key0")
        .await
        .is_ok());
    let _ = std::fs::remove_file(path);
}

#[test]
fn result_preview_respects_utf8_byte_bound() {
    let value = "界".repeat(30000);
    let result = preview(&value);
    assert!(result.len() <= 65536);
    assert!(value.starts_with(&result));
    assert!(result.len() >= 65533);
}

#[tokio::test]
async fn request_keys_are_scoped_to_agent() {
    let (service, agent, path) = setup();
    let other = service
        .state
        .write()
        .await
        .create_agent(
            serde_json::from_value(serde_json::json!({"name":"other", "model":"deterministic"}))
                .unwrap(),
        )
        .unwrap()
        .state
        .id;
    let first = service
        .create(&agent, "title", "prompt", "shared")
        .await
        .unwrap();
    let second = service
        .create(&other, "title", "prompt", "shared")
        .await
        .unwrap();
    assert_ne!(first.id, second.id);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn snapshot_rejects_duplicate_agent_keys_and_invalid_lifecycle() {
    let (service, agent, path) = setup();
    service
        .create(&agent, "title", "prompt", "key")
        .await
        .unwrap();
    let snapshot = load_control_plane_snapshot(&ControlPlaneStoreConfig::Json(path.clone()))
        .await
        .unwrap()
        .unwrap();
    let mut duplicate = snapshot.clone();
    let mut job = duplicate.jobs[0].clone();
    job.id = uuid::Uuid::new_v4().to_string();
    duplicate.jobs.push(job);
    assert!(DaemonState::new()
        .restore_control_plane_snapshot(duplicate)
        .is_err());
    let mut invalid = snapshot.clone();
    invalid.jobs[0].status = AgentJobStatus::Running;
    assert!(DaemonState::new()
        .restore_control_plane_snapshot(invalid)
        .is_err());
    let mut invalid = snapshot.clone();
    invalid.jobs[0].prompt = "x".repeat(32769);
    assert!(DaemonState::new()
        .restore_control_plane_snapshot(invalid)
        .is_err());
    assert!(DaemonState::new()
        .restore_control_plane_snapshot(snapshot)
        .is_ok());
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn durable_create_is_idempotent_and_conflicting_key_is_rejected() {
    let (service, agent, path) = setup();
    let first = service
        .create(&agent, "title", "prompt", "request")
        .await
        .unwrap();
    assert_eq!(
        first,
        service
            .create(&agent, "title", "prompt", "request")
            .await
            .unwrap()
    );
    assert!(matches!(
        service
            .create(&agent, "title", "different", "request")
            .await,
        Err(JobError::Conflict(_))
    ));
    let saved = load_control_plane_snapshot(&ControlPlaneStoreConfig::Json(path.clone()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.jobs, vec![first]);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn failed_save_rolls_back_only_jobs_and_missing_store_refuses_work() {
    let (service, agent, path) = setup();
    service.state.write().await.set_control_plane_store(None);
    assert!(matches!(
        service.create(&agent, "title", "prompt", "key").await,
        Err(JobError::Conflict(_))
    ));
    std::fs::create_dir(&path).unwrap();
    service
        .state
        .write()
        .await
        .set_control_plane_store(Some(ControlPlaneStoreConfig::Json(path.clone())));
    assert!(service
        .create(&agent, "title", "prompt", "key")
        .await
        .is_err());
    assert!(service.list(&agent).await.unwrap().is_empty());
    assert!(service.state.read().await.get_agent(&agent).is_some());
    let _ = std::fs::remove_dir(path);
}

#[tokio::test]
async fn revisions_cancellation_and_uncertain_retry_are_enforced() {
    let (service, agent, path) = setup();
    let job = service
        .create(&agent, "title", "prompt", "key")
        .await
        .unwrap();
    assert!(matches!(
        service.cancel(&agent, &job.id, 99).await,
        Err(JobError::Conflict(_))
    ));
    let cancelled = service.cancel(&agent, &job.id, job.revision).await.unwrap();
    assert_eq!(cancelled.status, AgentJobStatus::Cancelled);
    assert!(service
        .retry(&agent, &job.id, cancelled.revision, true)
        .await
        .is_err());
    {
        let mut state = service.state.write().await;
        let job = state.jobs.get_mut(&job.id).unwrap();
        job.status = AgentJobStatus::NeedsReview;
        job.attempt = 1;
        job.started_at_ms = Some(job.created_at_ms);
    }
    assert!(service
        .retry(&agent, &job.id, cancelled.revision, false)
        .await
        .is_err());
    let retried = service
        .retry(&agent, &job.id, cancelled.revision, true)
        .await
        .unwrap();
    assert_eq!(retried.status, AgentJobStatus::Queued);
    {
        let mut state = service.state.write().await;
        let job = state.jobs.get_mut(&job.id).unwrap();
        job.status = AgentJobStatus::Failed;
        job.attempt = 3;
    }
    assert!(service
        .retry(&agent, &job.id, retried.revision, true)
        .await
        .is_err());
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn restart_recovers_queued_but_never_replays_running() {
    let (service, agent, path) = setup();
    let queued = service
        .create(&agent, "queued", "prompt", "queued")
        .await
        .unwrap();
    let orphan = service
        .create(&agent, "orphan", "prompt", "orphan")
        .await
        .unwrap();
    {
        let mut state = service.state.write().await;
        let job = state.jobs.get_mut(&orphan.id).unwrap();
        job.status = AgentJobStatus::Running;
        job.attempt = 1;
        job.started_at_ms = Some(job.updated_at_ms);
    }
    service
        .state
        .write()
        .await
        .control_plane_persist_request()
        .save()
        .await
        .unwrap();
    let restored = load_control_plane_snapshot(&ControlPlaneStoreConfig::Json(path.clone()))
        .await
        .unwrap()
        .unwrap();
    let mut restored_state = DaemonState::new();
    restored_state.set_control_plane_store(Some(ControlPlaneStoreConfig::Json(path.clone())));
    restored_state
        .restore_control_plane_snapshot(restored)
        .unwrap();
    let state = Arc::new(RwLock::new(restored_state));
    let runs = AgentRunCoordinator::new(state.clone(), Arc::new(Semaphore::new(4)));
    let service = JobService::new(state, runs);
    service.start().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if service.state.read().await.jobs[&queued.id].status == AgentJobStatus::Completed {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    service.shutdown().await;
    let state = service.state.read().await;
    assert_eq!(state.jobs[&orphan.id].status, AgentJobStatus::NeedsReview);
    assert_eq!(state.jobs[&orphan.id].attempt, 1);
    drop(state);
    let saved = load_control_plane_snapshot(&ControlPlaneStoreConfig::Json(path.clone()))
        .await
        .unwrap()
        .unwrap();
    assert!(saved
        .jobs
        .iter()
        .any(|j| j.id == queued.id && j.status == AgentJobStatus::Completed));
    let _ = std::fs::remove_file(path);
}

include!("supervision_tests.rs");

#[path = "goal_tests.rs"]
mod goal_tests;
