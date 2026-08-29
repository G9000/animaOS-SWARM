mod runtime_events;
mod swarm_relationships;
mod swarm_runtime;
mod swarm_tools;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex as StdMutex;

use anima_core::{
    AgentConfig, AgentConfigUpdate, AgentRuntime, AgentRuntimeSnapshot, AgentStatus,
    DatabaseAdapter, MessageRole, ModelAdapter, ToolDescriptor,
};
use anima_memory::{locomo_query_expander, MemoryManager, QueryExpander, TextAnalyzer};
use anima_swarm::coordinator::CoordinatorMessageEventFn;
use anima_swarm::strategies::resolve_strategy;
use anima_swarm::{SwarmConfig, SwarmCoordinator, SwarmState};
#[cfg(test)]
use tokio::sync::Semaphore;
use tokio::sync::{Mutex as AsyncMutex, RwLock as AsyncRwLock};
use tracing::warn;

use crate::components::{default_evaluators, default_providers};
use crate::connectors::{
    InboundProcessingState, OutboundDeliveryState, TelegramConnectorRecord,
    TelegramCredentialCleanupIntent, TelegramInboundRecord, TelegramOutboundRecord,
};
use crate::control_plane_store::{
    save_control_plane_snapshot, ControlPlaneSnapshot, ControlPlaneStoreConfig, StoredSwarmSnapshot,
};
use crate::events::{EventFanout, EventSubscriber, DEFAULT_EVENT_BUFFER};
use crate::memory_embeddings::{MemoryEmbeddingRuntime, SharedMemoryEmbeddings};
use crate::memory_store::MemoryStoreConfig;
use crate::model::DeterministicModelAdapter;
use crate::schedules::{ScheduleTarget, ScheduledPromptRecord};
use crate::tools::{
    background_process_count, new_shared_process_manager_with_limit, SharedProcessManager,
    ToolExecutionContext, ToolRegistry, DEFAULT_MAX_BACKGROUND_PROCESSES,
};

use self::swarm_relationships::{persist_swarm_message_relationship, swarm_agent_names};

pub(crate) type SharedMemoryStore = Arc<AsyncRwLock<MemoryManager>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UpdateAgentError {
    InvalidTools(String),
    NotFound,
}

#[cfg(test)]
mod tests {
    use super::DaemonState;
    use crate::connectors::{
        InboundProcessingState, OutboundDeliveryState, TelegramBotIdentity, TelegramChatKind,
        TelegramChatMetadata, TelegramConnectorRecord, TelegramCredentialCleanupIntent,
        TelegramInboundRecord, TelegramOutboundRecord, TelegramPendingPairing,
        TelegramSenderMetadata,
    };
    use crate::control_plane_store::{
        load_control_plane_snapshot, ControlPlaneSnapshot, ControlPlaneStoreConfig,
    };
    use crate::schedules::{
        ScheduleLastFired, ScheduleOutcomeStatus, ScheduleSafeOutcome, ScheduleTarget,
        ScheduleTrigger, ScheduledPromptRecord,
    };
    use anima_core::{
        AgentConfig, AgentRuntime, AgentSettings, Content, Message, MessageRole, ToolDescriptor,
    };
    use std::sync::Arc;

    #[tokio::test]
    async fn stale_delayed_persist_request_cannot_overwrite_newer_snapshot() {
        let store_path = std::env::temp_dir().join(format!(
            "anima-persist-order-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos()
        ));
        let store_config = ControlPlaneStoreConfig::Json(store_path.clone());
        let mut state = DaemonState::new();
        state.set_control_plane_store(Some(store_config.clone()));
        state
            .create_agent(test_config("older"))
            .expect("first agent should be created");
        let older_request = state.control_plane_persist_request();
        state
            .create_agent(test_config("newer"))
            .expect("second agent should be created");
        let newer_request = state.control_plane_persist_request();

        let (release_older, wait_for_release) = tokio::sync::oneshot::channel();
        let older_save = tokio::spawn(async move {
            wait_for_release.await.expect("old save should be released");
            older_request.save().await
        });
        newer_request
            .save()
            .await
            .expect("newer snapshot should save first");
        release_older
            .send(())
            .expect("old save task should still be waiting");
        older_save
            .await
            .expect("old save task should join")
            .expect("stale save should be a successful no-op");

        let persisted = load_control_plane_snapshot(&store_config)
            .await
            .expect("snapshot should load")
            .expect("snapshot should exist");
        let mut restored = DaemonState::new();
        restored
            .restore_control_plane_snapshot(persisted)
            .expect("newest snapshot should restore");
        assert_eq!(restored.agent_count(), 2);
        let mut restored_names = restored
            .list_agents()
            .into_iter()
            .map(|snapshot| snapshot.state.config.name)
            .collect::<Vec<_>>();
        restored_names.sort();
        assert_eq!(
            restored_names,
            ["newer", "older"],
            "the newest persisted snapshot should retain both agents"
        );

        let _ = std::fs::remove_file(store_path);
    }

    #[tokio::test]
    async fn failed_newer_persist_request_does_not_suppress_older_snapshot() {
        let unique = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos()
        );
        let valid_path = std::env::temp_dir().join(format!("anima-persist-fallback-{unique}.json"));
        let invalid_path = std::env::temp_dir().join(format!("anima-persist-failure-{unique}"));
        std::fs::create_dir(&invalid_path)
            .expect("invalid file target directory should be created");

        let valid_store = ControlPlaneStoreConfig::Json(valid_path.clone());
        let mut state = DaemonState::new();
        state.set_control_plane_store(Some(valid_store.clone()));
        state
            .create_agent(test_config("durable-older"))
            .expect("first agent should be created");
        let older_request = state.control_plane_persist_request();

        state.set_control_plane_store(Some(ControlPlaneStoreConfig::Json(invalid_path.clone())));
        state
            .create_agent(test_config("failed-newer"))
            .expect("second agent should be created");
        let newer_request = state.control_plane_persist_request();
        assert!(
            newer_request.save().await.is_err(),
            "writing a snapshot to a directory should fail"
        );
        older_request
            .save()
            .await
            .expect("an older pending snapshot should remain eligible after a newer save fails");

        let persisted = load_control_plane_snapshot(&valid_store)
            .await
            .expect("fallback snapshot should load")
            .expect("fallback snapshot should exist");
        assert_eq!(persisted.agents.len(), 1);
        assert_eq!(persisted.agents[0].state.config.name, "durable-older");

