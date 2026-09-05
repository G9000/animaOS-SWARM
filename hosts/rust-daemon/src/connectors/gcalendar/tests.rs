use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anima_core::{AgentConfig, DataValue, Message, MessageRole, ToolCall};
use async_trait::async_trait;
use tokio::sync::{RwLock, Semaphore};
use zeroize::Zeroizing;

use crate::agent_runs::AgentRunCoordinator;
use crate::app::SharedDaemonState;
use crate::connectors::gcalendar::client::{
    GoogleCalendarEvent, GoogleCalendarTransport, GoogleTransportError,
};
use crate::connectors::gcalendar::store::{
    GoogleCredentialStore, GoogleOAuthTokens, InMemoryGoogleCredentialStore,
};
use crate::connectors::gcalendar::{
    now_ms, CalendarError, CalendarEventDraft, CalendarManager, CalendarWriteOperation,
    CalendarWriteState, GoogleOAuthConfig, CALENDAR_TOOL_NAMES,
};
use crate::state::DaemonState;

fn tool_message() -> Message {
    Message {
        id: "message-tool".to_string(),
        agent_id: "agent".to_string(),
        room_id: "room".to_string(),
        content: anima_core::Content::default(),
        role: MessageRole::User,
        created_at_ms: 1,
    }
}

struct FakeGoogleTransport {
    tokens: Mutex<Option<Result<GoogleOAuthTokens, GoogleTransportError>>>,
    refreshed: Mutex<Option<Result<GoogleOAuthTokens, GoogleTransportError>>>,
    events: Mutex<Vec<GoogleCalendarEvent>>,
    created: Mutex<Vec<CalendarEventDraft>>,
    updated: Mutex<Vec<CalendarEventDraft>>,
    deleted: Mutex<Vec<CalendarEventDraft>>,
    apply_error: Mutex<Option<GoogleTransportError>>,
}

impl FakeGoogleTransport {
    fn new() -> Self {
        Self {
            tokens: Mutex::new(Some(Ok(GoogleOAuthTokens::new(
                Zeroizing::new("access-initial".to_string()),
                Zeroizing::new("refresh-initial".to_string()),
                now_ms() + 3_600_000,
            )))),
            refreshed: Mutex::new(None),
            events: Mutex::new(Vec::new()),
            created: Mutex::new(Vec::new()),
            updated: Mutex::new(Vec::new()),
            deleted: Mutex::new(Vec::new()),
            apply_error: Mutex::new(None),
        }
    }
}

#[async_trait]
impl GoogleCalendarTransport for FakeGoogleTransport {
    async fn exchange_code(
        &self,
        _config: &GoogleOAuthConfig,
        _code: &str,
    ) -> Result<GoogleOAuthTokens, GoogleTransportError> {
        self.tokens
            .lock()
            .unwrap()
            .take()
            .unwrap_or(Err(GoogleTransportError::Unavailable))
    }

    async fn refresh_tokens(
        &self,
        _config: &GoogleOAuthConfig,
        _refresh_token: &str,
    ) -> Result<GoogleOAuthTokens, GoogleTransportError> {
        self.refreshed
            .lock()
            .unwrap()
            .take()
            .unwrap_or(Err(GoogleTransportError::Unavailable))
    }

    async fn primary_calendar(&self, _access_token: &str) -> Result<String, GoogleTransportError> {
        Ok("owner@example.com".to_string())
    }

    async fn list_events(
        &self,
        _access_token: &str,
        _calendar_id: &str,
        _time_min: &str,
        _time_max: &str,
    ) -> Result<Vec<GoogleCalendarEvent>, GoogleTransportError> {
        Ok(self.events.lock().unwrap().clone())
    }

    async fn create_event(
        &self,
        _access_token: &str,
        draft: &CalendarEventDraft,
    ) -> Result<String, GoogleTransportError> {
        if let Some(error) = self.apply_error.lock().unwrap().take() {
            return Err(error);
        }
        self.created.lock().unwrap().push(draft.clone());
        Ok("event-created-1".to_string())
    }

    async fn update_event(
        &self,
        _access_token: &str,
        draft: &CalendarEventDraft,
    ) -> Result<(), GoogleTransportError> {
        if let Some(error) = self.apply_error.lock().unwrap().take() {
            return Err(error);
        }
        self.updated.lock().unwrap().push(draft.clone());
        Ok(())
    }

