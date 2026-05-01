use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use anima_core::{AgentConfig, Content, TaskResult, TokenUsage};
use anima_swarm::{
    AgentMessage, MessageBus, StrategyContext, StrategyFn, SwarmAgentHandle, SwarmConfig,
    SwarmFuture, SwarmState, SwarmStatus, SwarmStrategy,
};

fn worker_config(name: &str) -> AgentConfig {
    AgentConfig {
        name: name.into(),
        model: "gpt-5.4".into(),
        bio: None,
        lore: None,
        knowledge: None,
        topics: None,
        adjectives: None,
        style: None,
        provider: Some("openai".into()),
        system: None,
        tools: None,
        plugins: None,
        settings: None,
    }
}

fn text_content(text: &str) -> Content {
    Content {
        text: text.into(),
        ..Content::default()
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    struct NoopWake;

    impl Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }

    let waker = Waker::from(Arc::new(NoopWake));
    let mut future = Pin::from(Box::new(future));
    let mut context = Context::from_waker(&waker);

    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

#[test]
fn swarm_types_keep_ts_shape_fields() {
    let config = SwarmConfig {
        strategy: SwarmStrategy::Supervisor,
        manager: worker_config("manager"),
        workers: vec![worker_config("worker-a"), worker_config("worker-b")],
        max_concurrent_agents: Some(2),
        max_parallel_delegations: Some(2),
        max_turns: Some(8),
        token_budget: Some(4_096),
    };

    let state = SwarmState {
        id: "swarm-1".into(),
        status: SwarmStatus::Idle,
        agent_ids: vec!["manager".into(), "worker-a".into(), "worker-b".into()],
        messages: Vec::new(),
        results: vec![TaskResult::success(text_content("done"), 12)],
        token_usage: TokenUsage {
            prompt_tokens: 5,
            completion_tokens: 7,
            total_tokens: 12,
        },
        started_at: Some(100),
        completed_at: Some(112),
    };

    let message = AgentMessage {
        id: "msg-1".into(),
        from: "manager".into(),
        to: "worker-a".into(),
        content: text_content("delegate"),
        timestamp: 123,
    };

    assert_eq!(config.strategy, SwarmStrategy::Supervisor);
    assert_eq!(config.manager.name, "manager");
    assert_eq!(config.workers.len(), 2);
    assert_eq!(state.status, SwarmStatus::Idle);
    assert_eq!(state.agent_ids.len(), 3);
    assert_eq!(state.results.len(), 1);
    assert_eq!(state.token_usage.total_tokens, 12);
    assert_eq!(message.to, "worker-a");
    assert_eq!(message.content.text, "delegate");
}

#[test]
fn strategy_context_keeps_async_spawn_agent_handle_shape() {
    let mut bus = MessageBus::new();
    let mut spawn_agent = |config: AgentConfig| -> SwarmFuture<'_, SwarmAgentHandle> {
        Box::pin(async move {
            SwarmAgentHandle {
                id: format!("{}-id", config.name),
                run: Box::new(|input: String| {
                    Box::pin(async move { TaskResult::success(text_content(&input), 1) })
                }),
            }
        })
    };
    let strategy: &StrategyFn = &|ctx| {
        Box::pin(async move {
            let handle = (ctx.spawn_agent)(ctx.worker_configs[0].clone()).await;
            let result = (handle.run)("delegate research".into()).await;
            TaskResult::success(
                Content {
                    text: format!("{}:{}", handle.id, result.data.unwrap().text),
                    ..Content::default()
                },
                2,
            )
        })
    };

    let mut context = StrategyContext {
        task: "delegate research".into(),
        manager_config: worker_config("manager"),
        worker_configs: vec![worker_config("worker-a")],
        spawn_agent: &mut spawn_agent,
        message_bus: &mut bus,
        max_parallel_delegations: 2,
        max_turns: 6,
    };

    assert_eq!(context.task, "delegate research");
    assert_eq!(context.manager_config.name, "manager");
    assert_eq!(context.worker_configs.len(), 1);
    assert_eq!(context.max_turns, 6);

    let result = block_on(strategy(&mut context));
    assert_eq!(result.status, anima_core::TaskStatus::Success);
    assert_eq!(
        result.data.as_ref().map(|content| content.text.as_str()),
        Some("worker-a-id:delegate research")
    );
}

#[test]
fn register_agent_is_lazy_and_idempotent() {
    let mut bus = MessageBus::new();

    bus.register_agent("agent-a");
    bus.register_agent("agent-a");

    assert!(bus.get_messages("agent-a").is_empty());
    assert!(bus.get_all_messages().is_empty());
}

