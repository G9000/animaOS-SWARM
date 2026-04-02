use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::Duration;

use anima_core::{AgentConfig, Content, DataValue, TaskResult, TaskStatus, TokenUsage};
use anima_swarm::coordinator::{
    CoordinatorAgentFactoryContext, CoordinatorAgentFactoryFn, CoordinatorAgentRef,
    CoordinatorAgentShell, CoordinatorDelegateFn, CoordinatorDispatchContext,
    CoordinatorStrategyFn,
};
use anima_swarm::strategies::resolve_strategy;
use anima_swarm::strategies::supervisor::supervisor_strategy;
use anima_swarm::{SwarmConfig, SwarmCoordinator, SwarmStatus, SwarmStrategy};

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

fn base_config(worker_names: &[&str]) -> SwarmConfig {
    SwarmConfig {
        strategy: SwarmStrategy::Supervisor,
        manager: worker_config("manager"),
        workers: worker_names
            .iter()
            .map(|name| worker_config(name))
            .collect(),
        max_concurrent_agents: None,
        max_turns: Some(4),
        token_budget: None,
    }
}

fn round_robin_config(worker_names: &[&str]) -> SwarmConfig {
    SwarmConfig {
        strategy: SwarmStrategy::RoundRobin,
        manager: worker_config("manager"),
        workers: worker_names
            .iter()
            .map(|name| worker_config(name))
            .collect(),
        max_concurrent_agents: None,
        max_turns: Some(4),
        token_budget: None,
    }
}

fn text_content(text: &str) -> Content {
    Content {
        text: text.into(),
        ..Content::default()
    }
}

fn data_value_as_str(value: &DataValue) -> &str {
    match value {
        DataValue::String(text) => text.as_str(),
        other => panic!("expected string data value, got {other:?}"),
    }
}

fn data_value_as_object(value: &DataValue) -> &std::collections::BTreeMap<String, DataValue> {
    match value {
        DataValue::Object(object) => object,
        other => panic!("expected object data value, got {other:?}"),
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
            Poll::Pending => thread::yield_now(),
        }
    }
}

#[derive(Default)]
struct TestHarnessState {
    spawn_counts: HashMap<String, usize>,
    spawn_log: Vec<String>,
    run_log: Vec<String>,
    clear_log: Vec<String>,
    stop_log: Vec<String>,
}

#[derive(Clone)]
struct TestHarness {
    state: Arc<Mutex<TestHarnessState>>,
    tokens: Arc<Mutex<HashMap<String, TokenUsage>>>,
}

impl TestHarness {
    fn new(tokens: HashMap<String, TokenUsage>) -> Self {
        Self {
            state: Arc::new(Mutex::new(TestHarnessState::default())),
            tokens: Arc::new(Mutex::new(tokens)),
        }
    }

    fn set_tokens(&self, agent_name: &str, token_usage: TokenUsage) {
        self.tokens
            .lock()
            .expect("token usage mutex should not be poisoned")
            .insert(agent_name.to_string(), token_usage);
    }

    fn factory(&self) -> Arc<CoordinatorAgentFactoryFn> {
        let shared = self.state.clone();
        let tokens = self.tokens.clone();

        Arc::new(move |context: CoordinatorAgentFactoryContext| {
            let shared = shared.clone();
            let tokens = tokens.clone();

            Box::pin(async move {
                let agent_id = {
                    let mut state = shared
                        .lock()
                        .expect("test harness state mutex should not be poisoned");
                    let _ = state
                        .spawn_counts
                        .entry(context.config.name.clone())
                        .and_modify(|value| *value += 1)
                        .or_insert(1);
                    state.spawn_log.push(context.config.name.clone());

                    context.agent_id.clone()
                };

                let run_state = shared.clone();
                let run_id = agent_id.clone();
                let send = context.send.clone();
                let broadcast = context.broadcast.clone();
                let stop_state = shared.clone();
                let stop_id = agent_id.clone();
                let clear_state = shared.clone();
                let clear_id = agent_id.clone();

                Ok(CoordinatorAgentShell {
                    run: Arc::new(move |input| {
                        let run_state = run_state.clone();
                        let run_id = run_id.clone();
                        let send = send.clone();
                        let broadcast = broadcast.clone();
                        Box::pin(async move {
                            run_state
                                .lock()
                                .expect("test harness state mutex should not be poisoned")
                                .run_log
                                .push(format!("{run_id}:{input}"));
                            if let Some(rest) = input.strip_prefix("send:") {
                                let mut parts = rest.splitn(2, ':');
                                let target =
                                    parts.next().expect("send target should exist").to_string();
                                let message =
                                    parts.next().expect("send payload should exist").to_string();
                                send(target, text_content(&message))
                                    .await
                                    .expect("send hook should succeed");
                            }
                            if let Some(message) = input.strip_prefix("broadcast:") {
                                broadcast(text_content(message))
                                    .await
                                    .expect("broadcast hook should succeed");
                            }
                            TaskResult::success(
                                text_content(&format!("{run_id} handled {input}")),
                                1,
                            )
                        })
                    }),
                    token_usage: Arc::new({
                        let tokens = tokens.clone();
                        let agent_name = context.config.name.clone();
                        move || {
                            tokens
                                .lock()
                                .expect("token usage mutex should not be poisoned")
                                .get(&agent_name)
                                .cloned()
                                .unwrap_or_else(TokenUsage::default)
                        }
                    }),
                    clear_task_state: Arc::new(move || {
                        clear_state
                            .lock()
                            .expect("test harness state mutex should not be poisoned")
                            .clear_log
                            .push(clear_id.clone());
                    }),
                    stop: Arc::new(move || {
                        let stop_state = stop_state.clone();
                        let stop_id = stop_id.clone();
                        Box::pin(async move {
                            stop_state
                                .lock()
                                .expect("test harness state mutex should not be poisoned")
                                .stop_log
                                .push(stop_id);
                        })
                    }),
                })
            })
        })
    }

    fn snapshot(&self) -> TestHarnessState {
        self.state
            .lock()
            .expect("test harness state mutex should not be poisoned")
            .clone()
    }
}

impl Clone for TestHarnessState {
    fn clone(&self) -> Self {
        Self {
            spawn_counts: self.spawn_counts.clone(),
            spawn_log: self.spawn_log.clone(),
            run_log: self.run_log.clone(),
            clear_log: self.clear_log.clone(),
            stop_log: self.stop_log.clone(),
        }
    }
}