    async fn delete_event(
        &self,
        _access_token: &str,
        draft: &CalendarEventDraft,
    ) -> Result<(), GoogleTransportError> {
        if let Some(error) = self.apply_error.lock().unwrap().take() {
            return Err(error);
        }
        self.deleted.lock().unwrap().push(draft.clone());
        Ok(())
    }
}

struct Fixture {
    state: SharedDaemonState,
    manager: CalendarManager,
    transport: Arc<FakeGoogleTransport>,
    credentials: Arc<InMemoryGoogleCredentialStore>,
    agent_id: String,
}

async fn fixture(oauth: Option<GoogleOAuthConfig>) -> Fixture {
    fixture_with_transport(oauth, Arc::new(FakeGoogleTransport::new())).await
}

async fn fixture_with_transport(
    oauth: Option<GoogleOAuthConfig>,
    transport: Arc<FakeGoogleTransport>,
) -> Fixture {
    let state: SharedDaemonState = Arc::new(RwLock::new(DaemonState::new()));
    let agent_id = state
        .write()
        .await
        .create_agent(test_config("calendar-owner"))
        .expect("agent should be created")
        .state
        .id;
    let agent_runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::new(Semaphore::new(2)));
    let credentials = Arc::new(InMemoryGoogleCredentialStore::default());
    let manager = CalendarManager::new(
        &state,
        agent_runs,
        credentials.clone(),
        transport.clone(),
        oauth,
    );
    state
        .write()
        .await
        .set_calendar_manager(Some(manager.clone()));
    Fixture {
        state,
        manager,
        transport,
        credentials,
        agent_id,
    }
}

fn test_config(name: &str) -> AgentConfig {
    AgentConfig {
        name: name.to_string(),
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
    }
}

fn draft() -> CalendarEventDraft {
    CalendarEventDraft {
        calendar_id: String::new(),
        event_id: None,
        title: "Lunch with Sam".to_string(),
        start: "2026-09-02T12:00:00Z".to_string(),
        end: "2026-09-02T13:00:00Z".to_string(),
        location: None,
        description: None,
    }
}

async fn connect(fixture: &Fixture) -> crate::connectors::gcalendar::GoogleCalendarConnectorRecord {
    let (record, _url) = fixture
        .manager
        .begin_connect(&fixture.agent_id)
        .await
        .expect("connect should begin");
    let nonce = record
        .pending_auth
        .as_ref()
        .expect("pending auth present")
        .nonce
        .clone();
    fixture
        .manager
        .complete_connect(&nonce, "auth-code")
        .await
        .expect("connect should complete")
}