        let _ = std::fs::remove_file(valid_path);
        let _ = std::fs::remove_dir(invalid_path);
    }

    #[test]
    fn restore_deduplicates_and_canonicalizes_legacy_agent_tools() {
        let source = DaemonState::new();
        let mut config = test_config("legacy");
        config.tools = Some(vec![
            forged_tool("read_file", "forged first"),
            forged_tool("read_file", "forged duplicate"),
            forged_tool("bash", "forged bash"),
        ]);
        let legacy_snapshot =
            AgentRuntime::new(config, Arc::clone(&source.model_adapter)).snapshot();
        let mut restored = DaemonState::new();

        restored
            .restore_control_plane_snapshot(ControlPlaneSnapshot::new(
                vec![legacy_snapshot],
                vec![],
            ))
            .expect("legacy duplicate descriptors should not abort daemon startup");

        let snapshot = restored
            .list_agents()
            .pop()
            .expect("legacy agent should restore");
        let tools = snapshot
            .state
            .config
            .tools
            .expect("restored tools should exist");
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["read_file", "bash"]
        );
        assert_eq!(
            tools[0],
            restored
                .tool_registry
                .descriptor("read_file")
                .expect("read_file should be registered")
        );
        assert_eq!(
            tools[1],
            restored
                .tool_registry
                .descriptor("bash")
                .expect("bash should be registered")
        );
    }

    #[test]
    fn restore_rejects_unknown_connector_without_mutating_existing_agents() {
        let mut state = DaemonState::new();
        state
            .create_agent(test_config("existing"))
            .expect("existing agent should be created");
        let invalid_snapshot: ControlPlaneSnapshot = serde_json::from_value(serde_json::json!({
            "version": 1,
            "agents": [],
            "swarms": [],
            "connectors": [{
                "id": "telegram-1",
                "agentId": "missing-agent",
                "roomId": "room-1",
                "bot": { "id": "bot-1", "username": "test-bot" },
                "approvedChat": null,
                "pendingPairing": null,
                "nextUpdateId": 0,
                "enabled": true,
                "createdAtMs": 1,
                "updatedAtMs": 1
            }]
        }))
        .expect("invalid connector snapshot should deserialize");

        assert!(
            state
                .restore_control_plane_snapshot(invalid_snapshot)
                .is_err(),
            "an unknown connector agent must reject the full restore"
        );
        assert_eq!(
            state.agent_count(),
            1,
            "failed restores must not mutate state"
        );
    }

    #[test]
    fn connector_and_schedule_records_round_trip_without_serializing_secrets() {
        const SENTINEL_TOKEN: &str = "never-persist-a-telegram-token";

        let mut source = DaemonState::new();
        let agent_id = source
            .create_agent(test_config("connector-owner"))
            .expect("agent should be created")
            .state
            .id;
        let connector = test_connector("telegram-a", &agent_id);
        let inbound = test_inbound(&agent_id);
        let outbound = test_outbound(&agent_id);
        let schedule = test_schedule(&agent_id);
        add_persisted_room_assistant_message(
            &mut source,
            &agent_id,
            "room-1",
            &outbound.assistant_message_id,
        );
        source
            .connectors
            .insert(connector.id.clone(), connector.clone());
        source.inbound.insert(
            (inbound.connector_id.clone(), inbound.update_id),
            inbound.clone(),
        );
        source
            .outbound
            .insert(outbound.id.clone(), outbound.clone());
        source
            .schedules
            .insert(schedule.id.clone(), schedule.clone());

        let snapshot = source.control_plane_snapshot();
        let serialized = serde_json::to_string(&snapshot).expect("snapshot should serialize");
        assert!(
            !serialized.contains(SENTINEL_TOKEN),
            "credentials held outside non-secret records must never serialize"
        );
        let snapshot: ControlPlaneSnapshot =
            serde_json::from_str(&serialized).expect("snapshot should deserialize");

        let mut restored = DaemonState::new();
        restored
            .restore_control_plane_snapshot(snapshot)
            .expect("connector state should restore");
        assert_eq!(restored.connectors.get("telegram-a"), Some(&connector));
        assert_eq!(
            restored.inbound.get(&(String::from("telegram-a"), 42)),
            Some(&inbound)
        );
        assert_eq!(restored.outbound.get("outbound-1"), Some(&outbound));
        assert_eq!(restored.schedules.get("schedule-1"), Some(&schedule));
    }

    #[test]
    fn snapshot_sorts_connector_records_deterministically() {
        let mut state = DaemonState::new();
        let agent_id = state
            .create_agent(test_config("connector-owner"))
            .expect("agent should be created")
            .state
            .id;
        state
            .connectors
            .insert("telegram-z".into(), test_connector("telegram-z", &agent_id));
        state
            .connectors
            .insert("telegram-a".into(), test_connector("telegram-a", &agent_id));

        let snapshot = state.control_plane_snapshot();
        assert_eq!(
            snapshot
                .connectors
                .iter()
                .map(|connector| connector.id.as_str())
                .collect::<Vec<_>>(),
            ["telegram-a", "telegram-z"]
        );
    }

    #[test]
    fn cleanup_intents_round_trip_and_sort_without_connector_metadata() {
        let mut source = DaemonState::new();
        for connector_id in ["telegram-z", "telegram-a"] {
            source.credential_cleanup.insert(
                connector_id.into(),
                TelegramCredentialCleanupIntent {
                    connector_id: connector_id.into(),
                    created_at_ms: 1,
                },
            );
        }

        let snapshot = source.control_plane_snapshot();
        assert_eq!(
            snapshot
                .credential_cleanup
                .iter()
                .map(|intent| intent.connector_id.as_str())
                .collect::<Vec<_>>(),
            ["telegram-a", "telegram-z"]
        );
        assert!(snapshot.connectors.is_empty());

        let mut restored = DaemonState::new();
        restored
            .restore_control_plane_snapshot(snapshot)
            .expect("non-secret credential cleanup intents should restore");
        assert_eq!(restored.credential_cleanup.len(), 2);
    }

    #[test]
    fn snapshot_sorts_inbound_outbound_and_schedules_deterministically() {
        let mut state = DaemonState::new();
        let agent_id = state
            .create_agent(test_config("connector-owner"))
            .expect("agent should be created")
            .state
            .id;
        state
            .connectors
            .insert("telegram-z".into(), test_connector("telegram-z", &agent_id));
        state
            .connectors
            .insert("telegram-a".into(), test_connector("telegram-a", &agent_id));

        let mut inbound_z = test_inbound(&agent_id);
        inbound_z.connector_id = "telegram-z".into();
        inbound_z.update_id = 9;
        let mut inbound_a_later = test_inbound(&agent_id);
        inbound_a_later.update_id = 7;
        let inbound_a_earlier = test_inbound(&agent_id);
        state.inbound.insert(
            (inbound_z.connector_id.clone(), inbound_z.update_id),
            inbound_z,
        );
        state.inbound.insert(
            (
                inbound_a_later.connector_id.clone(),
                inbound_a_later.update_id,
            ),
            inbound_a_later,
        );
        state.inbound.insert(
            (
                inbound_a_earlier.connector_id.clone(),
                inbound_a_earlier.update_id,
            ),
            inbound_a_earlier,
        );

        let mut outbound_z = test_outbound(&agent_id);
        outbound_z.id = "outbound-z".into();
        outbound_z.connector_id = "telegram-z".into();
        let outbound_a = test_outbound(&agent_id);
        state.outbound.insert(outbound_z.id.clone(), outbound_z);
        state.outbound.insert(outbound_a.id.clone(), outbound_a);

        let mut schedule_z = test_schedule(&agent_id);
        schedule_z.id = "schedule-z".into();
        schedule_z.target = ScheduleTarget::Workspace;
        let schedule_a = test_schedule(&agent_id);
        state.schedules.insert(schedule_z.id.clone(), schedule_z);
        state.schedules.insert(schedule_a.id.clone(), schedule_a);

        let snapshot = state.control_plane_snapshot();
        assert_eq!(
            snapshot
                .inbound
                .iter()
                .map(|record| (record.connector_id.as_str(), record.update_id))
                .collect::<Vec<_>>(),
            [("telegram-a", 7), ("telegram-a", 42), ("telegram-z", 9)]
        );
        assert_eq!(
            snapshot
                .outbound
                .iter()
                .map(|record| record.id.as_str())
                .collect::<Vec<_>>(),
            ["outbound-1", "outbound-z"]
        );
        assert_eq!(
            snapshot
                .schedules
                .iter()
                .map(|record| record.id.as_str())
                .collect::<Vec<_>>(),
            ["schedule-1", "schedule-z"]
        );
    }

    #[test]
    fn persisted_connector_and_schedule_contract_uses_required_json_names() {
        let connector = test_connector("telegram-a", "agent-a");
        let inbound = test_inbound("agent-a");
        let outbound = test_outbound("agent-a");
        let schedule = test_schedule("agent-a");
        assert_u64(connector.created_at_ms);
        assert_u64(connector.updated_at_ms);
        assert_u64(
            connector
                .pending_pairing
                .as_ref()
                .expect("test connector has a pairing")
                .requested_at_ms,
        );
        assert_u64(inbound.received_at_ms);
        assert_u64(outbound.created_at_ms);
        assert_u64(schedule.next_due_at_ms);
        assert_u64(schedule.created_at_ms);
        assert_u64(schedule.updated_at_ms);
        assert_u64(
            schedule
                .last_fired
                .as_ref()
                .expect("test schedule has a last-fired record")
                .fired_at_ms,
        );
        assert_u64(
            schedule
                .last_safe_outcome
                .as_ref()
                .expect("test schedule has an outcome")
                .occurred_at_ms,
        );
        let connector_json = serde_json::to_value(&connector).expect("connector should serialize");
        assert!(connector_json.get("bot").is_some());
        assert!(connector_json.get("pendingPairing").is_some());
        assert!(connector_json.get("botIdentity").is_none());
        assert!(connector_json.get("latestPendingPairing").is_none());

        let interval_trigger = ScheduleTrigger::Interval {
            interval_ms: 60_000,
        };
        let ScheduleTrigger::Interval { interval_ms } = &interval_trigger else {
            unreachable!("test trigger must be interval");
        };
        assert_u64(*interval_ms);
        assert_eq!(
            serde_json::to_value(interval_trigger).expect("interval trigger should serialize"),
            serde_json::json!({"interval": {"intervalMs": 60_000}})
        );
        assert_eq!(
            serde_json::to_value(ScheduleTrigger::Daily {
                hour: 9,
                minute: 30,
                time_zone: "Asia/Kuala_Lumpur".into(),
            })
            .expect("daily trigger should serialize"),
            serde_json::json!({
                "daily": {
                    "hour": 9,
                    "minute": 30,
                    "timeZone": "Asia/Kuala_Lumpur"
                }
            })
        );
        assert_eq!(
            serde_json::to_value(ScheduleTarget::Connector {
                connector_id: "telegram-a".into(),
            })
            .expect("connector target should serialize"),
            serde_json::json!({"connector": {"connectorId": "telegram-a"}})
        );
    }

    #[test]
    fn version_one_snapshot_restores_with_empty_connector_state() {
        let snapshot: ControlPlaneSnapshot = serde_json::from_value(serde_json::json!({
            "version": 1,
            "agents": [],
            "swarms": []
        }))
        .expect("version-one snapshot should deserialize");
        let mut state = DaemonState::new();

        state
            .restore_control_plane_snapshot(snapshot)
            .expect("version-one snapshot should restore");
        assert!(state.connectors.is_empty());
        assert!(state.inbound.is_empty());
        assert!(state.outbound.is_empty());
        assert!(state.schedules.is_empty());
    }

    #[test]
    fn restore_rejects_duplicate_and_inconsistent_connector_records_without_mutation() {
        let (snapshot, other_agent_id) = valid_connector_snapshot();
        let mut duplicate_connector = snapshot.clone();
        duplicate_connector
            .connectors
            .push(duplicate_connector.connectors[0].clone());

        let mut connector_missing_agent = snapshot.clone();
        connector_missing_agent.connectors[0].agent_id = "missing-agent".into();

        let mut inbound_missing_connector = snapshot.clone();
        inbound_missing_connector.inbound[0].connector_id = "missing-connector".into();

        let mut outbound_missing_agent = snapshot.clone();
        outbound_missing_agent.outbound[0].agent_id = "missing-agent".into();

        let mut outbound_wrong_owner = snapshot.clone();
        outbound_wrong_owner.outbound[0].agent_id = other_agent_id;

        let mut schedule_missing_connector = snapshot.clone();
        schedule_missing_connector.schedules[0].target = ScheduleTarget::Connector {
            connector_id: "missing-connector".into(),
        };

        let mut blank_cleanup_intent = snapshot.clone();
        blank_cleanup_intent
            .credential_cleanup
            .push(TelegramCredentialCleanupIntent {
                connector_id: " ".into(),
                created_at_ms: 1,
            });

        let mut zero_time_cleanup_intent = snapshot.clone();
        zero_time_cleanup_intent
            .credential_cleanup
            .push(TelegramCredentialCleanupIntent {
                connector_id: "orphan-zero".into(),
                created_at_ms: 0,
            });

        let mut duplicate_cleanup_intent = snapshot.clone();
        let duplicate = TelegramCredentialCleanupIntent {
            connector_id: "orphan-duplicate".into(),
            created_at_ms: 1,
        };
        duplicate_cleanup_intent
            .credential_cleanup
            .extend([duplicate.clone(), duplicate]);

        let mut conflicting_cleanup_intent = snapshot;
        conflicting_cleanup_intent
            .credential_cleanup
            .push(TelegramCredentialCleanupIntent {
                connector_id: conflicting_cleanup_intent.connectors[0].id.clone(),
                created_at_ms: 1,
            });

        for invalid_snapshot in [
            duplicate_connector,
            connector_missing_agent,
            inbound_missing_connector,
            outbound_missing_agent,
            outbound_wrong_owner,
            schedule_missing_connector,
            blank_cleanup_intent,
            zero_time_cleanup_intent,
            duplicate_cleanup_intent,
            conflicting_cleanup_intent,
        ] {
            let mut state = DaemonState::new();
            assert!(
                state
                    .restore_control_plane_snapshot(invalid_snapshot)
                    .is_err(),
                "invalid connector records must reject the full restore"
            );
            assert_eq!(state.agent_count(), 0, "invalid restores cannot add agents");
            assert!(state.connectors.is_empty());
            assert!(state.credential_cleanup.is_empty());
            assert!(state.inbound.is_empty());
            assert!(state.outbound.is_empty());
            assert!(state.schedules.is_empty());
        }
    }

    #[test]
    fn restore_rejects_invalid_durable_identities_and_schedule_invariants_without_mutation() {
        let (snapshot, _) = valid_connector_snapshot();

        let mut empty_inbound_idempotency = snapshot.clone();
        empty_inbound_idempotency.inbound[0].run_idempotency_key = " ".into();

        let mut duplicate_inbound_idempotency = snapshot.clone();
        let mut duplicate_inbound = duplicate_inbound_idempotency.inbound[0].clone();
        duplicate_inbound.update_id = 43;
        duplicate_inbound_idempotency
            .inbound
            .push(duplicate_inbound);

        let mut blank_import_idempotency = snapshot.clone();
        blank_import_idempotency.schedules[0].import_idempotency_key = Some(" ".into());

        let mut duplicate_import_idempotency = snapshot.clone();
        let mut duplicate_schedule = duplicate_import_idempotency.schedules[0].clone();
        duplicate_schedule.id = "schedule-2".into();
        duplicate_import_idempotency
            .schedules
            .push(duplicate_schedule);

        let mut dangling_assistant = snapshot.clone();
        dangling_assistant.outbound[0].assistant_message_id = "missing-message".into();

        let mut wrong_role_assistant = snapshot.clone();
        assistant_message_for_outbound(&mut wrong_role_assistant).role = MessageRole::User;

        let mut wrong_room_assistant = snapshot.clone();
        assistant_message_for_outbound(&mut wrong_room_assistant).room_id = "other-room".into();

        let mut duplicate_delivery_identity = snapshot.clone();
        let mut duplicate_delivery = duplicate_delivery_identity.outbound[0].clone();
        duplicate_delivery.id = "outbound-2".into();
        duplicate_delivery_identity
            .outbound
            .push(duplicate_delivery);

        let mut blank_prompt = snapshot.clone();
        blank_prompt.schedules[0].prompt = " \t".into();

        let mut zero_interval = snapshot.clone();
        zero_interval.schedules[0].trigger = ScheduleTrigger::Interval { interval_ms: 0 };

        let mut invalid_daily_hour = snapshot.clone();
        invalid_daily_hour.schedules[0].trigger = ScheduleTrigger::Daily {
            hour: 24,
            minute: 0,
            time_zone: "UTC".into(),
        };

        let mut invalid_daily_minute = snapshot.clone();
        invalid_daily_minute.schedules[0].trigger = ScheduleTrigger::Daily {
            hour: 0,
            minute: 60,
            time_zone: "UTC".into(),
        };

        let mut blank_daily_time_zone = snapshot.clone();
        blank_daily_time_zone.schedules[0].trigger = ScheduleTrigger::Daily {
            hour: 0,
            minute: 0,
            time_zone: " ".into(),
        };

        let mut invalid_schedule_timing = snapshot.clone();
        invalid_schedule_timing.schedules[0].updated_at_ms = 9;

        let mut zero_next_due = snapshot.clone();
        zero_next_due.schedules[0].next_due_at_ms = 0;

        let mut zero_last_fired_time = snapshot.clone();
        zero_last_fired_time.schedules[0]
            .last_fired
            .as_mut()
            .expect("fixture has a last-fired record")
            .fired_at_ms = 0;

        let mut zero_outcome_time = snapshot.clone();
        zero_outcome_time.schedules[0]
            .last_safe_outcome
            .as_mut()
            .expect("fixture has an outcome")
            .occurred_at_ms = 0;

        let mut empty_last_fired_key = snapshot;
        empty_last_fired_key.schedules[0]
            .last_fired
            .as_mut()
            .expect("fixture has a last-fired record")
            .run_idempotency_key = " ".into();

        for invalid_snapshot in [
            empty_inbound_idempotency,
            duplicate_inbound_idempotency,
            blank_import_idempotency,
            duplicate_import_idempotency,
            dangling_assistant,
            wrong_role_assistant,
            wrong_room_assistant,
            duplicate_delivery_identity,
            blank_prompt,
            zero_interval,
            invalid_daily_hour,
            invalid_daily_minute,
            blank_daily_time_zone,
            invalid_schedule_timing,
            zero_next_due,
            zero_last_fired_time,
            zero_outcome_time,
            empty_last_fired_key,
        ] {
            let mut state = DaemonState::new();
            assert!(
                state
                    .restore_control_plane_snapshot(invalid_snapshot)
                    .is_err(),
                "invalid durable identity or schedule state must reject the full restore"
            );
            assert_eq!(state.agent_count(), 0, "invalid restores cannot add agents");
            assert!(state.connectors.is_empty());
            assert!(state.inbound.is_empty());
            assert!(state.outbound.is_empty());
            assert!(state.schedules.is_empty());
        }
    }

    #[test]
    fn restore_allows_an_overdue_schedule_next_due_time() {
        let (mut snapshot, _) = valid_connector_snapshot();
        snapshot.schedules[0].next_due_at_ms = 1;

        let mut state = DaemonState::new();
        state
            .restore_control_plane_snapshot(snapshot)
            .expect("an overdue next due time remains a valid schedule");
        assert_eq!(state.schedules.len(), 1);
    }

    #[test]
    fn restore_does_not_validate_inbound_against_a_connector_absent_from_snapshot() {
        let mut state = DaemonState::new();
        let agent_id = state
            .create_agent(test_config("connector-owner"))
            .expect("agent should be created")
            .state
            .id;
        let existing_connector = test_connector("telegram-a", &agent_id);
        state
            .connectors
            .insert(existing_connector.id.clone(), existing_connector);
        let inbound = test_inbound(&agent_id);
        let snapshot = ControlPlaneSnapshot::with_connector_state(
            vec![],
            vec![],
            vec![],
            vec![inbound],
            vec![],
            vec![],
        );

        assert!(state.restore_control_plane_snapshot(snapshot).is_err());
        assert_eq!(
            state.agent_count(),
            1,
            "failed restores cannot add or remove agents"
        );
        assert!(
            state.connectors.contains_key("telegram-a"),
            "failed restores cannot clear existing connector state"
        );
        assert!(state.inbound.is_empty());
    }

    #[test]
    fn tombstoned_connector_history_restores_after_its_agent_is_deleted() {
        let (mut snapshot, _) = valid_connector_snapshot();
        snapshot.connectors[0].enabled = false;
        snapshot.connectors[0].deleted_at_ms = Some(20);
        snapshot.connectors[0].updated_at_ms = 20;
        snapshot.connectors[0].pending_pairing = None;
        snapshot.inbound[0].processing_state = InboundProcessingState::Processed;
        snapshot.outbound[0].delivery_state = OutboundDeliveryState::Delivered;
        snapshot.outbound[0].delivered_at_ms = Some(19);
        snapshot.schedules[0].enabled = false;
        snapshot.schedules[0].updated_at_ms = 20;
        snapshot.agents.clear();

        let mut restored = DaemonState::new();
        restored
            .restore_control_plane_snapshot(snapshot)
            .expect("archived Telegram history should not keep a deleted agent alive");
        assert_eq!(restored.connectors.len(), 1);
        assert_eq!(restored.inbound.len(), 1);
        assert_eq!(restored.outbound.len(), 1);
        assert_eq!(restored.schedules.len(), 1);
    }

    #[test]
    fn enabled_schedule_cannot_target_a_tombstoned_connector() {
        let (mut snapshot, _) = valid_connector_snapshot();
        snapshot.connectors[0].enabled = false;
        snapshot.connectors[0].deleted_at_ms = Some(20);
        snapshot.connectors[0].updated_at_ms = 20;

        let mut restored = DaemonState::new();
        assert!(restored.restore_control_plane_snapshot(snapshot).is_err());
    }

    #[test]
    fn tombstoned_connector_rejects_live_pairing_inbox_and_outbox_state() {
        let (mut snapshot, _) = valid_connector_snapshot();
        snapshot.connectors[0].enabled = false;
        snapshot.connectors[0].deleted_at_ms = Some(20);
        snapshot.connectors[0].updated_at_ms = 20;
        snapshot.connectors[0].pending_pairing = None;
        snapshot.inbound[0].processing_state = InboundProcessingState::Processed;
        snapshot.outbound[0].delivery_state = OutboundDeliveryState::Delivered;
        snapshot.outbound[0].delivered_at_ms = Some(19);
        snapshot.schedules[0].enabled = false;
        snapshot.schedules[0].updated_at_ms = 20;

        let mut pending_pairing = snapshot.clone();
        pending_pairing.connectors[0].pending_pairing = Some(TelegramPendingPairing {
            chat: test_chat(),
            requested_at_ms: 19,
        });
        assert!(DaemonState::new()
            .restore_control_plane_snapshot(pending_pairing)
            .is_err());

        let mut processing_inbound = snapshot.clone();
        processing_inbound.inbound[0].processing_state = InboundProcessingState::Processing;
        assert!(DaemonState::new()
            .restore_control_plane_snapshot(processing_inbound)
            .is_err());

        let mut pending_outbound = snapshot;
        pending_outbound.outbound[0].delivery_state = OutboundDeliveryState::Pending;
        pending_outbound.outbound[0].delivered_at_ms = None;
        assert!(DaemonState::new()
            .restore_control_plane_snapshot(pending_outbound)
            .is_err());
    }

    fn valid_connector_snapshot() -> (ControlPlaneSnapshot, String) {
        let mut source = DaemonState::new();
        let agent_id = source
            .create_agent(test_config("connector-owner"))
            .expect("agent should be created")
            .state
            .id;
        let other_agent_id = source
            .create_agent(test_config("other-agent"))
            .expect("other agent should be created")
            .state
            .id;
        let connector = test_connector("telegram-a", &agent_id);
        let inbound = test_inbound(&agent_id);
        let outbound = test_outbound(&agent_id);
        let schedule = test_schedule(&agent_id);
        add_persisted_room_assistant_message(
            &mut source,
            &agent_id,
            "room-1",
            &outbound.assistant_message_id,
        );
        source.connectors.insert(connector.id.clone(), connector);
        source
            .inbound
            .insert((inbound.connector_id.clone(), inbound.update_id), inbound);
        source.outbound.insert(outbound.id.clone(), outbound);
        source.schedules.insert(schedule.id.clone(), schedule);

        (source.control_plane_snapshot(), other_agent_id)
    }

    fn add_persisted_room_assistant_message(
        state: &mut DaemonState,
        agent_id: &str,
        room_id: &str,
        message_id: &str,
    ) {
        state.agents.remove(agent_id);
        let snapshot = state
            .agent_snapshots
            .get_mut(agent_id)
            .expect("fixture agent snapshot should exist");
        snapshot.messages.push(Message {
            id: message_id.into(),
            agent_id: agent_id.into(),
            room_id: room_id.into(),
            content: Content {
                text: "persisted assistant response".into(),
                ..Content::default()
            },
            role: MessageRole::Assistant,
            created_at_ms: 13,
        });
        snapshot.message_count = snapshot.messages.len();
    }

    fn assistant_message_for_outbound(snapshot: &mut ControlPlaneSnapshot) -> &mut Message {
        let (agent_id, assistant_message_id) = snapshot
            .outbound
            .first()
            .map(|outbound| {
                (
                    outbound.agent_id.clone(),
                    outbound.assistant_message_id.clone(),
                )
            })
            .expect("fixture has an outbound delivery");
        snapshot
            .agents
            .iter_mut()
            .find(|agent| agent.state.id == agent_id)
            .and_then(|agent| {
                agent
                    .messages
                    .iter_mut()
                    .find(|message| message.id == assistant_message_id)
            })
            .expect("fixture has the referenced assistant message")
    }

    fn test_connector(id: &str, agent_id: &str) -> TelegramConnectorRecord {
        TelegramConnectorRecord {
            id: id.into(),
            agent_id: agent_id.into(),
            room_id: "room-1".into(),
            bot: TelegramBotIdentity {
                id: "bot-1".into(),
                username: Some("test_bot".into()),
                display_name: Some("Test Bot".into()),
            },
            approved_chat: Some(test_chat()),
            pending_pairing: Some(TelegramPendingPairing {
                chat: test_chat(),
                requested_at_ms: 9,
            }),
            next_update_id: 43,
            enabled: true,
            deleted_at_ms: None,
            created_at_ms: 10,
            updated_at_ms: 11,
        }
    }

    fn test_chat() -> TelegramChatMetadata {
        TelegramChatMetadata {
            id: "chat-1".into(),
            kind: TelegramChatKind::Private,
            title: None,
            username: Some("safe_chat".into()),
        }
    }

    fn test_inbound(agent_id: &str) -> TelegramInboundRecord {
        TelegramInboundRecord {
            connector_id: "telegram-a".into(),
            update_id: 42,
            agent_id: agent_id.into(),
            room_id: "room-1".into(),
            normalized_text: "hello daemon".into(),
            sender: TelegramSenderMetadata {
                id: "sender-1".into(),
                username: Some("safe_sender".into()),
                display_name: Some("Safe Sender".into()),
            },
            chat: test_chat(),
            received_at_ms: 12,
            processing_state: InboundProcessingState::Processing,
            run_idempotency_key: "telegram-a:update:42".into(),
        }
    }

    fn test_outbound(agent_id: &str) -> TelegramOutboundRecord {
        TelegramOutboundRecord {
            id: "outbound-1".into(),
            connector_id: "telegram-a".into(),
            agent_id: agent_id.into(),
            room_id: "room-1".into(),
            assistant_message_id: "assistant-message-1".into(),
            text: "hello user".into(),
            created_at_ms: 13,
            delivered_at_ms: None,
            attempts: 1,
            delivery_state: OutboundDeliveryState::Pending,
        }
    }

    fn test_schedule(agent_id: &str) -> ScheduledPromptRecord {
        ScheduledPromptRecord {
            id: "schedule-1".into(),
            import_idempotency_key: Some("import-1".into()),
            agent_id: agent_id.into(),
            prompt: "Review the workspace".into(),
            trigger: ScheduleTrigger::Daily {
                hour: 9,
                minute: 30,
                time_zone: "Asia/Kuala_Lumpur".into(),
            },
            enabled: true,
            target: ScheduleTarget::Connector {
                connector_id: "telegram-a".into(),
            },
            next_due_at_ms: 14,
            last_fired: Some(ScheduleLastFired {
                fired_at_ms: 13,
                run_idempotency_key: "schedule-1:13".into(),
            }),
            last_safe_outcome: Some(ScheduleSafeOutcome {
                status: ScheduleOutcomeStatus::Succeeded,
                occurred_at_ms: 13,
            }),
            created_at_ms: 10,
            updated_at_ms: 14,
        }
    }

    fn assert_u64(_: u64) {}

    fn forged_tool(name: &str, description: &str) -> ToolDescriptor {
        ToolDescriptor {
            name: name.into(),
            description: description.into(),
            parameters_schema: Default::default(),
            examples: None,
        }
    }

    fn test_config(name: &str) -> AgentConfig {
        AgentConfig {
            name: name.into(),
            model: "deterministic".into(),
            bio: None,
            lore: None,
            knowledge: None,
            topics: None,
            adjectives: None,
            style: None,
            provider: None,
            system: None,
            tools: None,
            plugins: None,
            settings: Some(AgentSettings::default()),
        }
    }
}