#[test]
fn start_populates_workers_and_dispatch_reuses_the_pool() {
    let harness = TestHarness::new(HashMap::from([
        (
            "worker-a".into(),
            TokenUsage {
                prompt_tokens: 3,
                completion_tokens: 2,
                total_tokens: 5,
            },
        ),
        (
            "worker-b".into(),
            TokenUsage {
                prompt_tokens: 4,
                completion_tokens: 3,
                total_tokens: 7,
            },
        ),
        (
            "manager".into(),
            TokenUsage {
                prompt_tokens: 5,
                completion_tokens: 4,
                total_tokens: 9,
            },
        ),
    ]));

    let strategy: Arc<CoordinatorStrategyFn> = Arc::new(|ctx: CoordinatorDispatchContext| {
        Box::pin(async move {
            let worker = ctx
                .spawn_agent(ctx.worker_configs()[0].clone())
                .await
                .expect("worker spawn should succeed");
            let manager = ctx
                .spawn_agent(ctx.manager_config().clone())
                .await
                .expect("manager spawn should succeed");
            let worker_result = worker.run(ctx.task().to_string()).await;
            let worker_text = worker_result
                .data
                .as_ref()
                .map(|content| content.text.as_str())
                .expect("worker result should contain text");
            let manager_result = manager.run(format!("summarize {worker_text}")).await;
            let manager_text = manager_result
                .data
                .as_ref()
                .map(|content| content.text.as_str())
                .expect("manager result should contain text");

            TaskResult::success(text_content(manager_text), 2)
        })
    });

    let coordinator = SwarmCoordinator::with_hooks(
        base_config(&["worker-a", "worker-b"]),
        strategy,
        harness.factory(),
    );

    block_on(coordinator.start()).expect("start should succeed");
    let started = coordinator.get_state();
    assert_eq!(started.status, SwarmStatus::Idle);
    assert_eq!(started.agent_ids.len(), 2);

    let first = block_on(coordinator.dispatch("task one"));
    let second = block_on(coordinator.dispatch("task two"));

    assert_eq!(first.status, TaskStatus::Success);
    assert_eq!(second.status, TaskStatus::Success);

    let snapshot = harness.snapshot();
    assert_eq!(snapshot.spawn_counts.get("worker-a"), Some(&1));
    assert_eq!(snapshot.spawn_counts.get("worker-b"), Some(&1));
    assert_eq!(snapshot.spawn_counts.get("manager"), Some(&2));
    assert_eq!(snapshot.run_log.len(), 4);

    let state = coordinator.get_state();
    assert_eq!(state.results.len(), 2);
    assert_eq!(state.token_usage.total_tokens, 12);
}

#[test]
fn with_config_resolves_supervisor_strategy_and_uses_default_agent_factory_message() {
    let coordinator = SwarmCoordinator::with_config(base_config(&["worker-a"]));

    let result = block_on(coordinator.dispatch("Research and report"));

    assert_eq!(result.status, TaskStatus::Error);
    assert_eq!(
        result.error.as_deref(),
        Some("No coordinator agent factory configured for worker-a")
    );
}

#[test]
fn supervisor_strategy_delegates_to_worker_and_returns_the_manager_synthesis() {
    let spawn_order = Arc::new(Mutex::new(Vec::<String>::new()));
    let worker_inputs = Arc::new(Mutex::new(Vec::<String>::new()));
    let manager_inputs = Arc::new(Mutex::new(Vec::<String>::new()));

    let factory: Arc<CoordinatorAgentFactoryFn> = Arc::new({
        let spawn_order = Arc::clone(&spawn_order);
        let worker_inputs = Arc::clone(&worker_inputs);
        let manager_inputs = Arc::clone(&manager_inputs);

        move |context: CoordinatorAgentFactoryContext| {
            let spawn_order = Arc::clone(&spawn_order);
            let worker_inputs = Arc::clone(&worker_inputs);
            let manager_inputs = Arc::clone(&manager_inputs);

            Box::pin(async move {
                spawn_order
                    .lock()
                    .expect("spawn order mutex should not be poisoned")
                    .push(context.config.name.clone());

                match context.config.name.as_str() {
                    "worker-a" | "worker-b" => {
                        let run = Arc::new(move |input: String| {
                            let worker_inputs = Arc::clone(&worker_inputs);
                            Box::pin(async move {
                                worker_inputs
                                    .lock()
                                    .expect("worker input mutex should not be poisoned")
                                    .push(input);
                                TaskResult::success(
                                    text_content("worker result: research complete"),
                                    1,
                                )
                            })
                                as Pin<Box<dyn Future<Output = TaskResult<Content>> + Send>>
                        });

                        let shell = CoordinatorAgentShell {
                            run,
                            token_usage: Arc::new(TokenUsage::default),
                            clear_task_state: Arc::new(|| {}),
                            stop: Arc::new(|| Box::pin(async {})),
                        };
                        Ok(shell)
                    }
                    "manager" => {
                        let system = context.config.system.as_deref().unwrap_or_default();
                        assert!(
                            system.contains("delegate_task"),
                            "supervisor manager prompt should mention delegate_task"
                        );

                        let tool_names = context
                            .config
                            .tools
                            .as_deref()
                            .unwrap_or(&[])
                            .iter()
                            .map(|tool| tool.name.as_str())
                            .collect::<Vec<_>>();
                        assert!(
                            tool_names.contains(&"delegate_task"),
                            "supervisor manager should receive delegate_task tool"
                        );
                        let delegate_task = context
                            .delegate_task
                            .as_ref()
                            .expect("supervisor manager should receive delegate_task callback")
                            .clone();

                        let run = Arc::new(move |input: String| {
                            let manager_inputs = Arc::clone(&manager_inputs);
                            let delegate_task = Arc::clone(&delegate_task);
                            Box::pin(async move {
                                manager_inputs
                                    .lock()
                                    .expect("manager input mutex should not be poisoned")
                                    .push(input);

                                let worker_result =
                                    delegate_task("worker-b".into(), "Do research".into()).await;
                                assert_eq!(worker_result.status, TaskStatus::Success);
                                assert_eq!(
                                    worker_result
                                        .data
                                        .as_ref()
                                        .map(|content| content.text.as_str()),
                                    Some("worker result: research complete")
                                );

                                let unknown_worker_result =
                                    delegate_task("missing-worker".into(), "Do research".into())
                                        .await;
                                assert_eq!(unknown_worker_result.status, TaskStatus::Error);
                                assert_eq!(
                                    unknown_worker_result.error.as_deref(),
                                    Some(
                                        "Worker \"missing-worker\" not found. Available: \"worker-a\", \"worker-b\""
                                    )
                                );

                                TaskResult::success(
                                    text_content("Final synthesis: research is complete."),
                                    2,
                                )
                            })
                                as Pin<Box<dyn Future<Output = TaskResult<Content>> + Send>>
                        });

                        Ok(CoordinatorAgentShell {
                            run,
                            token_usage: Arc::new(TokenUsage::default),
                            clear_task_state: Arc::new(|| {}),
                            stop: Arc::new(|| Box::pin(async {})),
                        })
                    }
                    other => Err(format!("unexpected agent config: {other}")),
                }
            })
        }
    });

    let coordinator = SwarmCoordinator::with_hooks(
        base_config(&["worker-a", "worker-b"]),
        Arc::new(supervisor_strategy),
        factory,
    );

    let result = block_on(coordinator.dispatch("Research and report"));

    assert_eq!(result.status, TaskStatus::Success);
    assert_eq!(
        result.data.as_ref().map(|content| content.text.as_str()),
        Some("Final synthesis: research is complete.")
    );
    assert_eq!(
        spawn_order
            .lock()
            .expect("spawn order mutex should not be poisoned")
            .as_slice(),
        ["worker-a", "worker-b", "manager"]
    );
    assert_eq!(
        worker_inputs
            .lock()
            .expect("worker input mutex should not be poisoned")
            .as_slice(),
        ["Do research"]
    );
    assert_eq!(
        manager_inputs
            .lock()
            .expect("manager input mutex should not be poisoned")
            .as_slice(),
        ["Research and report"]
    );
}