#[tokio::test]
async fn begin_connect_creates_pairing_record_and_consent_url() {
    let fixture = fixture(Some(GoogleOAuthConfig::for_tests())).await;
    let (record, consent_url) = fixture
        .manager
        .begin_connect(&fixture.agent_id)
        .await
        .expect("connect should begin");
    assert!(record.pending_auth.is_some());
    assert!(!record.is_active());
    assert!(consent_url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
    assert!(consent_url.contains("access_type=offline"));
    assert!(consent_url.contains("state="));

    let duplicate = fixture.manager.begin_connect(&fixture.agent_id).await;
    assert_eq!(duplicate.unwrap_err(), CalendarError::AlreadyConnected);

    let missing = fixture.manager.begin_connect("agent-missing").await;
    assert_eq!(missing.unwrap_err(), CalendarError::AgentNotFound);
}

#[tokio::test]
async fn begin_connect_requires_oauth_configuration() {
    let fixture = fixture(None).await;
    let result = fixture.manager.begin_connect(&fixture.agent_id).await;
    assert_eq!(result.unwrap_err(), CalendarError::Unconfigured);
}

#[tokio::test]
async fn complete_connect_rejects_unknown_or_expired_nonce() {
    let fixture = fixture(Some(GoogleOAuthConfig::for_tests())).await;
    let result = fixture
        .manager
        .complete_connect("bogus-nonce", "auth-code")
        .await;
    assert_eq!(result.unwrap_err(), CalendarError::PairingNotFound);
}

#[tokio::test]
async fn complete_connect_stores_tokens_activates_and_adds_calendar_tools() {
    let fixture = fixture(Some(GoogleOAuthConfig::for_tests())).await;
    let record = connect(&fixture).await;
    assert!(record.is_active());
    assert_eq!(record.account_label.as_deref(), Some("owner@example.com"));

    let stored = fixture
        .credentials
        .load(&record.id)
        .await
        .expect("vault read")
        .expect("tokens stored");
    assert_eq!(stored.access_token(), "access-initial");
    assert_eq!(stored.refresh_token(), "refresh-initial");

    let snapshot = fixture
        .state
        .read()
        .await
        .get_agent(&fixture.agent_id)
        .expect("agent snapshot");
    let tool_names = snapshot
        .state
        .config
        .tools
        .expect("tools configured")
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<Vec<_>>();
    for name in CALENDAR_TOOL_NAMES {
        assert!(
            tool_names.iter().any(|tool| tool == name),
            "agent config should advertise {name}"
        );
    }
}

#[tokio::test]
async fn list_events_requires_an_active_connector() {
    let fixture = fixture(Some(GoogleOAuthConfig::for_tests())).await;
    let result = fixture
        .manager
        .list_events_for_agent(
            &fixture.agent_id,
            None,
            "2026-09-01T00:00:00Z",
            "2026-09-02T00:00:00Z",
        )
        .await;
    assert_eq!(result.unwrap_err(), CalendarError::NotConnected);

    connect(&fixture).await;
    fixture
        .transport
        .events
        .lock()
        .unwrap()
        .push(GoogleCalendarEvent {
            id: "event-1".to_string(),
            title: "Standup".to_string(),
            start: "2026-09-01T09:00:00Z".to_string(),
            end: "2026-09-01T09:15:00Z".to_string(),
            location: None,
            description: None,
        });
    let events = fixture
        .manager
        .list_events_for_agent(
            &fixture.agent_id,
            None,
            "2026-09-01T00:00:00Z",
            "2026-09-02T00:00:00Z",
        )
        .await
        .expect("events should list");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].title, "Standup");
}

#[tokio::test]
async fn expired_tokens_refresh_and_rejected_refresh_flags_reauth() {
    let fixture = fixture(Some(GoogleOAuthConfig::for_tests())).await;
    let record = connect(&fixture).await;
    // Force the stored token to be expired.
    fixture
        .credentials
        .put(
            &record.id,
            GoogleOAuthTokens::new(
                Zeroizing::new("access-stale".to_string()),
                Zeroizing::new("refresh-stale".to_string()),
                now_ms().saturating_sub(1_000),
            ),
        )
        .await
        .unwrap();
    *fixture.transport.refreshed.lock().unwrap() = Some(Ok(GoogleOAuthTokens::new(
        Zeroizing::new("access-fresh".to_string()),
        Zeroizing::new("refresh-fresh".to_string()),
        now_ms() + 3_600_000,
    )));
    fixture
        .manager
        .list_events_for_agent(&fixture.agent_id, None, "a", "b")
        .await
        .expect("refresh should recover access");
    assert_eq!(
        fixture
            .credentials
            .load(&record.id)
            .await
            .unwrap()
            .unwrap()
            .access_token(),
        "access-fresh"
    );

    // Now make the refresh get rejected: the connector must flag reauth.
    fixture
        .credentials
        .put(
            &record.id,
            GoogleOAuthTokens::new(
                Zeroizing::new("access-stale".to_string()),
                Zeroizing::new("refresh-stale".to_string()),
                now_ms().saturating_sub(1_000),
            ),
        )
        .await
        .unwrap();
    *fixture.transport.refreshed.lock().unwrap() = Some(Err(GoogleTransportError::Unauthorized));
    let result = fixture
        .manager
        .list_events_for_agent(&fixture.agent_id, None, "a", "b")
        .await;
    assert_eq!(result.unwrap_err(), CalendarError::ReauthRequired);
    let flagged = fixture
        .state
        .read()
        .await
        .calendar_connectors
        .get(&record.id)
        .expect("connector present")
        .reauth_required;
    assert!(flagged, "rejected refresh should flag reauthorization");
}

