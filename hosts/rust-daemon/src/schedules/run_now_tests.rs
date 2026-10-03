//! Run now, active hours and `once` at claim, and outcomes with counters,
//! fire records, and `automation.updated` (spec §9.1, §9.2).

use std::sync::Arc;

use tokio::sync::Semaphore;

use super::tests::{due_schedule, service, service_with_daemon, GatedModel};
use super::*;
use crate::agent_runs::test_support::next_event;
use crate::sessions::test_support::within;
use crate::state::DaemonState;

type Gated = (
    SchedulerService,
    SharedDaemonState,
    String,
    Arc<Semaphore>,
    Arc<Semaphore>,
);

/// A scheduler whose agent's model call waits for a `release` permit.
fn gated() -> Gated {
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let daemon = DaemonState::with_model_adapter(Arc::new(GatedModel {
        entered: entered.clone(),
        release: release.clone(),
    }));
    let (service, state, agent_id, _) = service_with_daemon(daemon);
    (service, state, agent_id, entered, release)
}

/// An automation of `agent_id` next due in an hour.
async fn later(service: &SchedulerService, agent_id: &str) -> ScheduledPromptRecord {
    service
        .create(
            agent_id.into(),
            "Check status".into(),
            ScheduleTrigger::Interval {
                interval_ms: 60_000,
            },
            ScheduleTarget::Workspace,
            true,
            None,
            Some(now_ms() + 3_600_000),
            None,
        )
        .await
        .unwrap()
        .0
}

async fn entered_run(entered: &Semaphore) {
    within("the automation's run to reach its model", entered.acquire())
        .await
        .unwrap()
        .forget();
}

#[test]
fn the_run_now_strings_are_the_specs() {
    assert_eq!(
        AUTOMATION_ALREADY_RUNNING,
        "This automation is already running"
    );
    assert_eq!(
        TOO_MANY_RUNNING_AUTOMATIONS,
        "Too many automations are running; try again shortly"
    );
}

#[tokio::test]
async fn run_now_fires_without_moving_the_due_time_and_is_recorded_as_manual() {
    let (service, state, agent_id, entered, release) = gated();
    let record = later(&service, &agent_id).await;
    let paused = service
        .update(&agent_id, &record.id, None, None, None, Some(false))
        .await
        .unwrap();
    release.add_permits(1);

    let claimed = service.run_now(&agent_id, &record.id).await.unwrap();
    entered_run(&entered).await;
    service.drain().await;

    assert!(claimed.last_fired.as_ref().unwrap().manual);
    let after = state.read().await.schedules[&record.id].clone();
    assert_eq!(
        after.next_due_at_ms, paused.next_due_at_ms,
        "the due time stays"
    );
    assert!(!after.enabled, "a paused automation stays paused");
    assert_eq!(
        after.last_safe_outcome.as_ref().unwrap().status,
        ScheduleOutcomeStatus::Silent
    );
    assert_eq!(after.counters.runs, 1);
    let fires = state.read().await.schedule_fires.for_schedule(&record.id);
    assert_eq!(fires.len(), 1);
    assert!(fires[0].manual);
    assert!(fires[0]
        .id
        .starts_with(&format!("schedule:{}:manual:", record.id)));
    assert!(fires[0].run_id.is_some());
    assert_eq!(
        fires[0].session_id.as_deref(),
        Some(crate::sessions::schedule_room_id(&record.id).as_str())
    );
}

#[tokio::test]
async fn run_now_refuses_while_the_automation_runs() {
    let (service, _state, agent_id, entered, release) = gated();
    let record = later(&service, &agent_id).await;
    service.run_now(&agent_id, &record.id).await.unwrap();
    entered_run(&entered).await;

    assert_eq!(
        service.run_now(&agent_id, &record.id).await,
        Err(ScheduleError::Conflict(AUTOMATION_ALREADY_RUNNING))
    );

    release.add_permits(2);
    service.drain().await;
    service
        .run_now(&agent_id, &record.id)
        .await
        .expect("free again once its run finished");
    entered_run(&entered).await;
    service.drain().await;
}

#[tokio::test]
async fn run_now_keeps_ownership_and_the_admission_cap() {
    let (service, _state, agent_id, _, _) = gated();
    let record = later(&service, &agent_id).await;
    assert_eq!(
        service.run_now("someone-else", &record.id).await,
        Err(ScheduleError::NotFound)
    );
    assert_eq!(
        service.run_now(&agent_id, "missing").await,
        Err(ScheduleError::NotFound)
    );
    {
        let mut jobs = service.inner.jobs.lock().await;
        for n in 0..MAX_ACTIVE_SCHEDULES {
            jobs.insert(
                format!("busy-{n}"),
                tokio::spawn(std::future::pending::<()>()),
            );
        }
    }
    assert_eq!(
        service.run_now(&agent_id, &record.id).await,
        Err(ScheduleError::Busy(TOO_MANY_RUNNING_AUTOMATIONS))
    );
    for (_, job) in std::mem::take(&mut *service.inner.jobs.lock().await) {
        job.abort();
    }
}

