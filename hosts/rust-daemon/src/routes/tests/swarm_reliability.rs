use super::*;
use crate::control_plane_store::{load_control_plane_snapshot, ControlPlaneStoreConfig};
use anima_swarm::SwarmStatus;

struct Fixture {
    app: axum::Router,
    state: Arc<RwLock<DaemonState>>,
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
    swarm_id: String,
    store: ControlPlaneStoreConfig,
    events: tokio::sync::Mutex<crate::events::EventSubscriber>,
    _workspace: WorkspaceAvatarTemp,
}

impl Fixture {
    async fn new(timeout: Duration, limit: usize) -> Self {
        let workspace = WorkspaceAvatarTemp::new("swarm-reliability");
        let store = ControlPlaneStoreConfig::Json(workspace.root.join("control-plane.json"));
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let mut daemon = DaemonState::with_model_adapter(Arc::new(GateFirstModelAdapter {
            entered: entered.clone(),
            release: release.clone(),
            calls: AtomicUsize::new(0),
        }));
        daemon.set_control_plane_store(Some(store.clone()));
        let state = Arc::new(RwLock::new(daemon));
        let app = router(
            state.clone(),
            DaemonConfig {
                request_timeout: timeout,
                max_concurrent_runs: limit,
                ..DaemonConfig::default()
            },
        );
        let swarm_id = create_test_swarm(&app, &state).await;
        let events = tokio::sync::Mutex::new(
            state
                .read()
                .await
                .subscribe_to_swarm_events(&swarm_id)
                .unwrap(),
        );
        Self {
            app,
            state,
            entered,
            release,
            swarm_id,
            store,
            events,
            _workspace: workspace,
        }
    }

    fn request(&self) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(format!("/api/swarms/{}/run", self.swarm_id))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"text":"finish owned work"}"#))
            .unwrap()
    }

    fn start(&self) -> tokio::task::JoinHandle<axum::response::Response> {
        let app = self.app.clone();
        let request = self.request();
        tokio::spawn(async move { app.oneshot(request).await.unwrap() })
    }

    async fn entered(&self) {
        tokio::time::timeout(Duration::from_secs(2), self.entered.acquire())
            .await
            .expect("model starts")
            .unwrap()
            .forget();
    }

    async fn completed(&self) {
        let completion = tokio::time::timeout(Duration::from_secs(5), async {
            let mut events = self.events.lock().await;
            loop {
                if events.recv().await.unwrap().event == "swarm:completed" {
                    break;
                }
            }
        })
        .await;
        if completion.is_err() {
            let live = self.state.read().await.get_swarm(&self.swarm_id);
            let disk = load_control_plane_snapshot(&self.store).await;
            panic!("owned swarm did not commit: live={live:?}; disk={disk:?}");
        }
        // Completion is published only after the atomic durable write. Read
        // once here, avoiding polling the file while Windows replaces it.
        let snapshot = load_control_plane_snapshot(&self.store)
            .await
            .unwrap()
            .unwrap();
        let swarm = &snapshot.swarms[0].state;
        assert_eq!(swarm.status, SwarmStatus::Idle);
        assert!(swarm.completed_at.is_some());
        assert_eq!(swarm.token_usage.total_tokens, 2);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn swarm_persists_running_before_model_execution() {
    let fixture = Fixture::new(Duration::from_secs(5), 2).await;
    let running = fixture.start();
    fixture.entered().await;
    let persisted = load_control_plane_snapshot(&fixture.store)
        .await
        .unwrap()
        .unwrap();
    fixture.release.add_permits(1);
    assert_eq!(running.await.unwrap().status(), StatusCode::OK);
    assert_eq!(persisted.swarms[0].state.status, SwarmStatus::Running);
    let mut restarted = DaemonState::new();
    restarted.restore_control_plane_snapshot(persisted).unwrap();
    assert_ne!(
        restarted.get_swarm(&fixture.swarm_id).unwrap().status,
        SwarmStatus::Idle
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn swarm_http_timeout_retains_admission_and_finishes_owned_work() {
    let fixture = Fixture::new(Duration::from_millis(50), 1).await;
    let running = fixture.start();
    fixture.entered().await;
    assert_eq!(running.await.unwrap().status(), StatusCode::REQUEST_TIMEOUT);
    let saturated = fixture
        .app
        .clone()
        .oneshot(fixture.request())
        .await
        .unwrap();
    fixture.release.add_permits(1);
    assert_eq!(saturated.status(), StatusCode::SERVICE_UNAVAILABLE);
    fixture.completed().await;
    let retry = fixture
        .app
        .clone()
        .oneshot(fixture.request())
        .await
        .unwrap()
        .status();
    // The short HTTP deadline may expire under parallel CI load even for this
    // second run; admission and eventual completion are the required contract.
    assert!(matches!(
        retry,
        StatusCode::OK | StatusCode::REQUEST_TIMEOUT
    ));
    fixture.completed().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn swarm_aborted_http_waiter_does_not_cancel_owned_work() {
    let fixture = Fixture::new(Duration::from_secs(5), 1).await;
    let running = fixture.start();
    fixture.entered().await;
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    fixture.release.add_permits(1);
    fixture.completed().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn swarm_initial_persistence_failure_does_not_start_the_model() {
    let fixture = Fixture::new(Duration::from_secs(5), 1).await;
    let gate = fixture
        .state
        .write()
        .await
        .install_test_control_plane_save_gate(true);
    gate.release.add_permits(1);
    assert_eq!(
        fixture.start().await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(fixture.entered.available_permits(), 0);
    assert_eq!(
        fixture.state.read().await.control_plane_snapshot().swarms[0]
            .state
            .status,
        SwarmStatus::Idle
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn swarm_queued_request_waits_for_the_first_durable_commit() {
    let fixture = Fixture::new(Duration::from_secs(5), 2).await;
    let first = fixture.start();
    fixture.entered().await;
    let gate = fixture
        .state
        .write()
        .await
        .install_test_control_plane_save_gate(false);
    let second = fixture.start();
    let early_save = tokio::time::timeout(Duration::from_millis(50), gate.entered.acquire()).await;
    let second_published_early = early_save.is_ok();
    drop(early_save);
    fixture.release.add_permits(1);
    if !second_published_early {
        tokio::time::timeout(Duration::from_secs(2), gate.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
    assert!(!first.is_finished());
    assert!(!second.is_finished());
    gate.release.add_permits(1);
    assert_eq!(first.await.unwrap().status(), StatusCode::OK);
    assert_eq!(second.await.unwrap().status(), StatusCode::OK);
    assert!(
        !second_published_early,
        "queued work must not publish over a running swarm"
    );
    fixture.completed().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn swarm_failed_final_commit_preserves_the_durable_running_marker() {
    let fixture = Fixture::new(Duration::from_secs(5), 1).await;
    let running = fixture.start();
    fixture.entered().await;
    let gate = fixture
        .state
        .write()
        .await
        .install_test_control_plane_save_gate(true);
    gate.release.add_permits(1);
    fixture.release.add_permits(1);
    assert_eq!(
        running.await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    // A later unrelated publisher must not leak the uncommitted terminal state.
    fixture
        .state
        .write()
        .await
        .control_plane_persist_request()
        .save()
        .await
        .unwrap();
    let persisted = load_control_plane_snapshot(&fixture.store)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.swarms[0].state.status, SwarmStatus::Running);
}
