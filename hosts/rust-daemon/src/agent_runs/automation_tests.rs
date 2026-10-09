//! The companion's automation tools in runs (spec §9.3).

use std::collections::BTreeMap;
use std::sync::Arc;

use anima_core::{DataValue, ToolCall};
use tokio::sync::{RwLock, Semaphore};

use super::skill_tests::call;
use super::test_support::{
    chat_request, companion_config, ledger_run, tool_input, tool_results, ScriptedModel, Step,
};
use super::AgentRunCoordinator;
use crate::schedules::{
    test_automation, AutomationCreator, ScheduleTrigger, AGENT_AUTOMATION_TOO_FREQUENT,
};
use crate::state::DaemonState;
use crate::tools::automations::{
    AUTOMATIONS_LIST_HEADER, AUTOMATION_NOT_YOURS, HELPERS_CANNOT_MANAGE_AUTOMATIONS,
    TELEGRAM_NOT_READY,
};

/// A coordinator whose one companion may use `tools`.
async fn automating(model: Arc<ScriptedModel>, tools: &[&str]) -> (AgentRunCoordinator, String) {
    let mut config = companion_config("companion");
    config.tools = Some(
        crate::tools::ToolRegistry::new()
            .resolve_descriptors(tools.iter().copied())
            .unwrap(),
    );
    let mut state = DaemonState::with_model_adapter(model);
    let agent_id = state.create_agent(config).unwrap().state.id;
    (
        AgentRunCoordinator::new(Arc::new(RwLock::new(state)), Arc::new(Semaphore::new(4))),
        agent_id,
    )
}

#[tokio::test]
async fn create_automation_records_its_creator_and_answers_its_next_runs() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![call(
            "create_automation",
            &[
                ("prompt", "Remind me to stretch"),
                ("schedule", "every 30 minutes"),
                ("name", "Stretch"),
            ],
        )]),
        Step::Text(vec!["scheduled"]),
    ]);
    let (coordinator, agent_id) = automating(model, &["create_automation"]).await;

    coordinator
        .run(chat_request(&agent_id, "chat:x", "remind me to stretch"))
        .await
        .unwrap();

    let results = tool_results(&coordinator, &agent_id).await;
    let reply = results.last().unwrap();
    assert!(
        reply.starts_with("Created the automation \"Stretch\" (schedule-"),
        "{reply}"
    );
    assert_eq!(
        reply.matches("Z,").count() + 1,
        3,
        "three next runs: {reply}"
    );
    let guard = coordinator.state.read().await;
    let record = guard.schedules.values().next().expect("one automation");
    assert_eq!(
        record.trigger,
        ScheduleTrigger::Interval {
            interval_ms: 1_800_000
        }
    );
    let AutomationCreator::Agent {
        agent_id: creator,
        session_id,
        run_id,
        tool_call_id,
    } = &record.created_by
    else {
        panic!("made by the agent: {:?}", record.created_by);
    };
    assert_eq!(creator, &agent_id);
    assert_eq!(session_id, "chat:x");
    assert!(run_id.starts_with("run_"));
    assert_eq!(tool_call_id, "create_automation-1");
}

#[tokio::test]
async fn too_frequent_automations_and_a_missing_telegram_are_refused() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![call(
            "create_automation",
            &[("prompt", "Ping"), ("schedule", "every 4 minutes")],
        )]),
        Step::Tools(vec![call(
            "create_automation",
            &[
                ("prompt", "Ping"),
                ("schedule", "every hour"),
                ("target", "telegram"),
            ],
        )]),
        Step::Text(vec!["no"]),
    ]);
    let (coordinator, agent_id) = automating(model, &["create_automation"]).await;

    coordinator
        .run(chat_request(&agent_id, "chat:x", "ping me"))
        .await
        .unwrap();

    let results = tool_results(&coordinator, &agent_id).await;
    assert!(
        results[0].contains(AGENT_AUTOMATION_TOO_FREQUENT),
        "{results:?}"
    );
    assert!(results[1].contains(TELEGRAM_NOT_READY), "{results:?}");
    assert!(coordinator.state.read().await.schedules.is_empty());
}