#[test]
fn stale_delegate_callbacks_are_fenced_by_manager_liveness() {
    let worker_inputs = Arc::new(Mutex::new(Vec::<String>::new()));
    let saved_delegate = Arc::new(Mutex::new(None::<Arc<CoordinatorDelegateFn>>));
    let saved_manager_id = Arc::new(Mutex::new(None::<String>));

    let factory: Arc<CoordinatorAgentFactoryFn> = Arc::new({
        let worker_inputs = Arc::clone(&worker_inputs);
        let saved_delegate = Arc::clone(&saved_delegate);
        let saved_manager_id = Arc::clone(&saved_manager_id);

        move |context: CoordinatorAgentFactoryContext| {
            let worker_inputs = Arc::clone(&worker_inputs);
            let saved_delegate = Arc::clone(&saved_delegate);
            let saved_manager_id = Arc::clone(&saved_manager_id);

            Box::pin(async move {
                match context.config.name.as_str() {
                    "worker-a" => {
                        let run = Arc::new(move |input: String| {
                            let worker_inputs = Arc::clone(&worker_inputs);
                            Box::pin(async move {
                                worker_inputs
                                    .lock()
                                    .expect("worker input mutex should not be poisoned")
                                    .push(input);
                                TaskResult::success(text_content("worker result"), 1)
                            })
                                as Pin<Box<dyn Future<Output = TaskResult<Content>> + Send>>
                        });

                        Ok(CoordinatorAgentShell {
                            run,
                            token_usage: Arc::new(TokenUsage::default),
                            clear_task_state: Arc::new(|| {}),
                            stop: Arc::new(|| Box::pin(async {})),
                        })
                    }
                    "manager" => {
                        saved_manager_id
                            .lock()
                            .expect("saved manager id mutex should not be poisoned")
                            .replace(context.agent_id.clone());
                        saved_delegate
                            .lock()
                            .expect("saved delegate mutex should not be poisoned")
                            .replace(
                                context
                                    .delegate_task
                                    .as_ref()
                                    .expect("manager should receive delegate_task callback")
                                    .clone(),
                            );

                        let delegate_task = context
                            .delegate_task
                            .as_ref()
                            .expect("manager should receive delegate_task callback")
                            .clone();
                        let run = Arc::new(move |input: String| {
                            let delegate_task = Arc::clone(&delegate_task);
                            Box::pin(async move {
                                assert_eq!(input, "Research and report");

                                let worker_result =
                                    delegate_task("worker-a".into(), "Do research".into()).await;
                                assert_eq!(worker_result.status, TaskStatus::Success);
                                assert_eq!(
                                    worker_result
                                        .data
                                        .as_ref()
                                        .map(|content| content.text.as_str()),
                                    Some("worker result")
                                );

                                TaskResult::success(
                                    text_content("Final synthesis: research is complete."),
                                    2,
                                )
                            })
                                as Pin<Box<dyn Future<Output = TaskResult<Content>> + Send>>
                        });

                        Ok(CoordinatorAgentShell {
                            run,
                            token_usage: Arc::new(TokenUsage::default),
                            clear_task_state: Arc::new(|| {}),
                            stop: Arc::new(|| Box::pin(async {})),
                        })
                    }
                    other => Err(format!("unexpected agent config: {other}")),
                }
            })
        }
    });

    let coordinator = SwarmCoordinator::with_hooks(
        base_config(&["worker-a"]),
        Arc::new(supervisor_strategy),
        factory,
    );

    let result = block_on(coordinator.dispatch("Research and report"));
    assert_eq!(result.status, TaskStatus::Success);

    let delegate_task = saved_delegate
        .lock()
        .expect("saved delegate mutex should not be poisoned")
        .clone()
        .expect("delegate_task should be captured");
    let manager_id = saved_manager_id
        .lock()
        .expect("saved manager id mutex should not be poisoned")
        .clone()
        .expect("manager id should be captured");

    let stale_result = block_on(delegate_task("worker-a".into(), "Late follow-up".into()));
    assert_eq!(stale_result.status, TaskStatus::Error);
    assert_eq!(
        stale_result.error.as_deref(),
        Some(format!("Coordinator agent {manager_id} is no longer active").as_str())
    );
    assert_eq!(
        worker_inputs
            .lock()
            .expect("worker input mutex should not be poisoned")
            .as_slice(),
        ["Do research"]
    );
}

