use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::Duration;

use anima_core::{AgentConfig, Content, TaskResult, TaskStatus, TokenUsage};
use anima_swarm::{
    CoordinatorAgentFactoryFn, CoordinatorAgentRef, CoordinatorAgentShell,
    CoordinatorDispatchContext, CoordinatorStrategyFn, SwarmConfig, SwarmCoordinator, SwarmStatus,
    SwarmStrategy,
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
    tokens: Arc<HashMap<String, TokenUsage>>,
}

impl TestHarness {
    fn new(tokens: HashMap<String, TokenUsage>) -> Self {
        Self {
            state: Arc::new(Mutex::new(TestHarnessState::default())),
            tokens: Arc::new(tokens),
        }
    }

    fn factory(&self) -> Arc<CoordinatorAgentFactoryFn> {
        let shared = self.state.clone();
        let tokens = self.tokens.clone();

        Arc::new(move |config: AgentConfig| {
            let shared = shared.clone();
            let tokens = tokens.clone();

            Box::pin(async move {
                let (agent_id, token_usage) = {
                    let mut state = shared
                        .lock()
                        .expect("test harness state mutex should not be poisoned");
                    let count = *state
                        .spawn_counts
                        .entry(config.name.clone())
                        .and_modify(|value| *value += 1)
                        .or_insert(1);
                    state.spawn_log.push(config.name.clone());

                    let agent_id = format!("{}-{}", config.name, count);
                    let token_usage = tokens
                        .get(&config.name)
                        .cloned()
                        .unwrap_or_else(TokenUsage::default);
                    (agent_id, token_usage)
                };

                let run_state = shared.clone();
                let run_id = agent_id.clone();
                let stop_state = shared.clone();
                let stop_id = agent_id.clone();
                let clear_state = shared.clone();
                let clear_id = agent_id.clone();

                Ok(CoordinatorAgentShell {
                    agent: CoordinatorAgentRef::new(agent_id.clone(), move |input| {
                        let run_state = run_state.clone();
                        let run_id = run_id.clone();
                        Box::pin(async move {
                            run_state
                                .lock()
                                .expect("test harness state mutex should not be poisoned")
                                .run_log
                                .push(format!("{run_id}:{input}"));
                            TaskResult::success(
                                text_content(&format!("{run_id} handled {input}")),
                                1,
                            )
                        })
                    }),
                    token_usage: Arc::new(move || token_usage.clone()),
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
    assert_eq!(state.token_usage.total_tokens, 21);
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

    coordinator
        .get_message_bus()
        .lock()
        .expect("message bus mutex should not be poisoned")
        .send("manager", "worker-a-1", text_content("stale"));

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
    assert_eq!(state.token_usage.total_tokens, 14);

    block_on(coordinator.stop()).expect("stop should succeed");
    assert_eq!(coordinator.get_state().token_usage.total_tokens, 14);
}
