use crate::{
    agent_runs::{AgentRunCoordinator, AgentRunRequest, RunRoom},
    app::SharedDaemonState,
    routes::ApiError,
    state::DaemonState,
};
use anima_core::{Content, TaskStatus};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{watch, Mutex, Notify};
use utoipa::ToSchema;

const MAX_JOBS: usize = 200;
const MAX_ACTIVE: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentJobStatus {
    Queued,
    Running,
    Completed,
    Failed,
    NeedsReview,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentJobRecord {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) title: String,
    pub(crate) prompt: String,
    pub(crate) request_key: String,
    pub(crate) status: AgentJobStatus,
    pub(crate) revision: u64,
    pub(crate) attempt: u32,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
    pub(crate) started_at_ms: Option<u64>,
    pub(crate) finished_at_ms: Option<u64>,
    pub(crate) result: Option<String>,
    pub(crate) error: Option<String>,
}

impl AgentJobRecord {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty()
            || self.agent_id.trim().is_empty()
            || self.title.trim().is_empty()
            || self.title.chars().count() > 160
            || self.prompt.trim().is_empty()
            || self.prompt.len() > 32 * 1024
            || self.request_key.trim().is_empty()
            || self.request_key.len() > 128
            || self.revision == 0
            || self.attempt > 3
            || self.result.as_ref().is_some_and(|v| v.len() > 64 * 1024)
            || self.error.as_ref().is_some_and(|v| v.len() > 64 * 1024)
            || self.updated_at_ms < self.created_at_ms
            || self
                .started_at_ms
                .is_some_and(|t| t < self.created_at_ms || t > self.updated_at_ms)
            || self
                .finished_at_ms
                .is_some_and(|t| t < self.created_at_ms || t > self.updated_at_ms)
        {
            return Err("Invalid durable job bounds or timestamps".into());
        }
        match self.status {
            AgentJobStatus::Queued
                if self.attempt >= 3
                    || self.started_at_ms.is_some()
                    || self.finished_at_ms.is_some() =>
            {
                Err("Invalid queued job".into())
            }
            AgentJobStatus::Running
                if self.attempt == 0
                    || self.started_at_ms.is_none()
                    || self.finished_at_ms.is_some() =>
            {
                Err("Invalid running job".into())
            }
            AgentJobStatus::Completed | AgentJobStatus::Failed | AgentJobStatus::NeedsReview
                if self.attempt == 0
                    || self.started_at_ms.is_none()
                    || self.finished_at_ms.is_none() =>
            {
                Err("Invalid finished job".into())
            }
            AgentJobStatus::Cancelled if self.finished_at_ms.is_none() => {
                Err("Invalid cancelled job".into())
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug)]
pub(crate) enum JobError {
    NotFound,
    Validation(String),
    Conflict(String),
    Unavailable(String),
}

type Worker = (watch::Sender<bool>, tokio::task::JoinHandle<()>);

#[derive(Clone)]
pub(crate) struct JobService {
    state: SharedDaemonState,
    runs: AgentRunCoordinator,
    wake: Arc<Notify>,
    worker: Arc<Mutex<Option<Worker>>>,
}

impl JobService {
    pub(crate) fn new(state: SharedDaemonState, runs: AgentRunCoordinator) -> Self {
        Self {
            state,
            runs,
            wake: Arc::new(Notify::new()),
            worker: Arc::new(Mutex::new(None)),
        }
    }

    // Keep the transaction alive even if a browser drops its response future.
    async fn mutate<T, F>(&self, action: F) -> Result<T, JobError>
    where
        T: Send + 'static,
        F: FnOnce(&mut DaemonState) -> Result<T, JobError> + Send + 'static,
    {
        let service = self.clone();
        tokio::spawn(async move {
            let _transaction = service.runs.control_plane_transaction().await;
            let (before, value, persist) = {
                let mut state = service.state.write().await;
                if state.control_plane_store.is_none() {
                    return Err(JobError::Conflict(
                        "Durable jobs require control-plane persistence".into(),
                    ));
                }
                let before = state.jobs.clone();
                let value = match action(&mut state) {
                    Ok(value) => value,
                    Err(error) => {
                        state.jobs = before;
                        return Err(error);
                    }
                };
                (before, value, state.control_plane_persist_request())
            };
            if let Err(error) = persist.save().await {
                service.state.write().await.jobs = before;
                return Err(JobError::Unavailable(error.to_string()));
            }
            service.wake.notify_one();
            Ok(value)
        })
        .await
        .map_err(|_| JobError::Unavailable("Job persistence worker stopped".into()))?
    }