#[tokio::test]
async fn a_once_automation_fires_once_and_turns_itself_off() {
    let (service, state, agent_id, entered, release) = gated();
    let at = now_ms() + 3_600_000;
    let (record, _) = service
        .automations()
        .create(
            AutomationInput::owner(
                agent_id.clone(),
                "Remind me".into(),
                ScheduleTrigger::Once { at_ms: at },
                ScheduleTarget::Workspace,
            ),
            now_ms(),
        )
        .await
        .unwrap();
    release.add_permits(1);

    let ticking = service.clone();
    let tick = tokio::spawn(async move { ticking.tick_at(at).await });
    entered_run(&entered).await;
    assert_eq!(tick.await.unwrap().unwrap(), 1);

    let fired = state.read().await.schedules[&record.id].clone();
    assert!(!fired.enabled, "the claim turned it off");
    assert_eq!(fired.next_due_at_ms, at);
    assert_eq!(fired.counters.runs, 1);
    assert_eq!(
        service.tick_at(at + 3_600_000).await.unwrap(),
        0,
        "it never fires again"
    );
}

#[tokio::test]
async fn active_hours_move_the_next_due_time_at_claim() {
    let (service, state, agent_id, entered, release) = gated();
    // 2026-01-05 21:30 UTC; the window closes at 22:00.
    let due = 1_767_648_600_000;
    let mut record = crate::schedules::test_automation(&agent_id, "windowed");
    record.trigger = ScheduleTrigger::Interval {
        interval_ms: 30 * 60_000,
    };
    record.active_hours = Some(ActiveHours {
        start: "08:00".into(),
        end: "22:00".into(),
        days: (0..=6).collect(),
        time_zone: "UTC".into(),
    });
    record.next_due_at_ms = due;
    state
        .write()
        .await
        .schedules
        .insert(record.id.clone(), record);
    release.add_permits(1);

    let ticking = service.clone();
    let tick = tokio::spawn(async move { ticking.tick_at(due + 60_000).await });
    entered_run(&entered).await;
    tick.await.unwrap().unwrap();

    assert_eq!(
        state.read().await.schedules["windowed"].next_due_at_ms,
        due + (10 * 60 + 30) * 60_000,
        "the next day's 08:00, not 22:00"
    );
}

#[tokio::test]
async fn outcomes_count_record_a_fire_and_announce() {
    let (service, state, agent_id, entered, release) = gated();
    let mut stream = state.read().await.live.subscribe(&agent_id).unwrap();
    let record = due_schedule(&service, &agent_id).await;
    release.add_permits(1);

    let ticking = service.clone();
    let tick = tokio::spawn(async move { ticking.tick_at(now_ms()).await });
    entered_run(&entered).await;
    tick.await.unwrap().unwrap();

    let after = state.read().await.schedules[&record.id].clone();
    assert_eq!(
        after.counters,
        AutomationCounters {
            runs: 1,
            failures: 0,
            consecutive_failures: 0
        }
    );
    let fires = state.read().await.schedule_fires.for_schedule(&record.id);
    assert_eq!(fires.len(), 1);
    assert_eq!(fires[0].outcome, ScheduleOutcomeStatus::Silent);
    assert!(!fires[0].manual);
    // Created, claimed, and finished.
    let mut announced = 0;
    while announced < 3 {
        let event = next_event(&mut stream).await.to_json(1);
        if event["type"] == "automation.updated" && event["scheduleId"] == record.id.as_str() {
            announced += 1;
        }
    }
}

#[tokio::test]
async fn a_failed_commit_save_rolls_back_the_outcome_counters_and_fire() {
    let (service, state, agent_id, entered, release) = gated();
    let record = due_schedule(&service, &agent_id).await;
    let ticking = service.clone();
    let tick = tokio::spawn(async move { ticking.tick_at(now_ms()).await });
    entered_run(&entered).await;
    let directory =
        std::env::temp_dir().join(format!("anima-fire-rollback-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    state.write().await.set_control_plane_store(Some(
        crate::control_plane_store::ControlPlaneStoreConfig::Json(directory.clone()),
    ));
    release.add_permits(1);
    tick.await.unwrap().unwrap();

    let after = state.read().await.schedules[&record.id].clone();
    assert!(after.last_safe_outcome.is_none());
    assert_eq!(after.counters, AutomationCounters::default());
    assert!(state
        .read()
        .await
        .schedule_fires
        .for_schedule(&record.id)
        .is_empty());
    let _ = std::fs::remove_dir_all(&directory);
}

#[tokio::test]
async fn restart_reconciliation_records_a_failed_fire() {
    let (service, state, agent_id, _) = service();
    let record = due_schedule(&service, &agent_id).await;
    claim_due(&service.inner, &record.id, now_ms())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(service.tick_at(now_ms()).await.unwrap(), 0);

    let after = state.read().await.schedules[&record.id].clone();
    assert!(!after.enabled);
    assert_eq!(
        after.counters,
        AutomationCounters {
            runs: 1,
            failures: 1,
            consecutive_failures: 1
        }
    );
    let fires = state.read().await.schedule_fires.for_schedule(&record.id);
    assert_eq!(fires.len(), 1);
    assert_eq!(fires[0].outcome, ScheduleOutcomeStatus::Failed);
    assert_eq!(
        fires[0].error_code.as_deref(),
        Some("schedule_run_interrupted")
    );
    assert_eq!(fires[0].run_id, None, "no run started before the restart");
}