#[tokio::test]
async fn submit_write_validates_and_never_calls_google() {
    let fixture = fixture(Some(GoogleOAuthConfig::for_tests())).await;
    let result = fixture
        .manager
        .submit_write(&fixture.agent_id, CalendarWriteOperation::Create, draft())
        .await;
    assert_eq!(result.unwrap_err(), CalendarError::NotConnected);

    connect(&fixture).await;
    let write = fixture
        .manager
        .submit_write(&fixture.agent_id, CalendarWriteOperation::Create, draft())
        .await
        .expect("write should be recorded");
    assert_eq!(write.state, CalendarWriteState::Pending);
    assert_eq!(write.draft.calendar_id, "primary");
    assert!(write.summary.contains("Lunch with Sam"));
    assert!(
        fixture.transport.created.lock().unwrap().is_empty(),
        "submit must not call Google before approval"
    );

    let invalid = CalendarEventDraft {
        title: String::new(),
        ..draft()
    };
    let result = fixture
        .manager
        .submit_write(&fixture.agent_id, CalendarWriteOperation::Create, invalid)
        .await;
    assert_eq!(result.unwrap_err(), CalendarError::InvalidDraft);
}

#[tokio::test]
async fn approve_applies_once_and_reject_blocks_approval() {
    let fixture = fixture(Some(GoogleOAuthConfig::for_tests())).await;
    let record = connect(&fixture).await;
    let write = fixture
        .manager
        .submit_write(&fixture.agent_id, CalendarWriteOperation::Create, draft())
        .await
        .expect("write recorded");

    let approved = fixture
        .manager
        .approve_write(&fixture.agent_id, &record.id, &write.id)
        .await
        .expect("approval should apply");
    assert_eq!(approved.state, CalendarWriteState::Applied);
    assert_eq!(fixture.transport.created.lock().unwrap().len(), 1);

    let again = fixture
        .manager
        .approve_write(&fixture.agent_id, &record.id, &write.id)
        .await;
    assert_eq!(again.unwrap_err(), CalendarError::WriteNotPending);

    let second = fixture
        .manager
        .submit_write(&fixture.agent_id, CalendarWriteOperation::Create, draft())
        .await
        .expect("second write recorded");
    let rejected = fixture
        .manager
        .reject_write(&fixture.agent_id, &record.id, &second.id)
        .await
        .expect("rejection should succeed");
    assert_eq!(rejected.state, CalendarWriteState::Rejected);
    let after_reject = fixture
        .manager
        .approve_write(&fixture.agent_id, &record.id, &second.id)
        .await;
    assert_eq!(after_reject.unwrap_err(), CalendarError::WriteNotPending);
    assert_eq!(
        fixture.transport.created.lock().unwrap().len(),
        1,
        "rejected writes must never reach Google"
    );
}

#[tokio::test]
async fn failed_apply_marks_write_failed_with_sanitized_error() {
    let fixture = fixture(Some(GoogleOAuthConfig::for_tests())).await;
    let record = connect(&fixture).await;
    let write = fixture
        .manager
        .submit_write(&fixture.agent_id, CalendarWriteOperation::Create, draft())
        .await
        .expect("write recorded");
    *fixture.transport.apply_error.lock().unwrap() = Some(GoogleTransportError::Unavailable);

    let applied = fixture
        .manager
        .approve_write(&fixture.agent_id, &record.id, &write.id)
        .await
        .expect("failed apply still resolves the write");
    assert_eq!(applied.state, CalendarWriteState::Failed);
    let error = applied.error.expect("error recorded");
    assert!(
        !error.contains("access-initial"),
        "error must not leak tokens"
    );
}