#[test]
fn unregister_agent_removes_inbox() {
    let mut bus = MessageBus::new();
    bus.register_agent("agent-a");
    bus.send("manager", "agent-a", text_content("before unregister"));

    assert_eq!(bus.get_messages("agent-a").len(), 1);

    bus.unregister_agent("agent-a");

    assert!(bus.get_messages("agent-a").is_empty());
}

#[test]
fn send_records_global_history_and_only_delivers_to_registered_recipient() {
    let mut bus = MessageBus::new();
    bus.register_agent("worker-a");

    bus.send("manager", "worker-a", text_content("assigned"));
    bus.send("manager", "worker-b", text_content("not delivered"));

    let worker_a = bus.get_messages("worker-a");
    let worker_b = bus.get_messages("worker-b");
    let all_messages = bus.get_all_messages();

    assert_eq!(worker_a.len(), 1);
    assert_eq!(worker_a[0].from, "manager");
    assert_eq!(worker_a[0].to, "worker-a");
    assert_eq!(worker_a[0].content.text, "assigned");
    assert!(worker_b.is_empty());
    assert_eq!(all_messages.len(), 2);
    assert_eq!(all_messages[0].to, "worker-a");
    assert_eq!(all_messages[1].to, "worker-b");
}

#[test]
fn broadcast_records_once_and_delivers_to_every_registered_agent_except_sender() {
    let mut bus = MessageBus::new();
    bus.register_agent("manager");
    bus.register_agent("worker-a");
    bus.register_agent("worker-b");

    bus.broadcast("manager", text_content("team update"));

    let manager_messages = bus.get_messages("manager");
    let worker_a_messages = bus.get_messages("worker-a");
    let worker_b_messages = bus.get_messages("worker-b");
    let all_messages = bus.get_all_messages();

    assert!(manager_messages.is_empty());
    assert_eq!(worker_a_messages.len(), 1);
    assert_eq!(worker_b_messages.len(), 1);
    assert_eq!(worker_a_messages[0], worker_b_messages[0]);
    assert_eq!(worker_a_messages[0].to, "broadcast");
    assert_eq!(all_messages.len(), 1);
}

#[test]
fn get_messages_returns_empty_for_unregistered_agent() {
    let bus = MessageBus::new();

    assert!(bus.get_messages("missing").is_empty());
}

#[test]
fn get_all_messages_returns_a_clone_of_history() {
    let mut bus = MessageBus::new();
    bus.register_agent("worker-a");
    bus.send("manager", "worker-a", text_content("first"));

    let mut snapshot = bus.get_all_messages();
    snapshot.push(AgentMessage {
        id: "external".into(),
        from: "other".into(),
        to: "broadcast".into(),
        content: text_content("mutated"),
        timestamp: 999,
    });

    assert_eq!(snapshot.len(), 2);
    assert_eq!(bus.get_all_messages().len(), 1);
}

#[test]
fn clear_resets_registrations_and_history() {
    let mut bus = MessageBus::new();
    bus.register_agent("manager");
    bus.register_agent("worker-a");
    bus.send("manager", "worker-a", text_content("one"));
    bus.broadcast("manager", text_content("two"));

    bus.clear();

    assert!(bus.get_messages("manager").is_empty());
    assert!(bus.get_messages("worker-a").is_empty());
    assert!(bus.get_all_messages().is_empty());
}

#[test]
fn clear_inboxes_preserves_registration_and_global_history() {
    let mut bus = MessageBus::new();
    bus.register_agent("manager");
    bus.register_agent("worker-a");
    bus.register_agent("worker-b");
    bus.send("manager", "worker-a", text_content("direct"));
    bus.broadcast("manager", text_content("broadcast"));

    let history_before = bus.get_all_messages();
    assert_eq!(history_before.len(), 2);
    assert_eq!(bus.get_messages("worker-a").len(), 2);
    assert_eq!(bus.get_messages("worker-b").len(), 1);

    bus.clear_inboxes();

    assert!(bus.get_messages("manager").is_empty());
    assert!(bus.get_messages("worker-a").is_empty());
    assert!(bus.get_messages("worker-b").is_empty());
    assert_eq!(bus.get_all_messages(), history_before);

    bus.send("manager", "worker-a", text_content("after clear"));
    assert_eq!(bus.get_messages("worker-a").len(), 1);
    assert_eq!(bus.get_all_messages().len(), 3);
}
