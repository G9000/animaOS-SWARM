use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex as AsyncMutex;

use anima_core::{AgentConfig, Content, TaskResult, TaskStatus, TokenUsage};

use crate::strategies::resolve_strategy;
use crate::{MessageBus, SwarmConfig, SwarmState, SwarmStatus};

static NEXT_COORDINATOR_ID: AtomicU64 = AtomicU64::new(0);
static NEXT_AGENT_ID: AtomicU64 = AtomicU64::new(0);

pub type CoordinatorFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

pub type CoordinatorStrategyFn =
    dyn Fn(CoordinatorDispatchContext) -> CoordinatorFuture<TaskResult<Content>> + Send + Sync;

pub type CoordinatorSendFn =
    dyn Fn(String, Content) -> CoordinatorFuture<Result<(), String>> + Send + Sync;

pub type CoordinatorBroadcastFn =
    dyn Fn(Content) -> CoordinatorFuture<Result<(), String>> + Send + Sync;

pub type CoordinatorAgentFactoryFn = dyn Fn(CoordinatorAgentFactoryContext) -> CoordinatorFuture<Result<CoordinatorAgentShell, String>>
    + Send
    + Sync;

#[derive(Clone)]
pub struct CoordinatorAgentFactoryContext {
    pub config: AgentConfig,
    pub agent_id: String,
    pub send: Arc<CoordinatorSendFn>,
    pub broadcast: Arc<CoordinatorBroadcastFn>,
}

#[derive(Clone)]
pub struct CoordinatorAgentRef {
    pub id: String,
    liveness: Arc<CoordinatorAgentLiveness>,
    run: Arc<dyn Fn(String) -> CoordinatorFuture<TaskResult<Content>> + Send + Sync>,
}

impl CoordinatorAgentRef {
    pub fn new<F>(id: impl Into<String>, run: F) -> Self
    where
        F: Fn(String) -> CoordinatorFuture<TaskResult<Content>> + Send + Sync + 'static,
    {
        let id = id.into();
        Self {
            id,
            liveness: Arc::new(CoordinatorAgentLiveness::default()),
            run: Arc::new(run),
        }
    }

    fn with_liveness(
        id: impl Into<String>,
        run: Arc<dyn Fn(String) -> CoordinatorFuture<TaskResult<Content>> + Send + Sync>,
        liveness: Arc<CoordinatorAgentLiveness>,
    ) -> Self {
        Self {
            id: id.into(),
            liveness,
            run,
        }
    }

    pub async fn run(&self, input: String) -> TaskResult<Content> {
        if !self.liveness.is_active() {
            return inactive_agent_result(&self.id);
        }
        (self.run)(input).await
    }
}

#[derive(Clone)]
pub struct CoordinatorAgentShell {
    pub run: Arc<dyn Fn(String) -> CoordinatorFuture<TaskResult<Content>> + Send + Sync>,
    pub token_usage: Arc<dyn Fn() -> TokenUsage + Send + Sync>,
    pub clear_task_state: Arc<dyn Fn() + Send + Sync>,
    pub stop: Arc<dyn Fn() -> CoordinatorFuture<()> + Send + Sync>,
}

#[derive(Clone)]
pub struct CoordinatorDispatchContext {
    task: String,
    manager_config: AgentConfig,
    worker_configs: Vec<AgentConfig>,
    max_turns: usize,
    message_bus: Arc<Mutex<MessageBus>>,
    spawn_agent: Arc<
        dyn Fn(AgentConfig) -> CoordinatorFuture<Result<CoordinatorAgentRef, String>> + Send + Sync,
    >,
}

impl CoordinatorDispatchContext {
    fn new(
        task: String,
        manager_config: AgentConfig,
        worker_configs: Vec<AgentConfig>,
        max_turns: usize,
        message_bus: Arc<Mutex<MessageBus>>,
        spawn_agent: Arc<
            dyn Fn(AgentConfig) -> CoordinatorFuture<Result<CoordinatorAgentRef, String>>
                + Send
                + Sync,
        >,
    ) -> Self {
        Self {
            task,
            manager_config,
            worker_configs,
            max_turns,
            message_bus,
            spawn_agent,
        }
    }

    pub fn task(&self) -> &str {
        &self.task
    }

    pub fn manager_config(&self) -> &AgentConfig {
        &self.manager_config
    }

    pub fn worker_configs(&self) -> &[AgentConfig] {
        &self.worker_configs
    }

