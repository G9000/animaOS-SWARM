use super::*;
use crate::connectors::gcalendar::store::InMemoryGoogleCredentialStore;
use crate::connectors::oauth_apps::{
    InMemoryOAuthAppVault, OAuthAppCredentials, OAuthAppService, OAuthEnvironment, OAuthProvider,
};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Semaphore;

#[derive(Default)]
struct Fake {
    exchanges: AtomicUsize,
    sends: AtomicUsize,
    refreshes: AtomicUsize,
    ambiguous: bool,
    gate: std::sync::Mutex<Option<Arc<Semaphore>>>,
    claim_path: std::sync::Mutex<Option<std::path::PathBuf>>,
    exchange_entered: std::sync::Mutex<Option<Arc<Semaphore>>>,
    exchange_gate: std::sync::Mutex<Option<Arc<Semaphore>>>,
}

struct EmptyEnvironment;

impl OAuthEnvironment for EmptyEnvironment {
    fn get(&self, _name: &str) -> Option<String> {
        None
    }
}

fn oauth_apps() -> OAuthAppService {
    OAuthAppService::with_backends(
        Arc::new(InMemoryOAuthAppVault::default()),
        Arc::new(EmptyEnvironment),
    )
}
fn tokens() -> GoogleOAuthTokens {
    GoogleOAuthTokens::new(
        "access-sentinel".to_string().into(),
        "refresh-sentinel".to_string().into(),
        now_ms() + 3600000,
    )
}
#[async_trait]
impl MailTransport for Fake {
    async fn exchange(
        &self,
        _: &OAuthConfig,
        _: &str,
        verifier: &str,
    ) -> Result<GoogleOAuthTokens, MailError> {
        assert!(verifier.len() >= 43);
        self.exchanges.fetch_add(1, Ordering::SeqCst);
        if let Some(entered) = self.exchange_entered.lock().unwrap().clone() {
            entered.add_permits(1);
        }
        let gate = self.exchange_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.acquire().await.unwrap().forget();
        }
        Ok(tokens())
    }
    async fn refresh(&self, _: &OAuthConfig, _: &str) -> Result<GoogleOAuthTokens, MailError> {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        Ok(tokens())
    }
    async fn account(&self, _: Provider, _: &str) -> Result<String, MailError> {
        Ok("owner@example.com".into())
    }
    async fn inbox(&self, _: Provider, _: &str) -> Result<Vec<MailMessage>, MailError> {
        Ok(vec![MailMessage {
            id: "m1".into(),
            from: "sender@example.com".into(),
            subject: "hello".into(),
            preview: "text".into(),
            received_at: "2026-09-05T00:00:00Z".into(),
        }])
    }
    async fn send(&self, _: Provider, _: &str, _: &MailDraft) -> Result<(), MailError> {
        if let Some(path) = self.claim_path.lock().unwrap().as_ref() {
            let value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            assert_eq!(value["mailDrafts"][0]["state"], "sending");
        }
        self.sends.fetch_add(1, Ordering::SeqCst);
        let gate = self.gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            let _permit = gate.acquire().await.unwrap();
        }
        if self.ambiguous {
            Err(MailError::Unknown)
        } else {
            Ok(())
        }
    }
}
async fn fixture(ambiguous: bool) -> (SharedDaemonState, MailManager, Arc<Fake>, String) {
    let state = Arc::new(RwLock::new(DaemonState::new()));
    let agent = state
        .write()
        .await
        .create_agent(anima_core::AgentConfig {
            name: "mail-owner".into(),
            model: "deterministic".into(),
            provider: None,
            bio: None,
            lore: None,
            knowledge: None,
            topics: None,
            adjectives: None,
            style: None,
            system: None,
            tools: None,
            plugins: None,
            settings: None,
        })
        .unwrap()
        .state
        .id;
    let fake = Arc::new(Fake {
        ambiguous,
        ..Default::default()
    });
    let runs = AgentRunCoordinator::new(state.clone(), Arc::new(Semaphore::new(2)));
    let oauth_apps = oauth_apps();
    for provider in [OAuthProvider::Google, OAuthProvider::Microsoft] {
        oauth_apps
            .put(
                provider,
                OAuthAppCredentials::new(provider, "test-client", "test-secret", None).unwrap(),
            )
            .await
            .unwrap();
    }
    let manager = MailManager::new(
        &state,
        runs,
        Arc::new(InMemoryGoogleCredentialStore::default()),
        fake.clone(),
        oauth_apps,
    );
    (state, manager, fake, agent)
}