const MEMORY_QUERY_EXPANDER_ENV: &str = "ANIMAOS_RS_MEMORY_QUERY_EXPANDER";
const MEMORY_TEXT_ANALYZER_ENV: &str = "ANIMAOS_RS_MEMORY_TEXT_ANALYZER";

pub(crate) fn memory_manager_from_env() -> MemoryManager {
    let text_analyzer = memory_text_analyzer_from_env();
    match memory_query_expander_from_env() {
        Some(query_expander) => {
            MemoryManager::with_text_analyzer_and_query_expander(text_analyzer, query_expander)
        }
        None => MemoryManager::with_text_analyzer(text_analyzer),
    }
}

pub(crate) fn memory_text_analyzer_from_env() -> TextAnalyzer {
    let Ok(value) = std::env::var(MEMORY_TEXT_ANALYZER_ENV) else {
        return TextAnalyzer::default();
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "default" | "multilingual" | "unicode" => TextAnalyzer::multilingual(),
        unknown => {
            warn!(
                env = MEMORY_TEXT_ANALYZER_ENV,
                value = unknown,
                "unknown memory text analyzer profile; using multilingual search"
            );
            TextAnalyzer::default()
        }
    }
}

pub(crate) fn memory_query_expander_from_env() -> Option<QueryExpander> {
    let Ok(value) = std::env::var(MEMORY_QUERY_EXPANDER_ENV) else {
        return None;
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "none" | "off" | "disabled" => None,
        "locomo" | "locomo-benchmark" => Some(locomo_query_expander()),
        unknown => {
            warn!(
                env = MEMORY_QUERY_EXPANDER_ENV,
                value = unknown,
                "unknown memory query expander profile; using default BM25 search"
            );
            None
        }
    }
}