#[tokio::test]
async fn disconnect_tombstones_connector_rejects_pending_and_strips_tools() {
    let fixture = fixture(Some(GoogleOAuthConfig::for_tests())).await;
    let record = connect(&fixture).await;
    let write = fixture
        .manager
        .submit_write(&fixture.agent_id, CalendarWriteOperation::Create, draft())
        .await
        .expect("write recorded");

    fixture
        .manager
        .disconnect(&fixture.agent_id, &record.id)
        .await
        .expect("disconnect should succeed");

    let guard = fixture.state.read().await;
    let tombstoned = guard
        .calendar_connectors
        .get(&record.id)
        .expect("connector retained as tombstone");
    assert!(!tombstoned.enabled);
    assert!(tombstoned.deleted_at_ms.is_some());
    let write_after = guard
        .calendar_writes
        .get(&write.id)
        .expect("write retained");
    assert_eq!(write_after.state, CalendarWriteState::Rejected);
    let tool_names = guard
        .get_agent(&fixture.agent_id)
        .expect("agent snapshot")
        .state
        .config
        .tools
        .map(|tools| {
            tools
                .iter()
                .map(|tool| tool.name.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for name in CALENDAR_TOOL_NAMES {
        assert!(!tool_names.iter().any(|tool| tool == name));
    }
    drop(guard);
    assert!(fixture
        .credentials
        .load(&record.id)
        .await
        .expect("vault read")
        .is_none());
}

#[tokio::test]
async fn calendar_state_round_trips_through_the_control_plane_snapshot() {
    const SENTINEL: &str = "google-refresh-sentinel";
    let fixture = fixture(Some(GoogleOAuthConfig::for_tests())).await;
    let record = connect(&fixture).await;
    fixture
        .credentials
        .put(
            &record.id,
            GoogleOAuthTokens::new(
                Zeroizing::new("access".to_string()),
                Zeroizing::new(SENTINEL.to_string()),
                now_ms() + 3_600_000,
            ),
        )
        .await
        .unwrap();
    fixture
        .manager
        .submit_write(&fixture.agent_id, CalendarWriteOperation::Create, draft())
        .await
        .expect("write recorded");

    let snapshot = fixture.state.read().await.control_plane_snapshot();
    let serialized = serde_json::to_string(&snapshot).expect("snapshot serializes");
    assert!(
        !serialized.contains(SENTINEL),
        "snapshot must never contain OAuth tokens"
    );
    let restored: crate::control_plane_store::ControlPlaneSnapshot =
        serde_json::from_str(&serialized).expect("snapshot deserializes");

    let mut target = DaemonState::new();
    target
        .restore_control_plane_snapshot(restored)
        .expect("calendar state should restore");
    assert!(target.calendar_connectors.contains_key(&record.id));
    assert_eq!(target.calendar_writes.len(), 1);
}

#[tokio::test]
async fn list_events_tool_reports_connection_guidance_when_unconnected() {
    let fixture = fixture(Some(GoogleOAuthConfig::for_tests())).await;
    let (context, agent) = {
        let mut guard = fixture.state.write().await;
        let (runtime, context) = guard
            .take_agent_runtime(&fixture.agent_id)
            .expect("runtime available");
        (context, runtime.state().clone())
    };
    let handler = crate::tools::ToolRegistry::new()
        .lookup("calendar_list_events")
        .unwrap();
    let result = handler(
        context,
        agent,
        tool_message(),
        ToolCall {
            id: "call-list".to_string(),
            name: "calendar_list_events".to_string(),
            args: BTreeMap::from([
                (
                    "time_min".to_string(),
                    DataValue::String("2026-09-01T00:00:00Z".to_string()),
                ),
                (
                    "time_max".to_string(),
                    DataValue::String("2026-09-02T00:00:00Z".to_string()),
                ),
            ]),
        },
    )
    .await;
    let message = result.error.expect("tool should report an error");
    assert!(
        message.contains("not connected"),
        "unexpected message: {message}"
    );
}

#[tokio::test]
async fn create_tool_records_pending_write_without_calling_google() {
    let fixture = fixture(Some(GoogleOAuthConfig::for_tests())).await;
    connect(&fixture).await;
    let (context, agent) = {
        let mut guard = fixture.state.write().await;
        let (runtime, context) = guard
            .take_agent_runtime(&fixture.agent_id)
            .expect("runtime available");
        (context, runtime.state().clone())
    };
    let handler = crate::tools::ToolRegistry::new()
        .lookup("calendar_create_event")
        .unwrap();
    let result = handler(
        context,
        agent,
        tool_message(),
        ToolCall {
            id: "call-create".to_string(),
            name: "calendar_create_event".to_string(),
            args: BTreeMap::from([
                (
                    "title".to_string(),
                    DataValue::String("Dentist".to_string()),
                ),
                (
                    "start".to_string(),
                    DataValue::String("2026-09-03T15:00:00Z".to_string()),
                ),
                (
                    "end".to_string(),
                    DataValue::String("2026-09-03T15:30:00Z".to_string()),
                ),
            ]),
        },
    )
    .await;
    let content = result.data.expect("tool should succeed");
    assert!(content.text.contains("pending owner confirmation"));
    assert!(content.text.contains("Dentist"));
    assert!(fixture.transport.created.lock().unwrap().is_empty());

    let guard = fixture.state.read().await;
    assert_eq!(guard.calendar_writes.len(), 1);
}