    pub(crate) async fn list(&self, agent_id: &str) -> Result<Vec<AgentJobRecord>, JobError> {
        // Do not expose a provisional job while its snapshot may still roll back.
        let _transaction = self.runs.control_plane_transaction().await;
        let state = self.state.read().await;
        if state.get_agent(agent_id).is_none() {
            return Err(JobError::NotFound);
        }
        let mut jobs: Vec<_> = state
            .jobs
            .values()
            .filter(|j| j.agent_id == agent_id)
            .cloned()
            .collect();
        jobs.sort_by(|a, b| {
            b.created_at_ms
                .cmp(&a.created_at_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(jobs)
    }

    pub(crate) async fn create(
        &self,
        agent_id: &str,
        title: &str,
        prompt: &str,
        request_key: &str,
    ) -> Result<AgentJobRecord, JobError> {
        if title.trim().is_empty()
            || title.chars().count() > 160
            || prompt.trim().is_empty()
            || prompt.len() > 32 * 1024
            || request_key.trim().is_empty()
            || request_key.len() > 128
        {
            return Err(JobError::Validation("Require a title up to 160 characters, prompt up to 32 KiB, and request key up to 128 bytes".into()));
        }
        let (agent_id, title, prompt, request_key) = (
            agent_id.to_owned(),
            title.to_owned(),
            prompt.to_owned(),
            request_key.to_owned(),
        );
        self.mutate(move |state| {
            if state.get_agent(&agent_id).is_none() {
                return Err(JobError::NotFound);
            }
            if let Some(job) = state
                .jobs
                .values()
                .find(|j| j.agent_id == agent_id && j.request_key == request_key)
            {
                return if job.agent_id == agent_id && job.title == title && job.prompt == prompt {
                    Ok(job.clone())
                } else {
                    Err(JobError::Conflict(
                        "Request key was already used for different input".into(),
                    ))
                };
            }
            if state.jobs.len() >= MAX_JOBS {
                return Err(JobError::Conflict(
                    "Job history capacity is exhausted".into(),
                ));
            }
            ensure_capacity(&state.jobs)?;
            let now = now_ms();
            let job = AgentJobRecord {
                id: uuid::Uuid::new_v4().to_string(),
                agent_id,
                title,
                prompt,
                request_key,
                status: AgentJobStatus::Queued,
                revision: 1,
                attempt: 0,
                created_at_ms: now,
                updated_at_ms: now,
                started_at_ms: None,
                finished_at_ms: None,
                result: None,
                error: None,
            };
            state.jobs.insert(job.id.clone(), job.clone());
            Ok(job)
        })
        .await
    }

    pub(crate) async fn cancel(
        &self,
        agent_id: &str,
        id: &str,
        revision: u64,
    ) -> Result<AgentJobRecord, JobError> {
        let (agent_id, id) = (agent_id.to_owned(), id.to_owned());
        self.mutate(move |state| {
            let job = checked_job(state, &agent_id, &id, revision)?;
            if job.status != AgentJobStatus::Queued {
                return Err(JobError::Conflict(
                    "Only queued jobs can be cancelled".into(),
                ));
            }
            job.status = AgentJobStatus::Cancelled;
            advance(job);
            job.finished_at_ms = Some(job.updated_at_ms);
            Ok(job.clone())
        })
        .await
    }

    pub(crate) async fn retry(
        &self,
        agent_id: &str,
        id: &str,
        revision: u64,
        acknowledge_uncertain: bool,
    ) -> Result<AgentJobRecord, JobError> {
        let (agent_id, id) = (agent_id.to_owned(), id.to_owned());
        self.mutate(move |state| {
            ensure_capacity(&state.jobs)?;
            let job = checked_job(state, &agent_id, &id, revision)?;
            if !matches!(
                job.status,
                AgentJobStatus::Failed | AgentJobStatus::NeedsReview
            ) || job.attempt >= 3
            {
                return Err(JobError::Conflict(
                    "Only failed or review jobs below three attempts can be retried".into(),
                ));
            }
            if job.status == AgentJobStatus::NeedsReview && !acknowledge_uncertain {
                return Err(JobError::Conflict(
                    "Acknowledge that retrying uncertain work may repeat external effects".into(),
                ));
            }
            job.status = AgentJobStatus::Queued;
            advance(job);
            job.started_at_ms = None;
            job.finished_at_ms = None;
            job.result = None;
            job.error = None;
            Ok(job.clone())
        })
        .await
    }

    pub(crate) async fn start(&self) -> Result<(), JobError> {
        let mut worker = self.worker.lock().await;
        if worker.is_some() {
            return Ok(());
        }
        // No store is a supported daemon configuration; creation stays disabled.
        if self.state.read().await.control_plane_store.is_none() {
            return Ok(());
        }
        self.mutate(|state| {
            for job in state
                .jobs
                .values_mut()
                .filter(|j| j.status == AgentJobStatus::Running)
            {
                review(
                    job,
                    "Daemon restarted during this attempt; inspect effects before retrying",
                );
            }
            Ok(())
        })
        .await?;
        let (stop, receiver) = watch::channel(false);
        let service = self.clone();
        *worker = Some((
            stop,
            tokio::spawn(async move {
                service.dispatch(receiver).await;
            }),
        ));
        Ok(())
    }

    pub(crate) async fn shutdown(&self) {
        let mut worker = self.worker.lock().await;
        if let Some((stop, handle)) = worker.take() {
            let _ = stop.send(true);
            // Finish admitted runs; interrupting a model call cannot guarantee cancellation.
            let _ = handle.await;
        }
    }

    async fn dispatch(&self, mut stop: watch::Receiver<bool>) {
        let mut active = tokio::task::JoinSet::new();
        let mut agents = std::collections::HashSet::new();
        let mut task_agents = HashMap::new();
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        loop {
            if *stop.borrow() {
                break;
            }
            tokio::select! {
                _ = stop.changed() => { break; }
                _ = self.wake.notified() => {}
                _ = tick.tick() => {}
                done = active.join_next_with_id(), if !active.is_empty() => {
                    if let Some(done) = done {
                        let task_id = match done {
                            Ok((id, ())) => id,
                            Err(error) => {
                                // Its persisted running claim remains uncertain and is
                                // never replayed. Do not block unrelated queued work.
                                tracing::warn!(error = %error, "durable job worker stopped unexpectedly");
                                error.id()
                            }
                        };
                        if let Some(agent) = task_agents.remove(&task_id) { agents.remove(&agent); }
                    }
                }
            }
            if *stop.borrow() {
                break;
            }
            let mut queued: Vec<_> = self
                .state
                .read()
                .await
                .jobs
                .values()
                .filter(|j| j.status == AgentJobStatus::Queued)
                .cloned()
                .collect();
            queued.sort_by_key(|j| (j.created_at_ms, j.id.clone()));
            for candidate in queued {
                if agents.contains(&candidate.agent_id)
                    || self.runs.is_agent_busy(&candidate.agent_id)
                {
                    continue;
                }
                let Ok(permit) = self.runs.try_admit() else {
                    break;
                };
                let id = candidate.id.clone();
                let claimed = self
                    .mutate(move |state| {
                        let job = state.jobs.get_mut(&id).ok_or(JobError::NotFound)?;
                        if job.status != AgentJobStatus::Queued {
                            return Err(JobError::Conflict("Job is no longer queued".into()));
                        }
                        if job.attempt >= 3 {
                            return Err(JobError::Conflict("Attempt limit reached".into()));
                        }
                        job.status = AgentJobStatus::Running;
                        job.attempt += 1;
                        advance(job);
                        job.started_at_ms = Some(job.updated_at_ms);
                        Ok(job.clone())
                    })
                    .await;
                let Ok(job) = claimed else {
                    continue;
                };
                agents.insert(job.agent_id.clone());
                let service = self.clone();
                let agent = job.agent_id.clone();
                let task = active.spawn(async move {
                    service.execute(job, permit).await;
                });
                task_agents.insert(task.id(), agent);
            }
        }
        while active.join_next().await.is_some() {}
    }

    async fn execute(&self, job: AgentJobRecord, permit: crate::agent_runs::AgentRunPermit) {
        let commit_id = job.id.clone();
        let rollback_job = job.clone();
        let revision = job.revision;
        let run = self
            .runs
            .run_with_commit_admitted_and_rollback(
                AgentRunRequest {
                    agent_id: job.agent_id.clone(),
                    content: Content {
                        text: job.prompt.clone(),
                        attachments: None,
                        metadata: None,
                    },
                    room: RunRoom::Stable(format!("job:{}", job.id)),
                    idempotency_key: Some(format!("job:{}:attempt:{}", job.id, job.attempt)),
                },
                permit,
                move |state, _, result| {
                    let current = state
                        .jobs
                        .get_mut(&commit_id)
                        .filter(|j| j.status == AgentJobStatus::Running && j.revision == revision)
                        .ok_or_else(|| ApiError::service_unavailable("Job claim changed"))?;
                    current.status = if result.status == TaskStatus::Success {
                        AgentJobStatus::Completed
                    } else {
                        AgentJobStatus::Failed
                    };
                    current.result = result.data.as_ref().map(|c| preview(&c.text));
                    current.error = result.error.as_ref().map(|e| preview(e));
                    advance(current);
                    current.finished_at_ms = Some(current.updated_at_ms);
                    Ok(())
                },
                move |state, _baseline| {
                    state.jobs.insert(rollback_job.id.clone(), rollback_job);
                    Ok(())
                },
            )
            .await;
        if run.is_err() {
            let id = job.id.clone();
            let result = self
                .mutate(move |state| {
                    if let Some(current) = state
                        .jobs
                        .get_mut(&id)
                        .filter(|j| j.status == AgentJobStatus::Running && j.revision == revision)
                    {
                        review(
                            current,
                            "Run did not commit a durable result; inspect effects before retrying",
                        );
                    }
                    Ok(())
                })
                .await;
            if result.is_err() {
                // Persistence is unavailable. Preserve uncertainty in RAM, and the saved
                // running claim becomes needs_review on restart; neither can replay.
                let _transaction = self.runs.control_plane_transaction().await;
                if let Some(current) = self
                    .state
                    .write()
                    .await
                    .jobs
                    .get_mut(&job.id)
                    .filter(|j| j.status == AgentJobStatus::Running && j.revision == revision)
                {
                    review(
                        current,
                        "Result persistence failed; inspect effects before retrying",
                    );
                }
            }
        }
    }
}

fn ensure_capacity(jobs: &HashMap<String, AgentJobRecord>) -> Result<(), JobError> {
    if jobs
        .values()
        .filter(|j| matches!(j.status, AgentJobStatus::Queued | AgentJobStatus::Running))
        .count()
        >= MAX_ACTIVE
    {
        Err(JobError::Conflict(
            "Active job capacity is exhausted".into(),
        ))
    } else {
        Ok(())
    }
}
fn checked_job<'a>(
    state: &'a mut DaemonState,
    agent_id: &str,
    id: &str,
    revision: u64,
) -> Result<&'a mut AgentJobRecord, JobError> {
    if state.get_agent(agent_id).is_none() {
        return Err(JobError::NotFound);
    }
    let job = state
        .jobs
        .get_mut(id)
        .filter(|j| j.agent_id == agent_id)
        .ok_or(JobError::NotFound)?;
    if job.revision != revision {
        return Err(JobError::Conflict(
            "Job revision changed; refresh before editing".into(),
        ));
    }
    Ok(job)
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
fn advance(job: &mut AgentJobRecord) {
    job.revision = job.revision.saturating_add(1);
    // Clock corrections must not make a saved lifecycle invalid on restart.
    job.updated_at_ms = job.updated_at_ms.max(now_ms());
}
fn review(job: &mut AgentJobRecord, error: &str) {
    job.status = AgentJobStatus::NeedsReview;
    job.error = Some(error.into());
    advance(job);
    job.finished_at_ms = Some(job.updated_at_ms);
}
fn preview(value: &str) -> String {
    let mut end = value.len().min(64 * 1024);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

#[cfg(test)]
mod tests;