#[test]
fn dispatch_is_serial_and_clears_inboxes_between_tasks() {
    let harness = TestHarness::new(HashMap::from([(
        "worker-a".into(),
        TokenUsage {
            prompt_tokens: 1,
            completion_tokens: 1,
            total_tokens: 2,
        },
    )]));
    let order = Arc::new(Mutex::new(Vec::<String>::new()));
    let inbox_sizes = Arc::new(Mutex::new(Vec::<usize>::new()));
    let (first_started_tx, first_started_rx) = mpsc::channel();
    let (release_first_tx, release_first_rx) = mpsc::channel();
    let first_started_tx = Arc::new(Mutex::new(Some(first_started_tx)));
    let release_first_rx = Arc::new(Mutex::new(Some(release_first_rx)));

    let strategy: Arc<CoordinatorStrategyFn> = Arc::new({
        let order = order.clone();
        let inbox_sizes = inbox_sizes.clone();
        let first_started_tx = first_started_tx.clone();
        let release_first_rx = release_first_rx.clone();

        move |ctx: CoordinatorDispatchContext| {
            let order = order.clone();
            let inbox_sizes = inbox_sizes.clone();
            let first_started_tx = first_started_tx.clone();
            let release_first_rx = release_first_rx.clone();

            Box::pin(async move {
                order
                    .lock()
                    .expect("order mutex should not be poisoned")
                    .push(format!("start:{}", ctx.task()));

                let worker = ctx
                    .spawn_agent(ctx.worker_configs()[0].clone())
                    .await
                    .expect("worker spawn should succeed");
                let worker_inbox = ctx
                    .message_bus()
                    .lock()
                    .expect("message bus mutex should not be poisoned")
                    .get_messages(&worker.id)
                    .len();
                inbox_sizes
                    .lock()
                    .expect("inbox sizes mutex should not be poisoned")
                    .push(worker_inbox);

                if ctx.task() == "task one" {
                    if let Some(sender) = first_started_tx
                        .lock()
                        .expect("channel mutex should not be poisoned")
                        .take()
                    {
                        sender
                            .send(())
                            .expect("first started notification should send");
                    }
                    if let Some(receiver) = release_first_rx
                        .lock()
                        .expect("channel mutex should not be poisoned")
                        .take()
                    {
                        receiver
                            .recv()
                            .expect("first dispatch release should arrive");
                    }
                }

                ctx.message_bus()
                    .lock()
                    .expect("message bus mutex should not be poisoned")
                    .send("manager", &worker.id, text_content(ctx.task()));

                let result = worker.run(ctx.task().to_string()).await;
                order
                    .lock()
                    .expect("order mutex should not be poisoned")
                    .push(format!("end:{}", ctx.task()));
                result
            })
        }
    });

    let coordinator =
        SwarmCoordinator::with_hooks(base_config(&["worker-a"]), strategy, harness.factory());
    block_on(coordinator.start()).expect("start should succeed");
    let worker_id = coordinator.get_state().agent_ids[0].clone();

    coordinator
        .get_message_bus()
        .lock()
        .expect("message bus mutex should not be poisoned")
        .send("manager", &worker_id, text_content("stale"));

    let first_coordinator = coordinator.clone();
    let first = thread::spawn(move || block_on(first_coordinator.dispatch("task one")));
    first_started_rx
        .recv()
        .expect("first dispatch should report start");

    let second_coordinator = coordinator.clone();
    let second = thread::spawn(move || block_on(second_coordinator.dispatch("task two")));

    thread::sleep(Duration::from_millis(30));
    assert_eq!(
        order
            .lock()
            .expect("order mutex should not be poisoned")
            .as_slice(),
        ["start:task one"]
    );

    release_first_tx
        .send(())
        .expect("first dispatch release should send");

    let first_result = first.join().expect("first dispatch thread should join");
    let second_result = second.join().expect("second dispatch thread should join");

    assert_eq!(first_result.status, TaskStatus::Success);
    assert_eq!(second_result.status, TaskStatus::Success);
    assert_eq!(
        order
            .lock()
            .expect("order mutex should not be poisoned")
            .as_slice(),
        [
            "start:task one",
            "end:task one",
            "start:task two",
            "end:task two"
        ]
    );
    assert_eq!(
        inbox_sizes
            .lock()
            .expect("inbox sizes mutex should not be poisoned")
            .as_slice(),
        [0, 0]
    );
}

#[test]
fn stop_waits_for_in_flight_dispatch_before_stopping_agents() {
    let harness = TestHarness::new(HashMap::from([
        (
            "worker-a".into(),
            TokenUsage {
                prompt_tokens: 2,
                completion_tokens: 1,
                total_tokens: 3,
            },
        ),
        (
            "manager".into(),
            TokenUsage {
                prompt_tokens: 4,
                completion_tokens: 2,
                total_tokens: 6,
            },
        ),
    ]));
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (stop_done_tx, stop_done_rx) = mpsc::channel();
    let started_tx = Arc::new(Mutex::new(Some(started_tx)));
    let release_rx = Arc::new(Mutex::new(Some(release_rx)));

    let strategy: Arc<CoordinatorStrategyFn> = Arc::new({
        let started_tx = started_tx.clone();
        let release_rx = release_rx.clone();

        move |ctx: CoordinatorDispatchContext| {
            let started_tx = started_tx.clone();
            let release_rx = release_rx.clone();

            Box::pin(async move {
                let worker = ctx
                    .spawn_agent(ctx.worker_configs()[0].clone())
                    .await
                    .expect("worker spawn should succeed");
                let manager = ctx
                    .spawn_agent(ctx.manager_config().clone())
                    .await
                    .expect("manager spawn should succeed");

                if let Some(sender) = started_tx
                    .lock()
                    .expect("channel mutex should not be poisoned")
                    .take()
                {
                    sender
                        .send(())
                        .expect("dispatch started notification should send");
                }
                if let Some(receiver) = release_rx
                    .lock()
                    .expect("channel mutex should not be poisoned")
                    .take()
                {
                    receiver.recv().expect("dispatch release should arrive");
                }

                let worker_result = worker.run(ctx.task().to_string()).await;
                let worker_text = worker_result
                    .data
                    .as_ref()
                    .map(|content| content.text.as_str())
                    .expect("worker result should contain text");
                manager.run(worker_text.to_string()).await
            })
        }
    });

    let coordinator =
        SwarmCoordinator::with_hooks(base_config(&["worker-a"]), strategy, harness.factory());
    block_on(coordinator.start()).expect("start should succeed");

    let dispatch_coordinator = coordinator.clone();
    let dispatch = thread::spawn(move || block_on(dispatch_coordinator.dispatch("task one")));
    started_rx
        .recv()
        .expect("dispatch should report that it started");

    let stop_coordinator = coordinator.clone();
    let stop_thread = thread::spawn(move || {
        block_on(stop_coordinator.stop()).expect("stop should succeed");
        stop_done_tx
            .send(())
            .expect("stop completion notification should send");
    });

    thread::sleep(Duration::from_millis(30));
    assert!(stop_done_rx.try_recv().is_err());
    assert!(harness.snapshot().stop_log.is_empty());

    release_tx.send(()).expect("dispatch release should send");

    let dispatch_result = dispatch.join().expect("dispatch thread should join");
    assert_eq!(dispatch_result.status, TaskStatus::Success);
    stop_thread.join().expect("stop thread should join");
    stop_done_rx
        .recv()
        .expect("stop completion notification should arrive");

    let snapshot = harness.snapshot();
    assert_eq!(snapshot.stop_log.len(), 2);

    let state = coordinator.get_state();
    assert_eq!(state.status, SwarmStatus::Idle);
    assert_eq!(state.token_usage.total_tokens, 9);
}