    pub fn max_turns(&self) -> usize {
        self.max_turns
    }

    pub fn message_bus(&self) -> Arc<Mutex<MessageBus>> {
        self.message_bus.clone()
    }

    pub async fn spawn_agent(&self, config: AgentConfig) -> Result<CoordinatorAgentRef, String> {
        (self.spawn_agent)(config).await
    }
}

#[derive(Clone)]
pub struct SwarmCoordinator {
    inner: Arc<CoordinatorInner>,
}

struct CoordinatorInner {
    config: SwarmConfig,
    state: Mutex<SwarmState>,
    message_bus: Arc<Mutex<MessageBus>>,
    strategy: Arc<CoordinatorStrategyFn>,
    agent_factory: Arc<CoordinatorAgentFactoryFn>,
    agents: Mutex<HashMap<String, CoordinatorManagedAgent>>,
    admitted_agent_ids: Mutex<HashSet<String>>,
    pool: Mutex<HashMap<String, String>>,
    dispatch_lock: AsyncMutex<()>,
}

struct CoordinatorAgentLiveness {
    active: AtomicBool,
}

impl Default for CoordinatorAgentLiveness {
    fn default() -> Self {
        Self {
            active: AtomicBool::new(true),
        }
    }
}

impl CoordinatorAgentLiveness {
    fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    fn deactivate(&self) {
        self.active.store(false, Ordering::Release);
    }
}

struct CoordinatorManagedAgent {
    shell: CoordinatorAgentShell,
    liveness: Arc<CoordinatorAgentLiveness>,
}

impl SwarmCoordinator {
    pub fn new() -> Self {
        Self::with_config(default_swarm_config())
    }

    pub fn with_config(config: SwarmConfig) -> Self {
        Self::with_hooks(
            config.clone(),
            resolve_strategy(config.strategy),
            default_agent_factory(),
        )
    }

    pub fn with_hooks(
        config: SwarmConfig,
        strategy: Arc<CoordinatorStrategyFn>,
        agent_factory: Arc<CoordinatorAgentFactoryFn>,
    ) -> Self {
        let id = next_id("swarm", &NEXT_COORDINATOR_ID);
        let state = SwarmState {
            id,
            status: SwarmStatus::Idle,
            agent_ids: Vec::new(),
            results: Vec::new(),
            token_usage: TokenUsage::default(),
            started_at: None,
            completed_at: None,
        };

        Self {
            inner: Arc::new(CoordinatorInner {
                config,
                state: Mutex::new(state),
                message_bus: Arc::new(Mutex::new(MessageBus::new())),
                strategy,
                agent_factory,
                agents: Mutex::new(HashMap::new()),
                admitted_agent_ids: Mutex::new(HashSet::new()),
                pool: Mutex::new(HashMap::new()),
                dispatch_lock: AsyncMutex::new(()),
            }),
        }
    }

    pub async fn start(&self) -> Result<(), String> {
        let _dispatch_guard = self.inner.dispatch_lock.lock().await;
        let mut created_workers = Vec::new();

        for config in self.inner.config.workers.clone() {
            if let Some(agent) = self.get_pool_agent(&config.name) {
                let _ = agent;
                continue;
            }

            match self.spawn_new_agent(config.clone()).await {
                Ok(agent) => {
                    self.inner
                        .pool
                        .lock()
                        .expect("coordinator pool mutex should not be poisoned")
                        .insert(config.name.clone(), agent.id.clone());
                    created_workers.push((config.name, agent.id));
                }
                Err(error) => {
                    self.rollback_started_workers(&created_workers).await;
                    return Err(error);
                }
            }
        }

        self.with_state(|state| {
            state.status = SwarmStatus::Idle;
        });

        Ok(())
    }

    pub async fn dispatch(&self, task: impl Into<String>) -> TaskResult<Content> {
        let _dispatch_guard = self.inner.dispatch_lock.lock().await;
        self.run_task(task.into()).await
    }