pub(crate) struct DaemonState {
    pub(crate) memory: SharedMemoryStore,
    pub(crate) memory_embeddings: SharedMemoryEmbeddings,
    pub(crate) memory_store: Option<MemoryStoreConfig>,
    pub(crate) control_plane_store: Option<ControlPlaneStoreConfig>,
    control_plane_revision: u64,
    control_plane_persist_order: Arc<ControlPlanePersistOrder>,
    pub(crate) agents: HashMap<String, AgentRuntime>,
    pub(crate) agent_snapshots: HashMap<String, AgentRuntimeSnapshot>,
    deleted_agent_ids: HashSet<String>,
    pub(crate) swarms: HashMap<String, SwarmCoordinator>,
    pub(crate) swarm_configs: HashMap<String, SwarmConfig>,
    pub(crate) swarm_events: HashMap<String, EventFanout>,
    pub(crate) swarm_snapshots: HashMap<String, SwarmState>,
    pub(crate) connectors: HashMap<String, TelegramConnectorRecord>,
    pub(crate) credential_cleanup: HashMap<String, TelegramCredentialCleanupIntent>,
    pub(crate) inbound: HashMap<(String, i64), TelegramInboundRecord>,
    pub(crate) outbound: HashMap<String, TelegramOutboundRecord>,
    pub(crate) schedules: HashMap<String, ScheduledPromptRecord>,
    pub(crate) model_adapter: Arc<dyn ModelAdapter>,
    pub(crate) tool_registry: ToolRegistry,
    pub(crate) process_manager: SharedProcessManager,
    pub(crate) event_fanout: EventFanout,
    pub(crate) db: Option<Arc<dyn DatabaseAdapter>>,
}