#[test]
fn get_state_preserves_results_and_get_message_bus_is_stable() {
    let harness = TestHarness::new(HashMap::from([
        (
            "worker-a".into(),
            TokenUsage {
                prompt_tokens: 3,
                completion_tokens: 3,
                total_tokens: 6,
            },
        ),
        (
            "manager".into(),
            TokenUsage {
                prompt_tokens: 4,
                completion_tokens: 4,
                total_tokens: 8,
            },
        ),
    ]));

    let strategy: Arc<CoordinatorStrategyFn> = Arc::new(|ctx: CoordinatorDispatchContext| {
        Box::pin(async move {
            let worker = ctx
                .spawn_agent(ctx.worker_configs()[0].clone())
                .await
                .expect("worker spawn should succeed");
            let manager = ctx
                .spawn_agent(ctx.manager_config().clone())
                .await
                .expect("manager spawn should succeed");

            let worker_result = worker.run(ctx.task().to_string()).await;
            let worker_text = worker_result
                .data
                .as_ref()
                .map(|content| content.text.as_str())
                .expect("worker result should contain text");

            manager.run(format!("summary {worker_text}")).await
        })
    });

    let coordinator =
        SwarmCoordinator::with_hooks(base_config(&["worker-a"]), strategy, harness.factory());

    let bus = coordinator.get_message_bus();
    assert!(Arc::ptr_eq(&bus, &coordinator.get_message_bus()));

    block_on(coordinator.start()).expect("start should succeed");
    let first = block_on(coordinator.dispatch("task one"));
    let second = block_on(coordinator.dispatch("task two"));
    assert_eq!(first.status, TaskStatus::Success);
    assert_eq!(second.status, TaskStatus::Success);

    let state = coordinator.get_state();
    assert_eq!(state.results.len(), 2);
    assert_eq!(state.results[0].status, TaskStatus::Success);
    assert_eq!(state.results[1].status, TaskStatus::Success);
    assert_eq!(state.token_usage.total_tokens, 6);

    block_on(coordinator.stop()).expect("stop should succeed");
    assert_eq!(coordinator.get_state().token_usage.total_tokens, 6);
}

#[test]
fn dispatch_injects_runtime_managed_send_and_broadcast_hooks() {
    let harness = TestHarness::new(HashMap::from([
        (
            "worker-a".into(),
            TokenUsage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
            },
        ),
        (
            "manager".into(),
            TokenUsage {
                prompt_tokens: 2,
                completion_tokens: 2,
                total_tokens: 4,
            },
        ),
    ]));

    let strategy: Arc<CoordinatorStrategyFn> = Arc::new(|ctx: CoordinatorDispatchContext| {
        Box::pin(async move {
            let manager = ctx
                .spawn_agent(ctx.manager_config().clone())
                .await
                .expect("manager spawn should succeed");
            let worker = ctx
                .spawn_agent(ctx.worker_configs()[0].clone())
                .await
                .expect("worker spawn should succeed");

            let broadcast_result = manager.run("broadcast:team update".into()).await;
            assert_eq!(broadcast_result.status, TaskStatus::Success);

            let send_result = worker
                .run(format!("send:{}:worker reply", manager.id))
                .await;
            assert_eq!(send_result.status, TaskStatus::Success);

            let bus = ctx.message_bus();
            let bus = bus
                .lock()
                .expect("message bus mutex should not be poisoned");
            let manager_inbox = bus.get_messages(&manager.id);
            let worker_inbox = bus.get_messages(&worker.id);
            let all_messages = bus.get_all_messages();

            assert_eq!(manager_inbox.len(), 1);
            assert_eq!(manager_inbox[0].from, worker.id);
            assert_eq!(manager_inbox[0].content.text, "worker reply");
            assert_eq!(worker_inbox.len(), 1);
            assert_eq!(worker_inbox[0].from, manager.id);
            assert_eq!(worker_inbox[0].to, "broadcast");
            assert_eq!(worker_inbox[0].content.text, "team update");
            assert_eq!(all_messages.len(), 2);

            TaskResult::success(text_content("hooks exercised"), 1)
        })
    });

    let coordinator =
        SwarmCoordinator::with_hooks(base_config(&["worker-a"]), strategy, harness.factory());

    block_on(coordinator.start()).expect("start should succeed");
    let result = block_on(coordinator.dispatch("exercise hooks"));

    assert_eq!(result.status, TaskStatus::Success);
}

