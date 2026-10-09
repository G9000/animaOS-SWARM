//! Automations in the daemon state (spec §9): announcing changes, recording
//! an occurrence's outcome with its counters and fire record (and undoing
//! that when the commit's save fails), and handing fires to the outbox.

use super::DaemonState;
use crate::live::{LiveEvent, LiveEventBody};
use crate::schedules::history::FireUndo;
use crate::schedules::{ScheduleFireRecord, ScheduleSafeOutcome, ScheduledPromptRecord};

/// What `record_automation_outcome` changed, so a rollback puts it back.
#[derive(Clone, Debug)]
pub(crate) struct OutcomeUndo {
    previous: ScheduledPromptRecord,
    fire: Option<FireUndo>,
}

impl DaemonState {
    /// Publishes `automation.updated` on the automation's agent's stream.
    /// Call it only after the change was saved.
    pub(crate) fn publish_automation_updated(
        &self,
        agent_id: &str,
        schedule_id: &str,
        deleted: bool,
    ) {
        self.live.publish(
            LiveEvent::new(
                agent_id,
                LiveEventBody::AutomationUpdated {
                    schedule_id: schedule_id.to_string(),
                    deleted,
                },
            ),
            None,
        );
    }

    /// Fire records for the outbox, oldest first. Those of deleted agents
    /// are dropped instead: their history went with the agent.
    pub(crate) fn unmirrored_schedule_fires(&mut self, limit: usize) -> Vec<ScheduleFireRecord> {
        let live = self.live_agent_ids();
        self.schedule_fires.retain_agents(&live);
        self.schedule_fires.unmirrored(limit)
    }

    /// Records the outcome of `schedule_id`'s current occurrence: the outcome,
    /// the counters, and a fire record keyed by the occurrence's run
    /// idempotency key. `run` is the occurrence's `(run id, session id)` when
    /// a run started. `None` when the automation is gone.
    pub(crate) fn record_automation_outcome(
        &mut self,
        schedule_id: &str,
        outcome: ScheduleSafeOutcome,
        run: Option<(String, String)>,
        finished_at_ms: u64,
    ) -> Option<OutcomeUndo> {
        let schedule = self.schedules.get_mut(schedule_id)?;
        let previous = schedule.clone();
        schedule.counters.record(&outcome.status);
        schedule.updated_at_ms = schedule
            .updated_at_ms
            .max(outcome.occurred_at_ms)
            .max(schedule.created_at_ms);
        let fire = schedule
            .last_fired
            .as_ref()
            .map(|fired| ScheduleFireRecord {
                id: fired.run_idempotency_key.clone(),
                schedule_id: schedule.id.clone(),
                agent_id: schedule.agent_id.clone(),
                fired_at_ms: fired.fired_at_ms,
                finished_at_ms: finished_at_ms.max(fired.fired_at_ms),
                outcome: outcome.status.clone(),
                run_id: run.as_ref().map(|(run_id, _)| run_id.clone()),
                session_id: run.map(|(_, session_id)| session_id),
                error_code: outcome.error_code.clone(),
                manual: fired.manual,
            });
        schedule.last_safe_outcome = Some(outcome);
        let fire = fire.map(|fire| self.schedule_fires.record_undoable(fire));
        Some(OutcomeUndo { previous, fire })
    }