pub(crate) struct ControlPlanePersistRequest {
    config: Option<ControlPlaneStoreConfig>,
    snapshot: ControlPlaneSnapshot,
    revision: u64,
    order: Arc<ControlPlanePersistOrder>,
}

struct ControlPlanePersistOrder {
    latest_successful_revision: AsyncMutex<u64>,
    #[cfg(test)]
    test_save_gate: StdMutex<Option<TestControlPlaneSaveGate>>,
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct TestControlPlaneSaveGate {
    pub(crate) entered: Arc<Semaphore>,
    pub(crate) release: Arc<Semaphore>,
    fail: bool,
}

impl ControlPlanePersistRequest {
    pub(crate) async fn save(self) -> std::io::Result<()> {
        let mut latest_successful_revision = self.order.latest_successful_revision.lock().await;
        if self.revision <= *latest_successful_revision {
            return Ok(());
        }
        #[cfg(test)]
        let test_save_gate = {
            self.order
                .test_save_gate
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
        };
        #[cfg(test)]
        if let Some(gate) = test_save_gate {
            gate.entered.add_permits(1);
            gate.release
                .acquire()
                .await
                .map_err(|_| std::io::Error::other("test save gate closed"))?
                .forget();
            if gate.fail {
                return Err(std::io::Error::other("injected control-plane save failure"));
            }
        }
        save_control_plane_snapshot(self.config.as_ref(), &self.snapshot).await?;
        *latest_successful_revision = self.revision;
        Ok(())
    }
}

impl DaemonState {
    #[allow(dead_code)]
    pub(crate) fn new() -> Self {
        Self::with_model_adapter_and_events_and_limits(
            Arc::new(DeterministicModelAdapter),
            EventFanout::new(DEFAULT_EVENT_BUFFER),
            DEFAULT_MAX_BACKGROUND_PROCESSES,
        )
    }

    #[allow(dead_code)]
    pub(crate) fn with_events(event_fanout: EventFanout) -> Self {
        Self::with_events_and_limits(event_fanout, DEFAULT_MAX_BACKGROUND_PROCESSES)
    }

    pub(crate) fn with_events_and_limits(
        event_fanout: EventFanout,
        max_background_processes: usize,
    ) -> Self {
        Self::with_model_adapter_and_events_and_limits(
            Arc::new(DeterministicModelAdapter),
            event_fanout,
            max_background_processes,
        )
    }

    #[allow(dead_code)]
    pub(crate) fn with_model_adapter(model_adapter: Arc<dyn ModelAdapter>) -> Self {
        Self::with_model_adapter_and_events_and_limits(
            model_adapter,
            EventFanout::new(DEFAULT_EVENT_BUFFER),
            DEFAULT_MAX_BACKGROUND_PROCESSES,
        )
    }

    #[allow(dead_code)]
    pub(crate) fn with_model_adapter_and_events(
        model_adapter: Arc<dyn ModelAdapter>,
        event_fanout: EventFanout,
    ) -> Self {
        Self::with_model_adapter_and_events_and_limits(
            model_adapter,
            event_fanout,
            DEFAULT_MAX_BACKGROUND_PROCESSES,
        )
    }