#[test]
fn spawn_agent_enforces_max_concurrent_agents_atomically_under_parallel_spawn() {
    let started = Arc::new(Mutex::new(Vec::<String>::new()));
    let factory: Arc<CoordinatorAgentFactoryFn> = Arc::new({
        let started = started.clone();
        move |context: CoordinatorAgentFactoryContext| {
            let started = started.clone();
            Box::pin(async move {
                started
                    .lock()
                    .expect("started mutex should not be poisoned")
                    .push(context.config.name.clone());
                thread::sleep(Duration::from_millis(50));

                Ok(CoordinatorAgentShell {
                    run: Arc::new(move |input: String| {
                        Box::pin(async move { TaskResult::success(text_content(&input), 1) })
                    }),
                    token_usage: Arc::new(TokenUsage::default),
                    clear_task_state: Arc::new(|| {}),
                    stop: Arc::new(|| Box::pin(async {})),
                })
            })
        }
    });

    let strategy: Arc<CoordinatorStrategyFn> = Arc::new(|ctx: CoordinatorDispatchContext| {
        Box::pin(async move {
            let first_ctx = ctx.clone();
            let first_config = ctx.worker_configs()[0].clone();
            let first = thread::spawn(move || block_on(first_ctx.spawn_agent(first_config)));

            let second_ctx = ctx.clone();
            let second_config = ctx.worker_configs()[1].clone();
            let second = thread::spawn(move || block_on(second_ctx.spawn_agent(second_config)));

            let first_result = first.join().expect("first spawn thread should join");
            let second_result = second.join().expect("second spawn thread should join");

            let errors = [first_result.as_ref().err(), second_result.as_ref().err()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            let successes = [first_result.as_ref().ok(), second_result.as_ref().ok()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();

            assert_eq!(successes.len(), 1);
            assert_eq!(errors.len(), 1);
            assert!(
                errors[0].contains("Max concurrent agents (1) reached"),
                "unexpected error: {}",
                errors[0]
            );

            TaskResult::success(text_content("atomic limit enforced"), 1)
        })
    });

    let coordinator = SwarmCoordinator::with_hooks(
        SwarmConfig {
            max_concurrent_agents: Some(1),
            ..base_config(&["worker-a", "worker-b"])
        },
        strategy,
        factory,
    );

    let result = block_on(coordinator.dispatch("parallel spawn"));
    assert_eq!(result.status, TaskStatus::Success);
    assert_eq!(
        started
            .lock()
            .expect("started mutex should not be poisoned")
            .len(),
        1
    );
    assert!(coordinator.get_state().agent_ids.is_empty());
}

#[test]
fn start_rolls_back_workers_created_before_a_later_spawn_failure() {
    let harness = TestHarness::new(HashMap::from([(
        "worker-a".into(),
        TokenUsage {
            prompt_tokens: 2,
            completion_tokens: 2,
            total_tokens: 4,
        },
    )]));
    let failing_factory: Arc<CoordinatorAgentFactoryFn> = Arc::new({
        let base_factory = harness.factory();
        move |context: CoordinatorAgentFactoryContext| {
            if context.config.name == "worker-b" {
                return Box::pin(async { Err("worker-b failed to start".into()) });
            }
            base_factory(context)
        }
    });

    let coordinator = SwarmCoordinator::with_hooks(
        base_config(&["worker-a", "worker-b"]),
        Arc::new(|_| Box::pin(async { TaskResult::success(text_content("unused"), 0) })),
        failing_factory,
    );

    let error = block_on(coordinator.start()).expect_err("start should fail");
    assert_eq!(error, "worker-b failed to start");

    let snapshot = harness.snapshot();
    assert_eq!(snapshot.spawn_counts.get("worker-a"), Some(&1));
    assert_eq!(snapshot.spawn_counts.get("worker-b"), None);
    assert_eq!(snapshot.stop_log.len(), 1);

    let state = coordinator.get_state();
    assert!(state.agent_ids.is_empty());
    assert!(state.results.is_empty());
    assert_eq!(state.token_usage.total_tokens, 0);
    assert!(coordinator
        .get_message_bus()
        .lock()
        .expect("message bus mutex should not be poisoned")
        .get_all_messages()
        .is_empty());

    block_on(coordinator.stop()).expect("stop after rollback should succeed");
    assert_eq!(harness.snapshot().stop_log.len(), 1);
}

#[test]
fn get_state_refreshes_live_token_usage_for_persistent_workers() {
    let harness = TestHarness::new(HashMap::from([(
        "worker-a".into(),
        TokenUsage {
            prompt_tokens: 1,
            completion_tokens: 2,
            total_tokens: 3,
        },
    )]));
    let coordinator = SwarmCoordinator::with_hooks(
        base_config(&["worker-a"]),
        Arc::new(|_| Box::pin(async { TaskResult::success(text_content("unused"), 0) })),
        harness.factory(),
    );

    block_on(coordinator.start()).expect("start should succeed");
    assert_eq!(coordinator.get_state().token_usage.total_tokens, 3);

    harness.set_tokens(
        "worker-a",
        TokenUsage {
            prompt_tokens: 5,
            completion_tokens: 6,
            total_tokens: 11,
        },
    );

    let state = coordinator.get_state();
    assert_eq!(state.token_usage.prompt_tokens, 5);
    assert_eq!(state.token_usage.completion_tokens, 6);
    assert_eq!(state.token_usage.total_tokens, 11);
}

#[test]
fn dispatch_releases_agents_lock_before_clear_task_state_hooks() {
    let hook_gate = Arc::new(Mutex::new(()));
    let (hook_entered_tx, hook_entered_rx) = mpsc::channel();
    let hook_entered_tx = Arc::new(Mutex::new(Some(hook_entered_tx)));

    let factory: Arc<CoordinatorAgentFactoryFn> = Arc::new({
        let hook_gate = hook_gate.clone();
        let hook_entered_tx = hook_entered_tx.clone();
        move |_context: CoordinatorAgentFactoryContext| {
            let hook_gate = hook_gate.clone();
            let hook_entered_tx = hook_entered_tx.clone();
            Box::pin(async move {
                Ok(CoordinatorAgentShell {
                    run: Arc::new(|input: String| {
                        Box::pin(async move { TaskResult::success(text_content(&input), 1) })
                    }),
                    token_usage: Arc::new(TokenUsage::default),
                    clear_task_state: Arc::new(move || {
                        if let Some(sender) = hook_entered_tx
                            .lock()
                            .expect("channel mutex should not be poisoned")
                            .take()
                        {
                            sender.send(()).expect("hook entered signal should send");
                        }
                        let _guard = hook_gate
                            .lock()
                            .expect("hook gate mutex should not be poisoned");
                    }),
                    stop: Arc::new(|| Box::pin(async {})),
                })
            })
        }
    });

    let coordinator = SwarmCoordinator::with_hooks(
        base_config(&["worker-a"]),
        Arc::new(|_| Box::pin(async { TaskResult::success(text_content("ok"), 0) })),
        factory,
    );

    block_on(coordinator.start()).expect("start should succeed");

    let held_gate = hook_gate
        .lock()
        .expect("hook gate mutex should not be poisoned");
    let dispatch_coordinator = coordinator.clone();
    let dispatch = thread::spawn(move || block_on(dispatch_coordinator.dispatch("task")));

    hook_entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("clear_task_state hook should be reached");

    let (state_done_tx, state_done_rx) = mpsc::channel();
    let state_coordinator = coordinator.clone();
    let state_thread = thread::spawn(move || {
        let state = state_coordinator.get_state();
        state_done_tx
            .send(state.status)
            .expect("state completion signal should send");
    });

    assert_eq!(
        state_done_rx
            .recv_timeout(Duration::from_millis(200))
            .expect("get_state should not be blocked by clear_task_state"),
        SwarmStatus::Running
    );

    drop(held_gate);

    let result = dispatch.join().expect("dispatch thread should join");
    assert_eq!(result.status, TaskStatus::Success);
    state_thread.join().expect("state thread should join");
}

#[test]
fn get_state_releases_agents_lock_before_token_usage_hooks() {
    let hook_gate = Arc::new(Mutex::new(()));
    let (hook_entered_tx, hook_entered_rx) = mpsc::channel();
    let hook_entered_tx = Arc::new(Mutex::new(Some(hook_entered_tx)));

    let factory: Arc<CoordinatorAgentFactoryFn> = Arc::new({
        let hook_gate = hook_gate.clone();
        let hook_entered_tx = hook_entered_tx.clone();
        move |_context: CoordinatorAgentFactoryContext| {
            let hook_gate = hook_gate.clone();
            let hook_entered_tx = hook_entered_tx.clone();
            Box::pin(async move {
                Ok(CoordinatorAgentShell {
                    run: Arc::new(|input: String| {
                        Box::pin(async move { TaskResult::success(text_content(&input), 1) })
                    }),
                    token_usage: Arc::new(move || {
                        if let Some(sender) = hook_entered_tx
                            .lock()
                            .expect("channel mutex should not be poisoned")
                            .take()
                        {
                            sender.send(()).expect("hook entered signal should send");
                        }
                        let _guard = hook_gate
                            .lock()
                            .expect("hook gate mutex should not be poisoned");
                        TokenUsage::default()
                    }),
                    clear_task_state: Arc::new(|| {}),
                    stop: Arc::new(|| Box::pin(async {})),
                })
            })
        }
    });

    let coordinator = SwarmCoordinator::with_hooks(
        base_config(&["worker-a"]),
        Arc::new(|_| Box::pin(async { TaskResult::success(text_content("ok"), 0) })),
        factory,
    );

    block_on(coordinator.start()).expect("start should succeed");

    let held_gate = hook_gate
        .lock()
        .expect("hook gate mutex should not be poisoned");
    let state_coordinator = coordinator.clone();
    let state_thread = thread::spawn(move || state_coordinator.get_state());

    hook_entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("token_usage hook should be reached");

    let (stop_done_tx, stop_done_rx) = mpsc::channel();
    let stop_coordinator = coordinator.clone();
    let stop_thread = thread::spawn(move || {
        block_on(stop_coordinator.stop()).expect("stop should succeed");
        stop_done_tx
            .send(())
            .expect("stop completion signal should send");
    });

    stop_done_rx
        .recv_timeout(Duration::from_millis(200))
        .expect("stop should not be blocked by token_usage");

    drop(held_gate);

    state_thread.join().expect("state thread should join");
    stop_thread.join().expect("stop thread should join");
}

#[test]
fn dispatch_cleanup_prunes_agent_ids_and_invalidates_removed_refs() {
    let harness = TestHarness::new(HashMap::from([
        (
            "worker-a".into(),
            TokenUsage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
            },
        ),
        (
            "manager".into(),
            TokenUsage {
                prompt_tokens: 2,
                completion_tokens: 2,
                total_tokens: 4,
            },
        ),
    ]));
    let saved_manager = Arc::new(Mutex::new(None::<CoordinatorAgentRef>));

    let strategy: Arc<CoordinatorStrategyFn> = Arc::new({
        let saved_manager = saved_manager.clone();
        move |ctx: CoordinatorDispatchContext| {
            let saved_manager = saved_manager.clone();
            Box::pin(async move {
                let worker = ctx
                    .spawn_agent(ctx.worker_configs()[0].clone())
                    .await
                    .expect("worker spawn should succeed");
                let manager = ctx
                    .spawn_agent(ctx.manager_config().clone())
                    .await
                    .expect("manager spawn should succeed");
                saved_manager
                    .lock()
                    .expect("saved manager mutex should not be poisoned")
                    .replace(manager.clone());

                let worker_result = worker.run(ctx.task().to_string()).await;
                let worker_text = worker_result
                    .data
                    .as_ref()
                    .map(|content| content.text.as_str())
                    .expect("worker result should contain text");
                manager.run(worker_text.to_string()).await
            })
        }
    });

    let coordinator =
        SwarmCoordinator::with_hooks(base_config(&["worker-a"]), strategy, harness.factory());

    block_on(coordinator.start()).expect("start should succeed");
    let pooled_agent_ids = coordinator.get_state().agent_ids;
    let result = block_on(coordinator.dispatch("task one"));
    assert_eq!(result.status, TaskStatus::Success);

    let state = coordinator.get_state();
    assert_eq!(state.agent_ids, pooled_agent_ids);

    let manager = saved_manager
        .lock()
        .expect("saved manager mutex should not be poisoned")
        .clone()
        .expect("manager ref should be captured");
    let expected_error = format!("Coordinator agent {} is no longer active", manager.id);
    let stale_result = block_on(manager.run("should fail".into()));
    assert_eq!(stale_result.status, TaskStatus::Error);
    assert_eq!(stale_result.error.as_deref(), Some(expected_error.as_str()));
}

#[test]
fn stop_prunes_agent_ids_and_invalidates_pooled_refs() {
    let harness = TestHarness::new(HashMap::from([(
        "worker-a".into(),
        TokenUsage {
            prompt_tokens: 1,
            completion_tokens: 1,
            total_tokens: 2,
        },
    )]));
    let saved_worker = Arc::new(Mutex::new(None::<CoordinatorAgentRef>));

    let strategy: Arc<CoordinatorStrategyFn> = Arc::new({
        let saved_worker = saved_worker.clone();
        move |ctx: CoordinatorDispatchContext| {
            let saved_worker = saved_worker.clone();
            Box::pin(async move {
                let worker = ctx
                    .spawn_agent(ctx.worker_configs()[0].clone())
                    .await
                    .expect("worker spawn should succeed");
                saved_worker
                    .lock()
                    .expect("saved worker mutex should not be poisoned")
                    .replace(worker.clone());
                worker.run(ctx.task().to_string()).await
            })
        }
    });

    let coordinator =
        SwarmCoordinator::with_hooks(base_config(&["worker-a"]), strategy, harness.factory());

    block_on(coordinator.start()).expect("start should succeed");
    let dispatch_result = block_on(coordinator.dispatch("task one"));
    assert_eq!(dispatch_result.status, TaskStatus::Success);
    assert_eq!(coordinator.get_state().agent_ids.len(), 1);

    block_on(coordinator.stop()).expect("stop should succeed");

    let state = coordinator.get_state();
    assert!(state.agent_ids.is_empty());

    let worker = saved_worker
        .lock()
        .expect("saved worker mutex should not be poisoned")
        .clone()
        .expect("worker ref should be captured");
    let expected_error = format!("Coordinator agent {} is no longer active", worker.id);
    let stale_result = block_on(worker.run("should fail".into()));
    assert_eq!(stale_result.status, TaskStatus::Error);
    assert_eq!(stale_result.error.as_deref(), Some(expected_error.as_str()));
}

#[test]
fn stale_send_and_broadcast_hooks_cannot_mutate_message_bus_after_agent_cleanup() {
    let saved_manager_send = Arc::new(Mutex::new(None));
    let saved_manager_broadcast = Arc::new(Mutex::new(None));
    let saved_manager_id = Arc::new(Mutex::new(None::<String>));

    let factory: Arc<CoordinatorAgentFactoryFn> = Arc::new({
        let saved_manager_send = saved_manager_send.clone();
        let saved_manager_broadcast = saved_manager_broadcast.clone();
        let saved_manager_id = saved_manager_id.clone();
        move |context: CoordinatorAgentFactoryContext| {
            let saved_manager_send = saved_manager_send.clone();
            let saved_manager_broadcast = saved_manager_broadcast.clone();
            let saved_manager_id = saved_manager_id.clone();
            Box::pin(async move {
                if context.config.name == "manager" {
                    saved_manager_send
                        .lock()
                        .expect("saved send mutex should not be poisoned")
                        .replace(context.send.clone());
                    saved_manager_broadcast
                        .lock()
                        .expect("saved broadcast mutex should not be poisoned")
                        .replace(context.broadcast.clone());
                    saved_manager_id
                        .lock()
                        .expect("saved manager id mutex should not be poisoned")
                        .replace(context.agent_id.clone());
                }

                Ok(CoordinatorAgentShell {
                    run: Arc::new(|input: String| {
                        Box::pin(async move { TaskResult::success(text_content(&input), 1) })
                    }),
                    token_usage: Arc::new(TokenUsage::default),
                    clear_task_state: Arc::new(|| {}),
                    stop: Arc::new(|| Box::pin(async {})),
                })
            })
        }
    });

    let strategy: Arc<CoordinatorStrategyFn> = Arc::new(|ctx: CoordinatorDispatchContext| {
        Box::pin(async move {
            let manager = ctx
                .spawn_agent(ctx.manager_config().clone())
                .await
                .expect("manager spawn should succeed");
            manager.run(ctx.task().to_string()).await
        })
    });

    let coordinator = SwarmCoordinator::with_hooks(base_config(&["worker-a"]), strategy, factory);

    block_on(coordinator.start()).expect("start should succeed");
    let worker_id = coordinator.get_state().agent_ids[0].clone();

    let dispatch_result = block_on(coordinator.dispatch("task one"));
    assert_eq!(dispatch_result.status, TaskStatus::Success);

    let manager_id = saved_manager_id
        .lock()
        .expect("saved manager id mutex should not be poisoned")
        .clone()
        .expect("manager id should be captured");
    let expected_error = format!("Coordinator agent {manager_id} is no longer active");
    let send = saved_manager_send
        .lock()
        .expect("saved send mutex should not be poisoned")
        .clone()
        .expect("manager send hook should be captured");
    let broadcast = saved_manager_broadcast
        .lock()
        .expect("saved broadcast mutex should not be poisoned")
        .clone()
        .expect("manager broadcast hook should be captured");

    let bus = coordinator.get_message_bus();
    assert!(bus
        .lock()
        .expect("message bus mutex should not be poisoned")
        .get_all_messages()
        .is_empty());

    let send_error = block_on(send(worker_id.clone(), text_content("late direct message")))
        .expect_err("stale send hook should fail");
    assert_eq!(send_error, expected_error);

    let broadcast_error = block_on(broadcast(text_content("late broadcast")))
        .expect_err("stale broadcast hook should fail");
    assert_eq!(broadcast_error, expected_error);

    let bus = bus
        .lock()
        .expect("message bus mutex should not be poisoned");
    assert!(bus.get_all_messages().is_empty());
    assert!(bus.get_messages(&worker_id).is_empty());
}

#[test]
fn round_robin_strategy_cycles_agents_and_aggregates_history() {
    let spawn_log = Arc::new(Mutex::new(Vec::<String>::new()));
    let run_log = Arc::new(Mutex::new(Vec::<String>::new()));

    let factory: Arc<CoordinatorAgentFactoryFn> = Arc::new({
        let spawn_log = Arc::clone(&spawn_log);
        let run_log = Arc::clone(&run_log);
        move |context: CoordinatorAgentFactoryContext| {
            let spawn_log = Arc::clone(&spawn_log);
            let run_log = Arc::clone(&run_log);
            Box::pin(async move {
                spawn_log
                    .lock()
                    .expect("spawn log mutex should not be poisoned")
                    .push(context.config.name.clone());

                let agent_name = context.config.name.clone();
                let run_log_for_agent = Arc::clone(&run_log);
                Ok(CoordinatorAgentShell {
                    run: Arc::new(move |input: String| {
                        let run_log = Arc::clone(&run_log_for_agent);
                        let agent_name = agent_name.clone();
                        Box::pin(async move {
                            run_log
                                .lock()
                                .expect("run log mutex should not be poisoned")
                                .push(format!("{agent_name}:{input}"));
                            TaskResult::success(
                                text_content(&format!("{agent_name} contribution")),
                                1,
                            )
                        })
                    }),
                    token_usage: Arc::new(TokenUsage::default),
                    clear_task_state: Arc::new(|| {}),
                    stop: Arc::new(|| Box::pin(async {})),
                })
            })
        }
    });

    let coordinator = SwarmCoordinator::with_hooks(
        round_robin_config(&["worker-a", "worker-b"]),
        resolve_strategy(SwarmStrategy::RoundRobin),
        factory,
    );

    let result = block_on(coordinator.dispatch("The actual task text"));

    assert_eq!(result.status, TaskStatus::Success);
    assert_eq!(
        result.data.as_ref().map(|content| content.text.as_str()),
        Some(concat!(
            "[manager]: manager contribution\n\n",
            "[worker-a]: worker-a contribution\n\n",
            "[worker-b]: worker-b contribution\n\n",
            "[manager]: manager contribution"
        ))
    );

    let metadata = result
        .data
        .as_ref()
        .and_then(|content| content.metadata.as_ref())
        .expect("round-robin result should include history metadata");
    let history = metadata
        .get("history")
        .expect("history metadata should exist");
    match history {
        DataValue::Array(entries) => {
            assert_eq!(entries.len(), 4);
            let first = data_value_as_object(&entries[0]);
            assert_eq!(
                data_value_as_str(first.get("speaker").expect("speaker should exist")),
                "manager"
            );
            assert_eq!(
                data_value_as_str(first.get("content").expect("content should exist")),
                "manager contribution"
            );
        }
        other => panic!("unexpected history metadata: {other:?}"),
    }

    assert_eq!(
        run_log
            .lock()
            .expect("run log mutex should not be poisoned")
            .as_slice(),
        [
            "manager:The actual task text",
            "worker-a:Continue working on this task: The actual task text\n\nIt's your turn to contribute.\n\nConversation so far:\n[manager]: manager contribution",
            "worker-b:Continue working on this task: The actual task text\n\nIt's your turn to contribute.\n\nConversation so far:\n[manager]: manager contribution\n[worker-a]: worker-a contribution",
            "manager:Continue working on this task: The actual task text\n\nIt's your turn to contribute.\n\nConversation so far:\n[manager]: manager contribution\n[worker-a]: worker-a contribution\n[worker-b]: worker-b contribution",
        ]
    );

    assert_eq!(
        spawn_log
            .lock()
            .expect("spawn log mutex should not be poisoned")
            .as_slice(),
        ["manager", "worker-a", "worker-b"]
    );
}