#[tokio::test]
async fn active_hours_and_a_time_zone_are_read() {
    let mut args = BTreeMap::from([
        ("prompt".to_string(), DataValue::String("Brief me".into())),
        (
            "schedule".to_string(),
            DataValue::String("0 9 * * 1-5".into()),
        ),
        (
            "timeZone".to_string(),
            DataValue::String("Europe/London".into()),
        ),
    ]);
    args.insert(
        "activeHours".into(),
        DataValue::Object(BTreeMap::from([
            ("start".to_string(), DataValue::String("08:00".into())),
            ("end".to_string(), DataValue::String("18:00".into())),
            (
                "days".to_string(),
                DataValue::Array((1..=5).map(|day| DataValue::Number(day as f64)).collect()),
            ),
        ])),
    );
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![ToolCall {
            id: "call-hours".into(),
            name: "create_automation".into(),
            args,
        }]),
        Step::Text(vec!["ok"]),
    ]);
    let (coordinator, agent_id) = automating(model, &["create_automation"]).await;

    coordinator
        .run(chat_request(&agent_id, "chat:x", "brief me on weekdays"))
        .await
        .unwrap();

    let guard = coordinator.state.read().await;
    let record = guard.schedules.values().next().expect("one automation");
    assert_eq!(
        record.trigger,
        ScheduleTrigger::Cron {
            expression: "0 9 * * 1-5".into(),
            time_zone: "Europe/London".into(),
        }
    );
    let hours = record.active_hours.as_ref().unwrap();
    assert_eq!(hours.days, vec![1, 2, 3, 4, 5]);
    assert_eq!(hours.time_zone, "Europe/London");
}

#[tokio::test]
async fn list_and_pause_reach_only_the_agents_own_automations() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![call("list_automations", &[])]),
        Step::Tools(vec![call("pause_automation", &[("id", "mine")])]),
        Step::Tools(vec![call("pause_automation", &[("id", "theirs")])]),
        Step::Text(vec!["done"]),
    ]);
    let (coordinator, agent_id) =
        automating(model, &["list_automations", "pause_automation"]).await;
    {
        let mut guard = coordinator.state.write().await;
        let other = guard
            .create_agent(companion_config("other"))
            .unwrap()
            .state
            .id;
        guard
            .schedules
            .insert("mine".into(), test_automation(&agent_id, "mine"));
        guard
            .schedules
            .insert("theirs".into(), test_automation(&other, "theirs"));
    }

    coordinator
        .run(chat_request(&agent_id, "chat:x", "tidy my automations"))
        .await
        .unwrap();

    let results = tool_results(&coordinator, &agent_id).await;
    assert!(
        results[0].starts_with(AUTOMATIONS_LIST_HEADER),
        "{results:?}"
    );
    assert!(results[0].contains("- mine:"));
    assert!(!results[0].contains("theirs"));
    assert_eq!(results[1], "Paused the automation \"Check status\" (mine).");
    assert!(results[2].contains(AUTOMATION_NOT_YOURS), "{results:?}");
    let guard = coordinator.state.read().await;
    assert!(!guard.schedules["mine"].enabled);
    assert!(guard.schedules["theirs"].enabled);
}

#[tokio::test]
async fn helpers_cannot_manage_automations() {
    let (coordinator, agent_id) = automating(
        ScriptedModel::new(vec![]),
        &["create_automation", "list_automations", "pause_automation"],
    )
    .await;
    let link = ledger_run(&coordinator, &agent_id, "chat:x").await;
    let (context, mut helper) = {
        let guard = coordinator.state.read().await;
        (
            guard
                .tool_execution_context()
                .with_team(coordinator.clone(), false)
                .with_run_link(Some(link)),
            guard.agents[&agent_id].state(),
        )
    };
    let additional = &mut helper.config.settings.as_mut().unwrap().additional;
    additional.insert("workspaceRole".into(), DataValue::String("helper".into()));
    additional.insert(
        "parentAgentId".into(),
        DataValue::String("companion-1".into()),
    );

    for tool in [
        call(
            "create_automation",
            &[("prompt", "P"), ("schedule", "every hour")],
        ),
        call("list_automations", &[]),
        call("pause_automation", &[("id", "x")]),
    ] {
        let result = context
            .clone()
            .execute_tool(helper.clone(), tool_input(&agent_id, "chat:x"), tool)
            .await;
        assert_eq!(
            result.error.as_deref(),
            Some(HELPERS_CANNOT_MANAGE_AUTOMATIONS)
        );
    }
    assert!(coordinator.state.read().await.schedules.is_empty());
}

#[test]
fn a_helper_never_gets_the_automation_tools() {
    let mut parent = companion_config("companion");
    parent.tools = Some(
        crate::tools::ToolRegistry::new()
            .resolve_descriptors([
                "create_automation",
                "list_automations",
                "pause_automation",
                "calculate",
            ])
            .unwrap(),
    );
    let mut state = DaemonState::new();
    let parent = state.create_agent(parent).unwrap().state;

    let helper = super::helper_config(&parent, "helper".into());

    let names: Vec<String> = helper
        .tools
        .unwrap_or_default()
        .into_iter()
        .map(|tool| tool.name)
        .collect();
    assert_eq!(names, ["calculate"]);
}