#[tokio::test]
async fn saved_provider_configs_enable_gmail_and_outlook_without_restart() {
    let state = Arc::new(RwLock::new(DaemonState::new()));
    let runs = AgentRunCoordinator::new(state.clone(), Arc::new(Semaphore::new(2)));
    let service = oauth_apps();
    let manager = MailManager::new(
        &state,
        runs,
        Arc::new(InMemoryGoogleCredentialStore::default()),
        Arc::new(Fake::default()),
        service.clone(),
    );
    assert!(!manager.configured(Provider::Gmail).await.unwrap());
    assert!(!manager.configured(Provider::Outlook).await.unwrap());

    service
        .put(
            OAuthProvider::Google,
            OAuthAppCredentials::new(OAuthProvider::Google, "google-live", "secret", None).unwrap(),
        )
        .await
        .unwrap();
    assert!(manager.configured(Provider::Gmail).await.unwrap());
    assert!(!manager.configured(Provider::Outlook).await.unwrap());

    service
        .put(
            OAuthProvider::Microsoft,
            OAuthAppCredentials::new(
                OAuthProvider::Microsoft,
                "microsoft-live",
                "secret",
                Some("organizations".into()),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(manager.configured(Provider::Outlook).await.unwrap());
}

#[tokio::test]
async fn every_non_deleted_mail_state_blocks_its_provider_config_changes() {
    let (state, manager, _transport, agent) = fixture(false).await;
    let (connector, _) = manager
        .begin_connect(&agent, Provider::Gmail)
        .await
        .unwrap();

    for status in ["pairing", "active", "reauthRequired", "error", "disabled"] {
        state
            .write()
            .await
            .mail_records
            .get_mut(&connector.id)
            .unwrap()
            .connector
            .status = status.into();
        assert!(manager.has_provider_dependency(OAuthProvider::Google).await);
        let dependency_check = manager.clone();
        assert_eq!(
            manager
                .oauth_apps
                .put_if_unused(
                    OAuthProvider::Google,
                    OAuthAppCredentials::new(
                        OAuthProvider::Google,
                        "blocked-client",
                        "blocked-secret",
                        None,
                    )
                    .unwrap(),
                    move || {
                        let manager = dependency_check.clone();
                        async move { manager.has_provider_dependency(OAuthProvider::Google).await }
                    },
                )
                .await
                .unwrap_err(),
            crate::connectors::oauth_apps::OAuthAppError::DependenciesExist
        );
    }
    assert!(
        !manager
            .has_provider_dependency(OAuthProvider::Microsoft)
            .await
    );
    state
        .write()
        .await
        .mail_records
        .get_mut(&connector.id)
        .unwrap()
        .deleted = true;
    assert!(!manager.has_provider_dependency(OAuthProvider::Google).await);
}

#[tokio::test]
async fn mail_callback_rejects_oauth_config_revision_changed_after_begin() {
    let (_state, manager, transport, agent) = fixture(false).await;
    let (_, url) = manager
        .begin_connect(&agent, Provider::Gmail)
        .await
        .unwrap();
    let nonce = reqwest::Url::parse(&url)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "state")
        .unwrap()
        .1
        .into_owned();
    manager
        .oauth_apps
        .put(
            OAuthProvider::Google,
            OAuthAppCredentials::new(OAuthProvider::Google, "new-client", "new-secret", None)
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        manager
            .complete_connect(Provider::Gmail, &nonce, "code")
            .await
            .unwrap_err(),
        MailError::Invalid
    );
    assert_eq!(transport.exchanges.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn google_config_mutation_waits_for_gmail_oauth_callback() {
    let (_state, manager, transport, agent) = fixture(false).await;
    let (pairing, url) = manager
        .begin_connect(&agent, Provider::Gmail)
        .await
        .unwrap();
    let nonce = reqwest::Url::parse(&url)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "state")
        .unwrap()
        .1
        .into_owned();
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    *transport.exchange_entered.lock().unwrap() = Some(entered.clone());
    *transport.exchange_gate.lock().unwrap() = Some(release.clone());

    let callback = {
        let manager = manager.clone();
        tokio::spawn(async move {
            manager
                .complete_connect(Provider::Gmail, &nonce, "code")
                .await
        })
    };
    entered.acquire().await.unwrap().forget();
    let mutation = {
        let service = manager.oauth_apps.clone();
        tokio::spawn(async move {
            service
                .put(
                    OAuthProvider::Google,
                    OAuthAppCredentials::new(
                        OAuthProvider::Google,
                        "replacement-client",
                        "replacement-secret",
                        None,
                    )
                    .unwrap(),
                )
                .await
        })
    };
    assert!(tokio::time::timeout(Duration::from_millis(50), async {
        while !mutation.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .is_err());
    release.add_permits(1);
    assert_eq!(callback.await.unwrap().unwrap().id, pairing.id);
    mutation.await.unwrap().unwrap();
}

async fn connect(manager: &MailManager, agent: &str) -> MailConnector {
    let (c, url) = manager.begin_connect(agent, Provider::Gmail).await.unwrap();
    let url = reqwest::Url::parse(&url).unwrap();
    assert_eq!(
        url.query_pairs()
            .find(|(k, _)| k == "code_challenge_method")
            .unwrap()
            .1,
        "S256"
    );
    let nonce = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    manager
        .complete_connect(Provider::Gmail, &nonce, "code")
        .await
        .unwrap();
    c
}
fn input() -> DraftInput {
    DraftInput {
        to: vec!["friend@example.com".into()],
        subject: "Subject".into(),
        body: "Message".into(),
    }
}
#[tokio::test]
async fn oauth_is_single_use_and_drafts_never_send_until_approval() {
    let (_state, m, f, agent) = fixture(false).await;
    let (c, url) = m.begin_connect(&agent, Provider::Gmail).await.unwrap();
    let nonce = reqwest::Url::parse(&url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    m.complete_connect(Provider::Gmail, &nonce, "code")
        .await
        .unwrap();
    assert!(m
        .complete_connect(Provider::Gmail, &nonce, "code")
        .await
        .is_err());
    assert_eq!(f.exchanges.load(Ordering::SeqCst), 1);
    let draft = m
        .create_draft(&agent, &c.id, Provider::Gmail, input())
        .await
        .unwrap();
    assert_eq!(f.sends.load(Ordering::SeqCst), 0);
    assert_eq!(
        m.resolve(&agent, &c.id, Provider::Gmail, &draft.id, true)
            .await
            .unwrap()
            .state,
        DraftState::Sent
    );
    assert!(m
        .resolve(&agent, &c.id, Provider::Gmail, &draft.id, true)
        .await
        .is_err());
    assert_eq!(f.sends.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn ambiguous_send_is_terminal_and_disconnect_invalidates_access() {
    let (_state, m, f, agent) = fixture(true).await;
    let c = connect(&m, &agent).await;
    let d = m
        .create_draft(&agent, &c.id, Provider::Gmail, input())
        .await
        .unwrap();
    assert_eq!(
        m.resolve(&agent, &c.id, Provider::Gmail, &d.id, true)
            .await
            .unwrap()
            .state,
        DraftState::Unknown
    );
    assert!(m
        .resolve(&agent, &c.id, Provider::Gmail, &d.id, true)
        .await
        .is_err());
    m.disconnect(&agent, &c.id, Provider::Gmail).await.unwrap();
    assert!(m
        .messages(&agent, &c.id, Provider::Gmail, true)
        .await
        .is_err());
    assert_eq!(f.sends.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn expiry_and_cross_provider_oauth_fail_before_exchange() {
    let (_state, m, f, agent) = fixture(false).await;
    let (_, url) = m.begin_connect(&agent, Provider::Gmail).await.unwrap();
    let nonce = reqwest::Url::parse(&url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    assert!(m
        .complete_connect(Provider::Outlook, &nonce, "code")
        .await
        .is_err());
    m.pending.lock().await.get_mut(&nonce).unwrap().expires = 0;
    assert!(m
        .complete_connect(Provider::Gmail, &nonce, "code")
        .await
        .is_err());
    assert_eq!(f.exchanges.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn reconnect_preserves_drafts_and_invalidates_previous_nonce() {
    let (_s, m, f, agent) = fixture(false).await;
    let c = connect(&m, &agent).await;
    let d = m
        .create_draft(&agent, &c.id, Provider::Gmail, input())
        .await
        .unwrap();
    let (_, first) = m.begin_connect(&agent, Provider::Gmail).await.unwrap();
    let (next, second) = m.begin_connect(&agent, Provider::Gmail).await.unwrap();
    assert_eq!(next.id, c.id);
    let nonce = |url: String| {
        reqwest::Url::parse(&url)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .into_owned()
    };
    assert!(m
        .complete_connect(Provider::Gmail, &nonce(first), "code")
        .await
        .is_err());
    m.complete_connect(Provider::Gmail, &nonce(second), "code")
        .await
        .unwrap();
    assert_eq!(
        m.drafts(&agent, &c.id, Provider::Gmail).await.unwrap()[0].id,
        d.id
    );
    assert_eq!(f.sends.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn missing_vault_grant_marks_reauth_and_never_sends() {
    let (_s, m, f, agent) = fixture(false).await;
    let c = connect(&m, &agent).await;
    let d = m
        .create_draft(&agent, &c.id, Provider::Gmail, input())
        .await
        .unwrap();
    m.credentials.delete(&c.id).await.unwrap();
    assert!(m
        .resolve(&agent, &c.id, Provider::Gmail, &d.id, true)
        .await
        .is_err());
    assert_eq!(
        m.connector_for_agent(&agent, Provider::Gmail)
            .await
            .unwrap()
            .unwrap()
            .status,
        "reauthRequired"
    );
    assert_eq!(f.sends.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn refresh_rotates_vault_and_snapshot_contains_no_oauth_secrets() {
    let (s, m, f, agent) = fixture(false).await;
    let c = connect(&m, &agent).await;
    m.credentials
        .put(
            &c.id,
            GoogleOAuthTokens::new(
                "expired".to_string().into(),
                "old-refresh".to_string().into(),
                0,
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        m.messages(&agent, &c.id, Provider::Gmail, true)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(f.refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(
        m.credentials
            .load(&c.id)
            .await
            .unwrap()
            .unwrap()
            .refresh_token(),
        "refresh-sentinel"
    );
    let d = m
        .create_draft(&agent, &c.id, Provider::Gmail, input())
        .await
        .unwrap();
    let snapshot = s.read().await.control_plane_snapshot();
    let serialized = serde_json::to_string(&snapshot).unwrap();
    assert!(!serialized.contains("access-sentinel"));
    assert!(!serialized.contains("refresh-sentinel"));
    let mut restored = DaemonState::new();
    restored
        .restore_control_plane_snapshot(serde_json::from_str(&serialized).unwrap())
        .unwrap();
    assert_eq!(restored.mail_drafts[&d.id].body, "Message");
    assert_eq!(restored.mail_records[&c.id].messages.len(), 1);
}
#[tokio::test]
async fn durable_send_claim_failure_prevents_provider_call() {
    let (s, m, f, agent) = fixture(false).await;
    let c = connect(&m, &agent).await;
    let d = m
        .create_draft(&agent, &c.id, Provider::Gmail, input())
        .await
        .unwrap();
    let directory =
        std::env::temp_dir().join(format!("anima-mail-failure-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    s.write().await.set_control_plane_store(Some(
        crate::control_plane_store::ControlPlaneStoreConfig::Json(directory.clone()),
    ));
    assert!(matches!(
        m.resolve(&agent, &c.id, Provider::Gmail, &d.id, true).await,
        Err(MailError::Persistence)
    ));
    assert_eq!(f.sends.load(Ordering::SeqCst), 0);
    assert_eq!(s.read().await.mail_drafts[&d.id].state, DraftState::Pending);
    std::fs::remove_dir(directory).unwrap();
}
#[tokio::test]
async fn restart_marks_inflight_send_unknown_and_reject_has_no_network_effect() {
    let (s, m, f, agent) = fixture(false).await;
    let c = connect(&m, &agent).await;
    let d = m
        .create_draft(&agent, &c.id, Provider::Gmail, input())
        .await
        .unwrap();
    assert_eq!(
        m.resolve(&agent, &c.id, Provider::Gmail, &d.id, false)
            .await
            .unwrap()
            .state,
        DraftState::Rejected
    );
    assert_eq!(f.sends.load(Ordering::SeqCst), 0);
    let d = m
        .create_draft(&agent, &c.id, Provider::Gmail, input())
        .await
        .unwrap();
    s.write().await.mail_drafts.get_mut(&d.id).unwrap().state = DraftState::Sending;
    let snapshot = s.read().await.control_plane_snapshot();
    let mut restored = DaemonState::new();
    restored.restore_control_plane_snapshot(snapshot).unwrap();
    assert_eq!(restored.mail_drafts[&d.id].state, DraftState::Unknown);
}

#[tokio::test]
async fn failed_oauth_publish_rolls_back_tools_and_vault() {
    let (s, m, _f, agent) = fixture(false).await;
    let (c, url) = m.begin_connect(&agent, Provider::Gmail).await.unwrap();
    let nonce = reqwest::Url::parse(&url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    let directory = std::env::temp_dir().join(format!(
        "anima-mail-publish-failure-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir(&directory).unwrap();
    s.write().await.set_control_plane_store(Some(
        crate::control_plane_store::ControlPlaneStoreConfig::Json(directory.clone()),
    ));
    assert!(m
        .complete_connect(Provider::Gmail, &nonce, "code")
        .await
        .is_err());
    assert!(m.credentials.load(&c.id).await.unwrap().is_none());
    assert!(!s
        .read()
        .await
        .get_agent(&agent)
        .unwrap()
        .state
        .config
        .tools
        .unwrap_or_default()
        .iter()
        .any(|t| t.name == "mail_list_messages"));
    std::fs::remove_dir(directory).unwrap();
}
#[tokio::test]
async fn cancelled_http_waiter_does_not_cancel_claimed_send() {
    let (_s, m, f, agent) = fixture(false).await;
    let c = connect(&m, &agent).await;
    let d = m
        .create_draft(&agent, &c.id, Provider::Gmail, input())
        .await
        .unwrap();
    let gate = Arc::new(Semaphore::new(0));
    *f.gate.lock().unwrap() = Some(gate.clone());
    let task = {
        let m = m.clone();
        let agent = agent.clone();
        let id = c.id.clone();
        let draft = d.id.clone();
        tokio::spawn(async move { m.resolve(&agent, &id, Provider::Gmail, &draft, true).await })
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.sends.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await
        }
    })
    .await
    .unwrap();
    task.abort();
    let _ = task.await;
    gate.add_permits(1);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if m.drafts(&agent, &c.id, Provider::Gmail).await.unwrap()[0].state == DraftState::Sent
            {
                break;
            }
            tokio::task::yield_now().await
        }
    })
    .await
    .unwrap();
    assert_eq!(f.sends.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn pending_draft_is_visible_after_many_newer_resolutions() {
    let (s, m, _f, agent) = fixture(false).await;
    let c = connect(&m, &agent).await;
    let d = m
        .create_draft(&agent, &c.id, Provider::Gmail, input())
        .await
        .unwrap();
    {
        let mut s = s.write().await;
        for i in 0..110 {
            let mut newer = d.clone();
            newer.id = format!("terminal-{i}");
            newer.state = DraftState::Rejected;
            newer.created_at_ms += i + 1;
            newer.resolved_at_ms = Some(now_ms());
            s.mail_drafts.insert(newer.id.clone(), newer);
        }
    }
    assert!(m
        .drafts(&agent, &c.id, Provider::Gmail)
        .await
        .unwrap()
        .iter()
        .any(|item| item.id == d.id));
}
#[tokio::test]
async fn send_transport_observes_durable_sending_claim() {
    let (s, m, f, agent) = fixture(false).await;
    let c = connect(&m, &agent).await;
    let path = std::env::temp_dir().join(format!("anima-mail-claim-{}.json", uuid::Uuid::new_v4()));
    s.write().await.set_control_plane_store(Some(
        crate::control_plane_store::ControlPlaneStoreConfig::Json(path.clone()),
    ));
    *f.claim_path.lock().unwrap() = Some(path.clone());
    let d = m
        .create_draft(&agent, &c.id, Provider::Gmail, input())
        .await
        .unwrap();
    m.resolve(&agent, &c.id, Provider::Gmail, &d.id, true)
        .await
        .unwrap();
    let persisted: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(persisted["mailDrafts"][0]["state"], "sent");
    std::fs::remove_file(path).unwrap();
}