    /// Puts back what `record_automation_outcome` changed: the outcome, the
    /// counters, and the fire log exactly as it was (controller ruling 3).
    /// Other fields keep any change made since (an owner's edit is not
    /// undone).
    pub(crate) fn undo_automation_outcome(&mut self, undo: OutcomeUndo) {
        if let Some(fire) = undo.fire {
            self.schedule_fires.undo(fire);
        }
        if let Some(schedule) = self.schedules.get_mut(&undo.previous.id) {
            schedule.last_safe_outcome = undo.previous.last_safe_outcome;
            schedule.counters = undo.previous.counters;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::agent_runs::test_support::{companion_config, next_event};
    use crate::schedules::{
        test_automation, ActiveHours, AutomationCounters, AutomationCreator, AutomationPreset,
        ScheduleFireRecord, ScheduleLastFired, ScheduleOutcomeStatus, ScheduleSafeOutcome,
        ScheduleTrigger,
    };
    use crate::state::DaemonState;

    fn claimed(agent_id: &str) -> crate::schedules::ScheduledPromptRecord {
        let mut record = test_automation(agent_id, "s1");
        record.last_fired = Some(ScheduleLastFired {
            fired_at_ms: 10_000,
            run_idempotency_key: "schedule:s1:10000".into(),
            manual: true,
        });
        record.updated_at_ms = 10_000;
        record
    }

    fn outcome(status: ScheduleOutcomeStatus) -> ScheduleSafeOutcome {
        ScheduleSafeOutcome {
            error_code: crate::schedules::checkin_error_code(&status),
            status,
            occurred_at_ms: 10_000,
        }
    }

    #[test]
    fn an_outcome_sets_the_counters_and_a_fire_and_its_undo_puts_them_back() {
        let mut state = DaemonState::new();
        let agent = state
            .create_agent(companion_config("companion"))
            .unwrap()
            .state
            .id;
        let before = claimed(&agent);
        state.schedules.insert("s1".into(), before.clone());

        let undo = state
            .record_automation_outcome(
                "s1",
                outcome(ScheduleOutcomeStatus::Failed),
                Some(("run_1".into(), "schedule:s1".into())),
                12_000,
            )
            .expect("the automation exists");

        let after = &state.schedules["s1"];
        assert_eq!(after.counters.failures, 1);
        assert_eq!(after.counters.consecutive_failures, 1);
        assert_eq!(
            after
                .last_safe_outcome
                .as_ref()
                .map(|outcome| outcome.status.clone()),
            Some(ScheduleOutcomeStatus::Failed)
        );
        assert_eq!(
            state.schedule_fires.snapshot(),
            vec![ScheduleFireRecord {
                id: "schedule:s1:10000".into(),
                schedule_id: "s1".into(),
                agent_id: agent.clone(),
                fired_at_ms: 10_000,
                finished_at_ms: 12_000,
                outcome: ScheduleOutcomeStatus::Failed,
                run_id: Some("run_1".into()),
                session_id: Some("schedule:s1".into()),
                error_code: Some("schedule_run_failed".into()),
                manual: true,
            }]
        );

        state.undo_automation_outcome(undo);

        let reverted = &state.schedules["s1"];
        assert_eq!(reverted.counters, before.counters);
        assert_eq!(reverted.last_safe_outcome, None);
        assert_eq!(state.schedule_fires.len(), 0);
        assert!(state
            .record_automation_outcome("missing", outcome(ScheduleOutcomeStatus::Spoke), None, 1)
            .is_none());
    }

    #[tokio::test]
    async fn automation_updates_reach_the_automations_agent() {
        let mut state = DaemonState::new();
        let agent = state
            .create_agent(companion_config("companion"))
            .unwrap()
            .state
            .id;
        let mut stream = state.live.subscribe(&agent).unwrap();

        state.publish_automation_updated(&agent, "s1", true);

        let event = next_event(&mut stream).await.to_json(1);
        assert_eq!(event["type"], "automation.updated");
        assert_eq!(event["agentId"], agent.as_str());
        assert_eq!(event["scheduleId"], "s1");
        assert_eq!(event["deleted"], true);
        assert!(event.get("sessionId").is_none());
    }

    #[test]
    fn unmirrored_fires_skip_deleted_agents() {
        let mut state = DaemonState::new();
        let agent = state
            .create_agent(companion_config("companion"))
            .unwrap()
            .state
            .id;
        let mut live_fire = crate::schedules::history::tests_support_fire(&agent);
        live_fire.id = "kept".into();
        let mut orphan = live_fire.clone();
        orphan.id = "gone".into();
        orphan.agent_id = "agent-deleted".into();
        state.schedule_fires.record(live_fire.clone());
        state.schedule_fires.record(orphan);

        assert_eq!(state.unmirrored_schedule_fires(10), vec![live_fire]);
        assert_eq!(
            state.schedule_fires.len(),
            1,
            "the orphan is dropped, not written"
        );
    }

    #[test]
    fn automation_fields_and_fires_round_trip_through_a_snapshot() {
        let mut state = DaemonState::new();
        let agent = state
            .create_agent(companion_config("companion"))
            .unwrap()
            .state
            .id;
        let mut record = test_automation(&agent, "s1");
        record.name = "Morning brief".into();
        record.trigger = ScheduleTrigger::Cron {
            expression: "0 9 * * 1-5".into(),
            time_zone: "Europe/London".into(),
        };
        record.active_hours = Some(ActiveHours {
            start: "08:00".into(),
            end: "22:00".into(),
            days: vec![1, 2, 3, 4, 5],
            time_zone: "Europe/London".into(),
        });
        record.created_by = AutomationCreator::Agent {
            agent_id: agent.clone(),
            session_id: "chat:1".into(),
            run_id: "run_1".into(),
            tool_call_id: "call-1".into(),
        };
        record.preset = Some(AutomationPreset::Heartbeat);
        record.counters = AutomationCounters {
            runs: 3,
            failures: 1,
            consecutive_failures: 1,
        };
        state.schedules.insert("s1".into(), record.clone());
        let mut fire = crate::schedules::history::tests_support_fire(&agent);
        fire.id = "schedule:s1:10".into();
        state.schedule_fires.record(fire.clone());

        let snapshot = state.control_plane_snapshot();
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["schedules"][0]["createdBy"]["kind"], "agent");
        assert_eq!(
            json["schedules"][0]["trigger"]["cron"]["timeZone"],
            "Europe/London"
        );
        assert_eq!(json["scheduleFires"][0]["id"], "schedule:s1:10");
        let mut restored = DaemonState::new();
        restored.restore_control_plane_snapshot(snapshot).unwrap();
        assert_eq!(restored.schedules["s1"], record);
        assert_eq!(restored.schedule_fires.snapshot(), vec![fire]);
    }

    #[test]
    fn restore_refuses_invalid_automation_fields_and_fires() {
        let mut state = DaemonState::new();
        let agent = state
            .create_agent(companion_config("companion"))
            .unwrap()
            .state
            .id;
        state
            .schedules
            .insert("s1".into(), test_automation(&agent, "s1"));
        let good = state.control_plane_snapshot();

        let mut long_name = good.clone();
        long_name.schedules[0].name = "x".repeat(81);
        let mut bad_counters = good.clone();
        bad_counters.schedules[0].counters.failures = 5;
        let mut duplicate_fires = good.clone();
        let fire = crate::schedules::history::tests_support_fire(&agent);
        duplicate_fires.schedule_fires = vec![fire.clone(), fire];
        for bad in [long_name, bad_counters, duplicate_fires] {
            assert!(DaemonState::new()
                .restore_control_plane_snapshot(bad)
                .is_err());
        }
        assert!(DaemonState::new()
            .restore_control_plane_snapshot(good)
            .is_ok());
    }
}