    pub async fn stop(&self) -> Result<(), String> {
        let _dispatch_guard = self.inner.dispatch_lock.lock().await;

        let agents = {
            let mut agents = self
                .inner
                .agents
                .lock()
                .expect("coordinator agents mutex should not be poisoned");
            let drained = agents
                .drain()
                .map(|(agent_id, agent)| {
                    agent.liveness.deactivate();
                    (agent_id, agent.shell)
                })
                .collect::<Vec<_>>();
            self.inner
                .pool
                .lock()
                .expect("coordinator pool mutex should not be poisoned")
                .clear();
            drained
        };

        {
            let mut bus = self
                .inner
                .message_bus
                .lock()
                .expect("message bus mutex should not be poisoned");
            bus.clear();
        }

        self.clear_live_agent_ids();

        for (agent_id, agent) in agents {
            self.release_agent_slot(&agent_id);
            (agent.stop)().await;
        }

        self.with_state(|state| {
            state.status = SwarmStatus::Idle;
            state.completed_at = Some(now_millis());
        });

        Ok(())
    }

    pub fn get_state(&self) -> SwarmState {
        if self.has_live_agents() {
            self.capture_live_token_usage();
        }

        self.inner
            .state
            .lock()
            .expect("coordinator state mutex should not be poisoned")
            .clone()
    }

    pub fn get_message_bus(&self) -> Arc<Mutex<MessageBus>> {
        self.inner.message_bus.clone()
    }

    async fn run_task(&self, task: String) -> TaskResult<Content> {
        self.with_state(|state| {
            state.status = SwarmStatus::Running;
            state.started_at = Some(now_millis());
            state.completed_at = None;
        });
        self.reset_task_state();

        let spawn_coordinator = self.clone();
        let spawn_agent = Arc::new(move |config: AgentConfig| {
            let spawn_coordinator = spawn_coordinator.clone();
            Box::pin(async move { spawn_coordinator.spawn_for_dispatch(config).await })
                as CoordinatorFuture<Result<CoordinatorAgentRef, String>>
        });
        let max_turns = self
            .inner
            .config
            .max_turns
            .unwrap_or(self.inner.config.workers.len() + 1);
        let context = CoordinatorDispatchContext::new(
            task,
            self.inner.config.manager.clone(),
            self.inner.config.workers.clone(),
            max_turns,
            self.inner.message_bus.clone(),
            spawn_agent,
        );

        let result = (self.inner.strategy)(context).await;
        self.capture_live_token_usage();
        self.stop_ephemeral_agents().await;

        let completed_at = now_millis();
        let result_status = result.status;
        self.with_state(|state| {
            state.completed_at = Some(completed_at);
            state.status = if result_status == TaskStatus::Success {
                SwarmStatus::Idle
            } else {
                SwarmStatus::Failed
            };
            state.results.push(result.clone());
        });

        result
    }

    async fn spawn_for_dispatch(&self, config: AgentConfig) -> Result<CoordinatorAgentRef, String> {
        if let Some(agent) = self.get_pool_agent(&config.name) {
            return Ok(agent);
        }

        self.spawn_new_agent(config).await
    }

    async fn spawn_new_agent(&self, config: AgentConfig) -> Result<CoordinatorAgentRef, String> {
        let agent_id = next_id(&config.name, &NEXT_AGENT_ID);
        let liveness = Arc::new(CoordinatorAgentLiveness::default());
        self.reserve_agent_slot(&agent_id)?;
        let shell = (self.inner.agent_factory)(CoordinatorAgentFactoryContext {
            config,
            agent_id: agent_id.clone(),
            send: self.build_send_hook(&agent_id, liveness.clone()),
            broadcast: self.build_broadcast_hook(&agent_id, liveness.clone()),
        })
        .await
        .inspect_err(|_| self.release_agent_slot(&agent_id))?;
        let agent = CoordinatorAgentRef::with_liveness(
            agent_id.clone(),
            shell.run.clone(),
            liveness.clone(),
        );

        {
            let mut bus = self
                .inner
                .message_bus
                .lock()
                .expect("message bus mutex should not be poisoned");
            bus.register_agent(&agent_id);
        }
        self.inner
            .agents
            .lock()
            .expect("coordinator agents mutex should not be poisoned")
            .insert(
                agent_id.clone(),
                CoordinatorManagedAgent { shell, liveness },
            );
        self.with_state(|state| {
            state.agent_ids.push(agent_id.clone());
        });

        Ok(agent)
    }

    fn get_pool_agent(&self, name: &str) -> Option<CoordinatorAgentRef> {
        let pool_agent_id = self
            .inner
            .pool
            .lock()
            .expect("coordinator pool mutex should not be poisoned")
            .get(name)
            .cloned()?;
        self.inner
            .agents
            .lock()
            .expect("coordinator agents mutex should not be poisoned")
            .get(&pool_agent_id)
            .map(|agent| {
                CoordinatorAgentRef::with_liveness(
                    pool_agent_id,
                    agent.shell.run.clone(),
                    agent.liveness.clone(),
                )
            })
    }