/// A JSON store at a directory: every save fails.
fn broken_store(state: &mut DaemonState) -> std::path::PathBuf {
    let directory =
        std::env::temp_dir().join(format!("anima-broken-store-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    state.set_control_plane_store(Some(
        crate::control_plane_store::ControlPlaneStoreConfig::Json(directory.clone()),
    ));
    directory
}

/// Counters and fires an earlier occurrence left, so a rollback that reset
/// them instead of restoring them would show.
async fn with_history(state: &SharedDaemonState, schedule_id: &str, agent_id: &str) {
    let mut guard = state.write().await;
    guard.schedules.get_mut(schedule_id).unwrap().counters = AutomationCounters {
        runs: 4,
        failures: 2,
        consecutive_failures: 1,
    };
    for n in 1..=2 {
        let mut fire = crate::schedules::history::tests_support_fire(agent_id);
        fire.id = format!("schedule:{schedule_id}:{n}");
        fire.schedule_id = schedule_id.to_string();
        guard.schedule_fires.record(fire);
    }
}

/// Controller ruling 3: a failed save restores exactly the previous
/// counters and fire records.
#[tokio::test]
async fn a_failed_outcome_save_restores_the_counters_and_fires_exactly() {
    let (service, state, agent_id, _) = service();
    let record = due_schedule(&service, &agent_id).await;
    with_history(&state, &record.id, &agent_id).await;
    claim_due(&service.inner, &record.id, now_ms())
        .await
        .unwrap()
        .unwrap();
    let before = state.read().await.schedules[&record.id].clone();
    let fires_before = state.read().await.schedule_fires.snapshot();
    let directory = broken_store(&mut *state.write().await);

    assert_eq!(
        record_outcome(
            &service.inner,
            &record.id,
            ScheduleOutcomeStatus::Failed,
            Some("schedule_run_failed"),
            now_ms(),
            None,
        )
        .await,
        Err(ScheduleError::Persistence)
    );

    let after = state.read().await.schedules[&record.id].clone();
    assert_eq!(after.counters, before.counters);
    assert_eq!(after.last_safe_outcome, before.last_safe_outcome);
    assert_eq!(state.read().await.schedule_fires.snapshot(), fires_before);
    let _ = std::fs::remove_dir_all(&directory);
}

/// Controller ruling 3, during restart reconciliation.
#[tokio::test]
async fn a_failed_reconciliation_save_restores_the_counters_and_fires_exactly() {
    let (service, state, agent_id, _) = service();
    let record = due_schedule(&service, &agent_id).await;
    with_history(&state, &record.id, &agent_id).await;
    claim_due(&service.inner, &record.id, now_ms())
        .await
        .unwrap()
        .unwrap();
    let before = state.read().await.schedules.clone();
    let fires_before = state.read().await.schedule_fires.snapshot();
    let directory = broken_store(&mut *state.write().await);

    assert_eq!(
        reconcile_interrupted(&service.inner, now_ms(), &BTreeSet::new()).await,
        Err(ScheduleError::Persistence)
    );

    assert_eq!(state.read().await.schedules, before);
    assert_eq!(state.read().await.schedule_fires.snapshot(), fires_before);
    let _ = std::fs::remove_dir_all(&directory);
}

#[tokio::test]
async fn run_now_refuses_an_occurrence_still_waiting_for_its_outcome() {
    let (service, _state, agent_id, _) = service();
    let record = later(&service, &agent_id).await;
    claim_manual(&service.inner, &agent_id, &record.id, now_ms())
        .await
        .unwrap();
    assert_eq!(
        service.run_now(&agent_id, &record.id).await,
        Err(ScheduleError::Conflict(AUTOMATION_ALREADY_RUNNING))
    );
}

#[tokio::test]
async fn a_restart_during_run_now_disables_the_automation_and_records_a_manual_fire() {
    let (service, state, agent_id, _) = service();
    let record = later(&service, &agent_id).await;
    claim_manual(&service.inner, &agent_id, &record.id, now_ms())
        .await
        .unwrap();

    reconcile_interrupted(&service.inner, now_ms(), &BTreeSet::new())
        .await
        .unwrap();

    let after = state.read().await.schedules[&record.id].clone();
    assert!(!after.enabled);
    assert_eq!(after.next_due_at_ms, record.next_due_at_ms);
    let fires = state.read().await.schedule_fires.for_schedule(&record.id);
    assert_eq!(fires.len(), 1);
    assert!(fires[0].manual);
    assert_eq!(fires[0].outcome, ScheduleOutcomeStatus::Failed);
}