    pub(crate) fn with_model_adapter_and_events_and_limits(
        model_adapter: Arc<dyn ModelAdapter>,
        event_fanout: EventFanout,
        max_background_processes: usize,
    ) -> Self {
        let memory = Arc::new(AsyncRwLock::new(memory_manager_from_env()));
        let memory_embeddings = Arc::new(AsyncRwLock::new(MemoryEmbeddingRuntime::local_default()));
        Self {
            memory,
            memory_embeddings,
            memory_store: None,
            control_plane_store: None,
            control_plane_revision: 0,
            control_plane_persist_order: Arc::new(ControlPlanePersistOrder {
                latest_successful_revision: AsyncMutex::new(0),
                #[cfg(test)]
                test_save_gate: StdMutex::new(None),
            }),
            agents: HashMap::new(),
            agent_snapshots: HashMap::new(),
            deleted_agent_ids: HashSet::new(),
            swarms: HashMap::new(),
            swarm_configs: HashMap::new(),
            swarm_events: HashMap::new(),
            swarm_snapshots: HashMap::new(),
            connectors: HashMap::new(),
            credential_cleanup: HashMap::new(),
            inbound: HashMap::new(),
            outbound: HashMap::new(),
            schedules: HashMap::new(),
            model_adapter,
            tool_registry: ToolRegistry::new(),
            process_manager: new_shared_process_manager_with_limit(max_background_processes),
            event_fanout,
            db: None,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn event_fanout(&self) -> EventFanout {
        self.event_fanout.clone()
    }

    pub(crate) fn memory_handle(&self) -> SharedMemoryStore {
        Arc::clone(&self.memory)
    }

    pub(crate) fn memory_embeddings_handle(&self) -> SharedMemoryEmbeddings {
        Arc::clone(&self.memory_embeddings)
    }

    pub(crate) fn memory_store_config(&self) -> Option<MemoryStoreConfig> {
        self.memory_store.clone()
    }

    pub(crate) fn replace_memory(&mut self, memory: MemoryManager) {
        self.memory = Arc::new(AsyncRwLock::new(memory));
    }

    pub(crate) fn set_memory_store(&mut self, memory_store: Option<MemoryStoreConfig>) {
        self.memory_store = memory_store;
    }

    pub(crate) fn set_control_plane_store(
        &mut self,
        control_plane_store: Option<ControlPlaneStoreConfig>,
    ) {
        self.control_plane_store = control_plane_store;
    }

    pub(crate) fn control_plane_persist_request(&mut self) -> ControlPlanePersistRequest {
        self.control_plane_revision = self
            .control_plane_revision
            .checked_add(1)
            .expect("control-plane persistence revision overflowed");
        ControlPlanePersistRequest {
            config: self.control_plane_store.clone(),
            snapshot: self.control_plane_snapshot(),
            revision: self.control_plane_revision,
            order: Arc::clone(&self.control_plane_persist_order),
        }
    }

    #[cfg(test)]
    pub(crate) fn install_test_control_plane_save_gate(
        &mut self,
        fail: bool,
    ) -> TestControlPlaneSaveGate {
        let gate = TestControlPlaneSaveGate {
            entered: Arc::new(Semaphore::new(0)),
            release: Arc::new(Semaphore::new(0)),
            fail,
        };
        *self
            .control_plane_persist_order
            .test_save_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(gate.clone());
        gate
    }

    pub(crate) fn control_plane_snapshot(&self) -> ControlPlaneSnapshot {
        let agents = self.list_agents();
        let mut swarms = self
            .swarm_configs
            .iter()
            .filter_map(|(swarm_id, config)| {
                self.get_swarm(swarm_id).map(|state| StoredSwarmSnapshot {
                    config: config.clone(),
                    state,
                })
            })
            .collect::<Vec<_>>();
        swarms.sort_by(|left, right| left.state.id.cmp(&right.state.id));
        let mut connectors = self.connectors.values().cloned().collect::<Vec<_>>();
        connectors.sort_by(|left, right| left.id.cmp(&right.id));
        let mut credential_cleanup = self
            .credential_cleanup
            .values()
            .cloned()
            .collect::<Vec<_>>();
        credential_cleanup.sort_by(|left, right| left.connector_id.cmp(&right.connector_id));
        let mut inbound = self.inbound.values().cloned().collect::<Vec<_>>();
        inbound.sort_by(|left, right| {
            left.connector_id
                .cmp(&right.connector_id)
                .then_with(|| left.update_id.cmp(&right.update_id))
        });
        let mut outbound = self.outbound.values().cloned().collect::<Vec<_>>();
        outbound.sort_by(|left, right| left.id.cmp(&right.id));
        let mut schedules = self.schedules.values().cloned().collect::<Vec<_>>();
        schedules.sort_by(|left, right| left.id.cmp(&right.id));

        ControlPlaneSnapshot::with_connector_state_and_cleanup(
            agents,
            swarms,
            connectors,
            credential_cleanup,
            inbound,
            outbound,
            schedules,
        )
    }

    pub(crate) fn restore_control_plane_snapshot(
        &mut self,
        snapshot: ControlPlaneSnapshot,
    ) -> Result<(usize, usize), String> {
        self.validate_control_plane_snapshot(&snapshot)?;
        let mut restored_agents = 0;
        let mut restored_swarms = 0;

        for agent_snapshot in snapshot.agents {
            self.restore_agent_snapshot(agent_snapshot)?;
            restored_agents += 1;
        }

        for stored_swarm in snapshot.swarms {
            let (coordinator, event_stream) =
                self.build_recovered_swarm(stored_swarm.config, stored_swarm.state)?;
            self.register_recovered_swarm(coordinator, event_stream);
            restored_swarms += 1;
        }

        self.connectors = snapshot
            .connectors
            .into_iter()
            .map(|connector| (connector.id.clone(), connector))
            .collect();
        self.credential_cleanup = snapshot
            .credential_cleanup
            .into_iter()
            .map(|intent| (intent.connector_id.clone(), intent))
            .collect();
        self.inbound = snapshot
            .inbound
            .into_iter()
            .map(|record| ((record.connector_id.clone(), record.update_id), record))
            .collect();
        self.outbound = snapshot
            .outbound
            .into_iter()
            .map(|record| (record.id.clone(), record))
            .collect();
        self.schedules = snapshot
            .schedules
            .into_iter()
            .map(|schedule| (schedule.id.clone(), schedule))
            .collect();

        Ok((restored_agents, restored_swarms))
    }

    fn validate_control_plane_snapshot(
        &self,
        snapshot: &ControlPlaneSnapshot,
    ) -> Result<(), String> {
        let mut persisted_agents = self
            .list_agents()
            .into_iter()
            .map(|agent| (agent.state.id.clone(), agent))
            .collect::<HashMap<_, _>>();
        for agent in &snapshot.agents {
            persisted_agents.insert(agent.state.id.clone(), agent.clone());
        }
        let mut agent_ids = self
            .agent_snapshots
            .keys()
            .chain(self.agents.keys())
            .cloned()
            .collect::<HashSet<_>>();
        let mut snapshot_agent_ids = HashSet::new();
        for agent in &snapshot.agents {
            let agent_id = &agent.state.id;
            if agent_id.is_empty() || !snapshot_agent_ids.insert(agent_id.clone()) {
                return Err(format!(
                    "duplicate or empty agent id in snapshot: {agent_id}"
                ));
            }
            self.resolve_restored_agent_tools(agent.state.config.tools.clone())?;
            agent_ids.insert(agent_id.clone());
        }

        let mut swarm_ids = HashSet::new();
        for swarm in &snapshot.swarms {
            let swarm_id = &swarm.state.id;
            if swarm_id.is_empty() || !swarm_ids.insert(swarm_id.clone()) {
                return Err(format!(
                    "duplicate or empty swarm id in snapshot: {swarm_id}"
                ));
            }
            self.validate_swarm_tools(&swarm.config)?;
        }

        let mut connectors = HashMap::new();
        let mut connector_ids = HashSet::new();
        for connector in &snapshot.connectors {
            if connector.id.is_empty() || !connector_ids.insert(connector.id.clone()) {
                return Err(format!(
                    "duplicate or empty connector id in snapshot: {}",
                    connector.id
                ));
            }
            let tombstoned = connector.deleted_at_ms.is_some() && !connector.enabled;
            if connector.deleted_at_ms.is_some_and(|deleted_at_ms| {
                deleted_at_ms < connector.created_at_ms || deleted_at_ms < connector.updated_at_ms
            }) {
                return Err(format!(
                    "connector '{}' has invalid deletion timing",
                    connector.id
                ));
            }
            if connector.deleted_at_ms.is_some() && connector.enabled {
                return Err(format!(
                    "connector '{}' cannot be enabled after deletion",
                    connector.id
                ));
            }
            if tombstoned && connector.pending_pairing.is_some() {
                return Err(format!(
                    "connector '{}' cannot retain pending pairing after deletion",
                    connector.id
                ));
            }
            if !agent_ids.contains(&connector.agent_id) && !tombstoned {
                return Err(format!(
                    "connector '{}' references missing agent '{}'",
                    connector.id, connector.agent_id
                ));
            }
            connectors.insert(connector.id.clone(), connector.clone());
        }

        let mut cleanup_ids = HashSet::new();
        for intent in &snapshot.credential_cleanup {
            if intent.connector_id.trim().is_empty()
                || intent.created_at_ms == 0
                || !cleanup_ids.insert(intent.connector_id.clone())
                || connector_ids.contains(&intent.connector_id)
            {
                return Err(format!(
                    "invalid or conflicting credential cleanup intent: {}",
                    intent.connector_id
                ));
            }
        }

        let mut inbound_keys = HashSet::new();
        let mut inbound_idempotency_keys = HashSet::new();
        for record in &snapshot.inbound {
            let key = (record.connector_id.clone(), record.update_id);
            if record.connector_id.is_empty() || !inbound_keys.insert(key) {
                return Err(format!(
                    "duplicate or empty inbound connector/update key: {}:{}",
                    record.connector_id, record.update_id
                ));
            }
            let connector = connectors.get(&record.connector_id).ok_or_else(|| {
                format!(
                    "inbound update references missing connector '{}'",
                    record.connector_id
                )
            })?;
            if connector.deleted_at_ms.is_some()
                && !matches!(
                    record.processing_state,
                    InboundProcessingState::Processed | InboundProcessingState::Rejected
                )
            {
                return Err(format!(
                    "inbound update {}:{} cannot remain unprocessed after connector deletion",
                    record.connector_id, record.update_id
                ));
            }
            let archived = !connector.is_active()
                && matches!(
                    record.processing_state,
                    InboundProcessingState::Processed | InboundProcessingState::Rejected
                );
            if !agent_ids.contains(&record.agent_id) && !archived {
                return Err(format!(
                    "inbound update references missing agent '{}'",
                    record.agent_id
                ));
            }
            if connector.agent_id != record.agent_id || connector.room_id != record.room_id {
                return Err(format!(
                    "inbound update {}:{} conflicts with connector room or agent ownership",
                    record.connector_id, record.update_id
                ));
            }
            let idempotency_key = record.run_idempotency_key.trim();
            if idempotency_key.is_empty()
                || !inbound_idempotency_keys.insert(idempotency_key.to_string())
            {
                return Err(format!(
                    "inbound update {}:{} has a duplicate or empty run idempotency key",
                    record.connector_id, record.update_id
                ));
            }
        }

        let mut outbound_ids = HashSet::new();
        let mut outbound_delivery_identities = HashSet::new();
        for record in &snapshot.outbound {
            if record.id.is_empty() || !outbound_ids.insert(record.id.clone()) {
                return Err(format!(
                    "duplicate or empty outbound id in snapshot: {}",
                    record.id
                ));
            }
            let connector = connectors.get(&record.connector_id).ok_or_else(|| {
                format!(
                    "outbound delivery references missing connector '{}'",
                    record.connector_id
                )
            })?;
            if connector.deleted_at_ms.is_some()
                && record.delivery_state != OutboundDeliveryState::Delivered
            {
                return Err(format!(
                    "outbound delivery '{}' cannot remain undelivered after connector deletion",
                    record.id
                ));
            }
            let archived =
                !connector.is_active() && record.delivery_state == OutboundDeliveryState::Delivered;
            if !agent_ids.contains(&record.agent_id) && !archived {
                return Err(format!(
                    "outbound delivery references missing agent '{}'",
                    record.agent_id
                ));
            }
            if connector.agent_id != record.agent_id || connector.room_id != record.room_id {
                return Err(format!(
                    "outbound delivery '{}' conflicts with connector room or agent ownership",
                    record.id
                ));
            }
            if record.assistant_message_id.trim().is_empty()
                || !outbound_delivery_identities.insert((
                    record.connector_id.clone(),
                    record.assistant_message_id.clone(),
                ))
            {
                return Err(format!(
                    "outbound delivery '{}' has a duplicate or empty connector/assistant-message identity",
                    record.id
                ));
            }
            if let Some(agent) = persisted_agents.get(&record.agent_id) {
                let assistant_message_exists = agent.messages.iter().any(|message| {
                    message.id == record.assistant_message_id
                        && message.agent_id == record.agent_id
                        && message.room_id == connector.room_id
                        && message.role == MessageRole::Assistant
                });
                if !assistant_message_exists {
                    return Err(format!(
                        "outbound delivery '{}' references a missing, non-assistant, or wrong-room assistant message '{}'",
                        record.id, record.assistant_message_id
                    ));
                }
            } else if !archived {
                return Err(format!(
                    "outbound delivery '{}' has no persisted snapshot for agent '{}'",
                    record.id, record.agent_id
                ));
            }
        }

        let mut schedule_ids = HashSet::new();
        let mut import_idempotency_keys = HashSet::new();
        for schedule in &snapshot.schedules {
            if schedule.id.is_empty() || !schedule_ids.insert(schedule.id.clone()) {
                return Err(format!(
                    "duplicate or empty schedule id in snapshot: {}",
                    schedule.id
                ));
            }
            let archived_connector = match &schedule.target {
                ScheduleTarget::Connector { connector_id } => connectors
                    .get(connector_id)
                    .is_some_and(|connector| !connector.is_active()),
                ScheduleTarget::Workspace => false,
            };
            if !agent_ids.contains(&schedule.agent_id) && (schedule.enabled || !archived_connector)
            {
                return Err(format!(
                    "schedule '{}' references missing agent '{}'",
                    schedule.id, schedule.agent_id
                ));
            }
            if schedule.prompt.trim().is_empty() {
                return Err(format!("schedule '{}' has a blank prompt", schedule.id));
            }
            if schedule.next_due_at_ms == 0
                || schedule.created_at_ms == 0
                || schedule.updated_at_ms == 0
                || schedule.updated_at_ms < schedule.created_at_ms
            {
                return Err(format!(
                    "schedule '{}' has invalid next-due or created/updated timing",
                    schedule.id
                ));
            }
            if let Some(import_idempotency_key) = &schedule.import_idempotency_key {
                let import_idempotency_key = import_idempotency_key.trim();
                if import_idempotency_key.is_empty()
                    || !import_idempotency_keys.insert(import_idempotency_key.to_string())
                {
                    return Err(format!(
                        "schedule '{}' has a duplicate or empty import idempotency key",
                        schedule.id
                    ));
                }
            }
            match &schedule.trigger {
                crate::schedules::ScheduleTrigger::Interval { interval_ms }
                    if *interval_ms == 0 =>
                {
                    return Err(format!("schedule '{}' has a zero interval", schedule.id));
                }
                crate::schedules::ScheduleTrigger::Daily {
                    hour,
                    minute,
                    time_zone,
                } if *hour > 23 || *minute > 59 || time_zone.trim().is_empty() => {
                    return Err(format!(
                        "schedule '{}' has an invalid daily trigger",
                        schedule.id
                    ));
                }
                _ => {}
            }
            if let Some(last_fired) = &schedule.last_fired {
                if last_fired.fired_at_ms == 0
                    || last_fired.fired_at_ms > schedule.updated_at_ms
                    || last_fired.run_idempotency_key.trim().is_empty()
                {
                    return Err(format!(
                        "schedule '{}' has invalid last-fired state",
                        schedule.id
                    ));
                }
            }
            if let Some(last_safe_outcome) = &schedule.last_safe_outcome {
                if last_safe_outcome.occurred_at_ms == 0
                    || last_safe_outcome.occurred_at_ms > schedule.updated_at_ms
                {
                    return Err(format!(
                        "schedule '{}' has invalid last safe outcome timing",
                        schedule.id
                    ));
                }
            }
            if let ScheduleTarget::Connector { connector_id } = &schedule.target {
                let connector = connectors.get(connector_id).ok_or_else(|| {
                    format!(
                        "schedule '{}' references missing connector '{connector_id}'",
                        schedule.id
                    )
                })?;
                if connector.agent_id != schedule.agent_id {
                    return Err(format!(
                        "schedule '{}' conflicts with connector agent ownership",
                        schedule.id
                    ));
                }
                if schedule.enabled && !connector.is_active() {
                    return Err(format!(
                        "schedule '{}' is enabled for archived connector '{connector_id}'",
                        schedule.id
                    ));
                }
            }
        }

        Ok(())
    }

    pub(crate) fn replace_memory_embeddings(&mut self, embeddings: MemoryEmbeddingRuntime) {
        self.memory_embeddings = Arc::new(AsyncRwLock::new(embeddings));
    }

    pub(crate) fn agent_count(&self) -> usize {
        let mut count = self.agent_snapshots.len();
        for agent_id in self.agents.keys() {
            if !self.agent_snapshots.contains_key(agent_id) {
                count += 1;
            }
        }
        count
    }

    pub(crate) fn swarm_count(&self) -> usize {
        self.swarms.len()
    }

    pub(crate) fn swarm_snapshot_count(&self) -> usize {
        self.swarm_snapshots.len()
    }

    pub(crate) fn database_configured(&self) -> bool {
        self.db.is_some()
    }

    pub(crate) fn control_plane_durability(&self) -> String {
        self.control_plane_store
            .as_ref()
            .map(|config| config.storage_label().to_string())
            .unwrap_or_else(|| "ephemeral".to_string())
    }

    pub(crate) fn background_process_count(&self) -> Result<usize, String> {
        background_process_count(&self.process_manager)
    }

    pub(crate) fn set_database(&mut self, db: Arc<dyn DatabaseAdapter>) {
        for runtime in self.agents.values_mut() {
            runtime.set_database(Arc::clone(&db));
        }
        self.db = Some(db);
    }

    pub(crate) fn build_swarm(
        &self,
        config: SwarmConfig,
    ) -> Result<(SwarmCoordinator, EventFanout), String> {
        self.validate_swarm_tools(&config)?;

        let event_stream = EventFanout::new(self.event_fanout.capacity());
        let swarm_message_events = event_stream.clone();
        let global_message_events = self.event_fanout();
        let memory = Arc::clone(&self.memory);
        let memory_embeddings = Arc::clone(&self.memory_embeddings);
        let memory_store = self.memory_store.clone();
        let agent_names = Arc::new(swarm_agent_names(&config));
        let message_events: Arc<CoordinatorMessageEventFn> = Arc::new(move |swarm_id, message| {
            let global_message_events = global_message_events.clone();
            let swarm_message_events = swarm_message_events.clone();
            let memory = Arc::clone(&memory);
            let memory_embeddings = Arc::clone(&memory_embeddings);
            let memory_store = memory_store.clone();
            let agent_names = Arc::clone(&agent_names);
            Box::pin(async move {
                runtime_events::publish_swarm_message_event(
                    &global_message_events,
                    &swarm_message_events,
                    &swarm_id,
                    &message,
                );
                persist_swarm_message_relationship(
                    memory,
                    memory_embeddings,
                    memory_store,
                    agent_names,
                    swarm_id,
                    message,
                )
                .await;
            })
        });
        let strategy = resolve_strategy(config.strategy);
        let factory = self.swarm_agent_factory(event_stream.clone());

        Ok((
            SwarmCoordinator::with_hooks_and_message_events(
                config,
                strategy,
                factory,
                Some(message_events),
            ),
            event_stream,
        ))
    }

    pub(crate) fn build_recovered_swarm(
        &self,
        config: SwarmConfig,
        mut snapshot: SwarmState,
    ) -> Result<(SwarmCoordinator, EventFanout), String> {
        self.validate_swarm_tools(&config)?;
        snapshot.agent_ids.clear();
        if snapshot.status == anima_swarm::SwarmStatus::Running {
            snapshot.status = anima_swarm::SwarmStatus::Failed;
            snapshot
                .completed_at
                .get_or_insert_with(anima_core::primitives::now_millis);
        }

        let event_stream = EventFanout::new(self.event_fanout.capacity());
        let swarm_message_events = event_stream.clone();
        let global_message_events = self.event_fanout();
        let memory = Arc::clone(&self.memory);
        let memory_embeddings = Arc::clone(&self.memory_embeddings);
        let memory_store = self.memory_store.clone();
        let agent_names = Arc::new(swarm_agent_names(&config));
        let message_events: Arc<CoordinatorMessageEventFn> = Arc::new(move |swarm_id, message| {
            let global_message_events = global_message_events.clone();
            let swarm_message_events = swarm_message_events.clone();
            let memory = Arc::clone(&memory);
            let memory_embeddings = Arc::clone(&memory_embeddings);
            let memory_store = memory_store.clone();
            let agent_names = Arc::clone(&agent_names);
            Box::pin(async move {
                runtime_events::publish_swarm_message_event(
                    &global_message_events,
                    &swarm_message_events,
                    &swarm_id,
                    &message,
                );
                persist_swarm_message_relationship(
                    memory,
                    memory_embeddings,
                    memory_store,
                    agent_names,
                    swarm_id,
                    message,
                )
                .await;
            })
        });
        let strategy = resolve_strategy(config.strategy);
        let factory = self.swarm_agent_factory(event_stream.clone());

        Ok((
            SwarmCoordinator::with_recovered_state_and_hooks(
                config,
                snapshot,
                strategy,
                factory,
                Some(message_events),
            ),
            event_stream,
        ))
    }

    pub(crate) fn register_swarm(
        &mut self,
        coordinator: SwarmCoordinator,
        event_stream: EventFanout,
    ) -> SwarmState {
        let snapshot = coordinator.get_state();
        let swarm_id = snapshot.id.clone();
        self.swarm_configs
            .insert(swarm_id.clone(), coordinator.config());
        self.swarms.insert(swarm_id.clone(), coordinator);
        self.swarm_events.insert(swarm_id.clone(), event_stream);
        self.swarm_snapshots.insert(swarm_id, snapshot.clone());
        snapshot
    }

    pub(crate) fn register_recovered_swarm(
        &mut self,
        coordinator: SwarmCoordinator,
        event_stream: EventFanout,
    ) -> SwarmState {
        self.register_swarm(coordinator, event_stream)
    }

    pub(crate) fn get_swarm(&self, swarm_id: &str) -> Option<SwarmState> {
        self.swarms
            .get(swarm_id)
            .map(SwarmCoordinator::get_state)
            .or_else(|| self.swarm_snapshots.get(swarm_id).cloned())
    }

    pub(crate) fn list_swarms(&self) -> Vec<SwarmState> {
        let mut snapshots = self.swarm_snapshots.clone();
        for (swarm_id, coordinator) in &self.swarms {
            snapshots.insert(swarm_id.clone(), coordinator.get_state());
        }

        let mut snapshots: Vec<_> = snapshots.into_values().collect();
        snapshots.sort_by(|left, right| left.id.cmp(&right.id));
        snapshots
    }

    pub(crate) fn get_swarm_coordinator(&self, swarm_id: &str) -> Option<SwarmCoordinator> {
        self.swarms.get(swarm_id).cloned()
    }

    pub(crate) fn subscribe_to_swarm_events(&self, swarm_id: &str) -> Option<EventSubscriber> {
        self.swarm_events.get(swarm_id).map(EventFanout::subscribe)
    }

    pub(crate) fn swarm_event_fanout(&self, swarm_id: &str) -> Option<EventFanout> {
        self.swarm_events.get(swarm_id).cloned()
    }

    pub(crate) fn store_swarm_snapshot(&mut self, snapshot: SwarmState) {
        self.swarm_snapshots.insert(snapshot.id.clone(), snapshot);
    }

    pub(crate) fn create_agent(
        &mut self,
        mut config: AgentConfig,
    ) -> Result<AgentRuntimeSnapshot, String> {
        config.tools = self.resolve_agent_tools(config.tools)?;
        let mut runtime = AgentRuntime::new(config, Arc::clone(&self.model_adapter));
        runtime.set_providers(default_providers(Arc::clone(&self.memory)));
        runtime.set_evaluators(default_evaluators(
            Arc::clone(&self.memory),
            Arc::clone(&self.memory_embeddings),
            self.memory_store.clone(),
        ));
        if let Some(db) = &self.db {
            runtime.set_database(Arc::clone(db));
        }
        runtime.init();
        let agent_id = runtime.id().to_string();
        let snapshot = runtime.snapshot();
        self.agent_snapshots
            .insert(agent_id.clone(), snapshot.clone());
        self.agents.insert(agent_id, runtime);
        Ok(snapshot)
    }

    /// Apply a fully validated partial config update to an existing agent and
    /// refresh its snapshot.
    pub(crate) fn update_agent(
        &mut self,
        agent_id: &str,
        mut patch: AgentConfigUpdate,
    ) -> Result<AgentRuntimeSnapshot, UpdateAgentError> {
        if !self.agents.contains_key(agent_id) && !self.agent_snapshots.contains_key(agent_id) {
            return Err(UpdateAgentError::NotFound);
        }
        patch.tools = self
            .resolve_agent_tools(patch.tools)
            .map_err(UpdateAgentError::InvalidTools)?;

        if let Some(runtime) = self.agents.get_mut(agent_id) {
            runtime.update_config(patch);
            let snapshot = runtime.snapshot();
            self.agent_snapshots
                .insert(agent_id.to_string(), snapshot.clone());
            return Ok(snapshot);
        }

        // The runtime can be checked out for an in-flight run; fall back to
        // patching the snapshot so the change still persists. The run itself
        // keeps the config it started with.
        let snapshot = self
            .agent_snapshots
            .get_mut(agent_id)
            .expect("agent existence was checked before validation");
        if let Some(name) = patch.name {
            snapshot.state.name = name.clone();
            snapshot.state.config.name = name;
        }
        if let Some(model) = patch.model {
            snapshot.state.config.model = model;
        }
        if let Some(provider) = patch.provider {
            snapshot.state.config.provider = if provider.is_empty() {
                None
            } else {
                Some(provider)
            };
        }
        if let Some(system) = patch.system {
            snapshot.state.config.system = if system.is_empty() {
                None
            } else {
                Some(system)
            };
        }
        if let Some(tools) = patch.tools {
            snapshot.state.config.tools = Some(tools);
        }
        Ok(snapshot.clone())
    }

    fn restore_agent_snapshot(&mut self, mut snapshot: AgentRuntimeSnapshot) -> Result<(), String> {
        snapshot.state.config.tools =
            self.resolve_restored_agent_tools(snapshot.state.config.tools)?;
        let agent_id = snapshot.state.id.clone();
        let mut runtime = AgentRuntime::from_snapshot(snapshot, Arc::clone(&self.model_adapter));
        runtime.set_providers(default_providers(Arc::clone(&self.memory)));
        runtime.set_evaluators(default_evaluators(
            Arc::clone(&self.memory),
            Arc::clone(&self.memory_embeddings),
            self.memory_store.clone(),
        ));
        if let Some(db) = &self.db {
            runtime.set_database(Arc::clone(db));
        }
        if runtime.state().status == AgentStatus::Running {
            runtime.mark_failed("daemon restarted before task completed", 0);
        }

        let restored_snapshot = runtime.snapshot();
        self.agent_snapshots
            .insert(agent_id.clone(), restored_snapshot);
        self.agents.insert(agent_id, runtime);
        Ok(())
    }

    pub(crate) fn list_agents(&self) -> Vec<AgentRuntimeSnapshot> {
        let mut snapshots = self.agent_snapshots.clone();
        for (agent_id, runtime) in &self.agents {
            snapshots.insert(agent_id.clone(), runtime.snapshot());
        }

        let mut snapshots: Vec<_> = snapshots.into_values().collect();
        snapshots.sort_by(|left, right| {
            left.state
                .created_at_ms
                .cmp(&right.state.created_at_ms)
                .then_with(|| left.state.id.cmp(&right.state.id))
        });
        snapshots
    }

    pub(crate) fn get_agent(&self, agent_id: &str) -> Option<AgentRuntimeSnapshot> {
        self.agents
            .get(agent_id)
            .map(AgentRuntime::snapshot)
            .or_else(|| self.agent_snapshots.get(agent_id).cloned())
    }

    pub(crate) fn remove_agent(&mut self, agent_id: &str) {
        let had_snapshot = self.agent_snapshots.remove(agent_id).is_some();
        if let Some(mut runtime) = self.agents.remove(agent_id) {
            runtime.stop();
        } else if had_snapshot {
            self.deleted_agent_ids.insert(agent_id.to_string());
        }
    }

    pub(crate) fn restore_removed_agent(
        &mut self,
        snapshot: AgentRuntimeSnapshot,
    ) -> Result<(), String> {
        self.deleted_agent_ids.remove(&snapshot.state.id);
        self.restore_agent_snapshot(snapshot)
    }

    pub(crate) fn agent_runtime_id(&self, agent_id: &str) -> Option<String> {
        self.agents
            .get(agent_id)
            .map(|runtime| runtime.id().to_string())
            .or_else(|| {
                self.agent_snapshots
                    .get(agent_id)
                    .map(|snapshot| snapshot.state.id.clone())
            })
    }

    pub(crate) fn take_agent_runtime(
        &mut self,
        agent_id: &str,
    ) -> Option<(AgentRuntime, ToolExecutionContext)> {
        let runtime = self.agents.remove(agent_id)?;
        let mut snapshot = runtime.snapshot();
        snapshot.state.status = AgentStatus::Running;
        self.agent_snapshots.insert(agent_id.to_string(), snapshot);
        let tool_context = ToolExecutionContext::new(
            Arc::clone(&self.memory),
            Arc::clone(&self.memory_embeddings),
            self.memory_store.clone(),
            self.tool_registry.clone(),
            Arc::clone(&self.process_manager),
        );
        Some((runtime, tool_context))
    }

    pub(crate) fn restore_agent_runtime(
        &mut self,
        mut runtime: AgentRuntime,
    ) -> (
        AgentRuntimeSnapshot,
        String,
        String,
        SharedMemoryStore,
        SharedMemoryEmbeddings,
        Option<MemoryStoreConfig>,
    ) {
        let agent_id = runtime.id().to_string();
        let was_deleted = self.deleted_agent_ids.remove(&agent_id);
        if let Some(latest_config) = self
            .agent_snapshots
            .get(runtime.id())
            .map(|snapshot| snapshot.state.config.clone())
        {
            runtime.update_config(AgentConfigUpdate {
                name: Some(latest_config.name),
                model: Some(latest_config.model),
                provider: Some(latest_config.provider.unwrap_or_default()),
                system: Some(latest_config.system.unwrap_or_default()),
                tools: latest_config.tools,
            });
        }
        let snapshot = runtime.snapshot();
        let agent_name = runtime.state().name;
        if !was_deleted {
            self.agent_snapshots
                .insert(agent_id.clone(), snapshot.clone());
            self.agents.insert(agent_id.clone(), runtime);
        }

        (
            snapshot,
            agent_id,
            agent_name,
            Arc::clone(&self.memory),
            Arc::clone(&self.memory_embeddings),
            self.memory_store.clone(),
        )
    }

    /// Restores a pre-run transcript after an undurable final snapshot while
    /// retaining configuration changes accepted during the in-flight run.
    /// A concurrently deleted agent is never resurrected.
    pub(crate) fn rollback_agent_runtime(
        &mut self,
        mut previous: AgentRuntimeSnapshot,
    ) -> Result<bool, String> {
        let agent_id = previous.state.id.clone();
        let Some(latest) = self.get_agent(&agent_id) else {
            return Ok(false);
        };
        previous.state.name = latest.state.name;
        previous.state.config = latest.state.config;
        self.restore_agent_snapshot(previous)?;
        Ok(true)
    }

    fn validate_swarm_tools(&self, config: &SwarmConfig) -> Result<(), String> {
        self.tool_registry
            .validate_tools(config.manager.tools.as_deref())?;

        for worker in &config.workers {
            self.tool_registry.validate_tools(worker.tools.as_deref())?;
        }

        Ok(())
    }

    fn resolve_agent_tools(
        &self,
        tools: Option<Vec<ToolDescriptor>>,
    ) -> Result<Option<Vec<ToolDescriptor>>, String> {
        let Some(tools) = tools else {
            return Ok(None);
        };
        let mut names = Vec::with_capacity(tools.len());
        let mut seen = HashSet::with_capacity(tools.len());
        for tool in tools {
            if !seen.insert(tool.name.clone()) {
                return Err(format!("duplicate tool '{}'", tool.name));
            }
            if self.tool_registry.descriptor(&tool.name).is_none() {
                return Err(format!("unknown tool: {}", tool.name));
            }
            names.push(tool.name);
        }

        self.tool_registry.resolve_descriptors(names).map(Some)
    }

    fn resolve_restored_agent_tools(
        &self,
        tools: Option<Vec<ToolDescriptor>>,
    ) -> Result<Option<Vec<ToolDescriptor>>, String> {
        let Some(tools) = tools else {
            return Ok(None);
        };
        let mut names = Vec::with_capacity(tools.len());
        let mut seen = HashSet::with_capacity(tools.len());
        for tool in tools {
            if seen.insert(tool.name.clone()) {
                names.push(tool.name);
            }
        }

        self.tool_registry.resolve_descriptors(names).map(Some)
    }
}