    fn reset_task_state(&self) {
        self.inner
            .message_bus
            .lock()
            .expect("message bus mutex should not be poisoned")
            .clear_inboxes();

        let clear_hooks = self
            .inner
            .agents
            .lock()
            .expect("coordinator agents mutex should not be poisoned")
            .values()
            .map(|agent| agent.shell.clear_task_state.clone())
            .collect::<Vec<_>>();

        for clear_task_state in clear_hooks {
            clear_task_state();
        }
    }

    async fn stop_ephemeral_agents(&self) {
        let pooled_agent_ids = self
            .inner
            .pool
            .lock()
            .expect("coordinator pool mutex should not be poisoned")
            .values()
            .cloned()
            .collect::<HashSet<_>>();

        let ephemeral = {
            let mut agents = self
                .inner
                .agents
                .lock()
                .expect("coordinator agents mutex should not be poisoned");
            let ephemeral_ids = agents
                .keys()
                .filter(|agent_id| !pooled_agent_ids.contains(*agent_id))
                .cloned()
                .collect::<Vec<_>>();
            ephemeral_ids
                .into_iter()
                .filter_map(|agent_id| {
                    agents.remove(&agent_id).map(|agent| {
                        agent.liveness.deactivate();
                        (agent_id, agent.shell)
                    })
                })
                .collect::<Vec<_>>()
        };

        self.remove_live_agent_ids(ephemeral.iter().map(|(agent_id, _)| agent_id.as_str()));

        for (agent_id, agent) in ephemeral {
            self.inner
                .message_bus
                .lock()
                .expect("message bus mutex should not be poisoned")
                .unregister_agent(&agent_id);
            self.release_agent_slot(&agent_id);
            (agent.stop)().await;
        }
    }

    fn capture_live_token_usage(&self) {
        let token_hooks = self
            .inner
            .agents
            .lock()
            .expect("coordinator agents mutex should not be poisoned")
            .values()
            .map(|agent| agent.shell.token_usage.clone())
            .collect::<Vec<_>>();

        if token_hooks.is_empty() {
            return;
        }

        let mut token_usage = TokenUsage::default();
        for token_hook in token_hooks {
            let snapshot = token_hook();
            token_usage.prompt_tokens += snapshot.prompt_tokens;
            token_usage.completion_tokens += snapshot.completion_tokens;
            token_usage.total_tokens += snapshot.total_tokens;
        }

        self.with_state(|state| {
            state.token_usage = token_usage;
        });
    }

    fn has_live_agents(&self) -> bool {
        !self
            .inner
            .agents
            .lock()
            .expect("coordinator agents mutex should not be poisoned")
            .is_empty()
    }

    fn reserve_agent_slot(&self, agent_id: &str) -> Result<(), String> {
        let max_agents = self
            .inner
            .config
            .max_concurrent_agents
            .unwrap_or(usize::MAX);
        let mut admitted = self
            .inner
            .admitted_agent_ids
            .lock()
            .expect("admission mutex should not be poisoned");
        if admitted.len() >= max_agents {
            return Err(format!("Max concurrent agents ({max_agents}) reached"));
        }
        admitted.insert(agent_id.to_string());
        Ok(())
    }

    fn release_agent_slot(&self, agent_id: &str) {
        self.inner
            .admitted_agent_ids
            .lock()
            .expect("admission mutex should not be poisoned")
            .remove(agent_id);
    }

    fn build_send_hook(
        &self,
        from_agent_id: &str,
        liveness: Arc<CoordinatorAgentLiveness>,
    ) -> Arc<CoordinatorSendFn> {
        let message_bus = self.inner.message_bus.clone();
        let from_agent_id = from_agent_id.to_string();
        Arc::new(move |to_agent_id: String, content: Content| {
            let message_bus = message_bus.clone();
            let from_agent_id = from_agent_id.clone();
            let liveness = liveness.clone();
            Box::pin(async move {
                if !liveness.is_active() {
                    return Err(inactive_agent_error(&from_agent_id));
                }
                message_bus
                    .lock()
                    .expect("message bus mutex should not be poisoned")
                    .send(&from_agent_id, &to_agent_id, content);
                Ok(())
            })
        })
    }

    fn build_broadcast_hook(
        &self,
        from_agent_id: &str,
        liveness: Arc<CoordinatorAgentLiveness>,
    ) -> Arc<CoordinatorBroadcastFn> {
        let message_bus = self.inner.message_bus.clone();
        let from_agent_id = from_agent_id.to_string();
        Arc::new(move |content: Content| {
            let message_bus = message_bus.clone();
            let from_agent_id = from_agent_id.clone();
            let liveness = liveness.clone();
            Box::pin(async move {
                if !liveness.is_active() {
                    return Err(inactive_agent_error(&from_agent_id));
                }
                message_bus
                    .lock()
                    .expect("message bus mutex should not be poisoned")
                    .broadcast(&from_agent_id, content);
                Ok(())
            })
        })
    }

    fn with_state(&self, update: impl FnOnce(&mut SwarmState)) {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("coordinator state mutex should not be poisoned");
        update(&mut state);
    }

    async fn rollback_started_workers(&self, created_workers: &[(String, String)]) {
        if created_workers.is_empty() {
            return;
        }

        let created_names = created_workers
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<HashSet<_>>();
        let created_ids = created_workers
            .iter()
            .map(|(_, agent_id)| agent_id.clone())
            .collect::<HashSet<_>>();

        self.inner
            .pool
            .lock()
            .expect("coordinator pool mutex should not be poisoned")
            .retain(|name, _| !created_names.contains(name));

        let removed_agents = {
            let mut agents = self
                .inner
                .agents
                .lock()
                .expect("coordinator agents mutex should not be poisoned");
            created_ids
                .iter()
                .filter_map(|agent_id| {
                    agents.remove(agent_id).map(|agent| {
                        agent.liveness.deactivate();
                        (agent_id.clone(), agent.shell)
                    })
                })
                .collect::<Vec<_>>()
        };

        {
            let mut bus = self
                .inner
                .message_bus
                .lock()
                .expect("message bus mutex should not be poisoned");
            for agent_id in &created_ids {
                bus.unregister_agent(agent_id);
            }
        }

        let should_reset_tokens = {
            let agents = self
                .inner
                .agents
                .lock()
                .expect("coordinator agents mutex should not be poisoned");
            agents.is_empty()
        };

        self.with_state(|state| {
            state
                .agent_ids
                .retain(|agent_id| !created_ids.contains(agent_id));
            state.status = SwarmStatus::Idle;
            if should_reset_tokens {
                state.token_usage = TokenUsage::default();
            }
        });

        for (agent_id, agent) in removed_agents {
            self.release_agent_slot(&agent_id);
            (agent.stop)().await;
        }
    }

    fn remove_live_agent_ids<'a>(&self, removed_ids: impl IntoIterator<Item = &'a str>) {
        let removed_ids = removed_ids
            .into_iter()
            .map(str::to_string)
            .collect::<HashSet<_>>();
        if removed_ids.is_empty() {
            return;
        }

        self.with_state(|state| {
            state
                .agent_ids
                .retain(|agent_id| !removed_ids.contains(agent_id));
        });
    }

    fn clear_live_agent_ids(&self) {
        self.with_state(|state| {
            state.agent_ids.clear();
        });
    }
}

fn default_swarm_config() -> SwarmConfig {
    SwarmConfig {
        strategy: crate::SwarmStrategy::Supervisor,
        manager: AgentConfig {
            name: "manager".into(),
            model: "unconfigured".into(),
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
            settings: None,
        },
        workers: Vec::new(),
        max_concurrent_agents: None,
        max_turns: None,
        token_budget: None,
    }
}

fn default_agent_factory() -> Arc<CoordinatorAgentFactoryFn> {
    Arc::new(|context| {
        Box::pin(async move {
            Err(format!(
                "No coordinator agent factory configured for {}",
                context.config.name
            ))
        })
    })
}

fn next_id(prefix: &str, counter: &AtomicU64) -> String {
    format!("{}-{}", prefix, counter.fetch_add(1, Ordering::Relaxed) + 1)
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_millis()
}

fn inactive_agent_result(agent_id: &str) -> TaskResult<Content> {
    TaskResult::error(inactive_agent_error(agent_id), 0)
}

fn inactive_agent_error(agent_id: &str) -> String {
    format!("Coordinator agent {agent_id} is no longer active")
}
