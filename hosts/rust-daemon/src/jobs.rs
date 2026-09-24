use crate::{
    agent_runs::{AgentRunCoordinator, AgentRunRequest, RunRoom},
    app::SharedDaemonState,
    routes::ApiError,
    runs::RunSource,
    state::DaemonState,
};
use anima_core::{Content, TaskStatus};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{watch, Mutex, Notify};

const MAX_JOBS: usize = 200;
const MAX_ACTIVE: usize = 8;

mod goals;
pub(crate) use goals::{validate_goals, GoalRecord, GoalStatus, GoalView};
mod records;
pub(crate) use records::{
    AgentJobAttempt, AgentJobRecord, AgentJobStatus, JobOutputReview, JobReviewDecision,
};

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
                let before = (state.jobs.clone(), state.goals.clone());
                let value = match action(&mut state) {
                    Ok(value) => value,
                    Err(error) => {
                        state.jobs = before.0;
                        state.goals = before.1;
                        return Err(error);
                    }
                };
                (before, value, state.control_plane_persist_request())
            };
            if let Err(error) = persist.save().await {
                let mut state = service.state.write().await;
                state.jobs = before.0;
                state.goals = before.1;
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
        self.create_with_controls(agent_id, title, prompt, request_key, 3, false)
            .await
    }

    pub(crate) async fn create_with_controls(
        &self,
        agent_id: &str,
        title: &str,
        prompt: &str,
        request_key: &str,
        max_attempts: u32,
        requires_approval: bool,
    ) -> Result<AgentJobRecord, JobError> {
        self.create_with_goal(
            agent_id,
            title,
            prompt,
            request_key,
            max_attempts,
            requires_approval,
            None,
        )
        .await
    }

    pub(crate) async fn create_with_goal(
        &self,
        agent_id: &str,
        title: &str,
        prompt: &str,
        request_key: &str,
        max_attempts: u32,
        requires_approval: bool,
        goal_id: Option<&str>,
    ) -> Result<AgentJobRecord, JobError> {
        let goal_id = goal_id.map(str::to_owned);
        if !(1..=3).contains(&max_attempts) {
            return Err(JobError::Validation(
                "Maximum attempts must be between 1 and 3".into(),
            ));
        }
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
                return if job.agent_id == agent_id
                    && job.title == title
                    && job.prompt == prompt
                    && job.max_attempts == max_attempts
                    && job.requires_approval == requires_approval
                    && job.goal_id == goal_id
                {
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
            goals::check_link(state, goal_id.as_deref(), !requires_approval)?;
            if !requires_approval {
                ensure_capacity(&state.jobs)?;
            }
            let now = now_ms();
            let job = AgentJobRecord {
                id: uuid::Uuid::new_v4().to_string(),
                agent_id,
                title,
                prompt,
                request_key,
                status: if requires_approval {
                    AgentJobStatus::AwaitingApproval
                } else {
                    AgentJobStatus::Queued
                },
                revision: 1,
                attempt: 0,
                created_at_ms: now,
                updated_at_ms: now,
                started_at_ms: None,
                finished_at_ms: None,
                result: None,
                error: None,
                max_attempts,
                requires_approval,
                approved_at_ms: None,
                attempts: vec![],
                goal_id,
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
            if !matches!(
                job.status,
                AgentJobStatus::Queued | AgentJobStatus::AwaitingApproval
            ) {
                return Err(JobError::Conflict(
                    "Only queued jobs or pending proposals can be cancelled".into(),
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
            let checked = checked_job(state, &agent_id, &id, revision)?;
            let requires_approval = checked.requires_approval;
            let goal_id = checked.goal_id.clone();
            goals::check_link(state, goal_id.as_deref(), !requires_approval)?;
            if !requires_approval { ensure_capacity(&state.jobs)?; }
            let job = checked_job(state, &agent_id, &id, revision)?;
            job.preserve_legacy_attempt();
            let changes_requested = job.status == AgentJobStatus::Completed && job.attempts.last()
                .and_then(|a| a.review.as_ref()).is_some_and(|r| r.decision == JobReviewDecision::ChangesRequested);
            if !changes_requested && !matches!(
                job.status,
                AgentJobStatus::Failed | AgentJobStatus::NeedsReview
            ) || job.attempt >= job.max_attempts
            {
                return Err(JobError::Conflict(
                    "Only failed, uncertain, or changes-requested jobs below their attempt limit can be retried".into(),
                ));
            }
            if job.status == AgentJobStatus::NeedsReview && !acknowledge_uncertain {
                return Err(JobError::Conflict(
                    "Acknowledge that retrying uncertain work may repeat external effects".into(),
                ));
            }
            job.status = if job.requires_approval { AgentJobStatus::AwaitingApproval } else { AgentJobStatus::Queued };
            job.approved_at_ms = None;
            advance(job);
            job.started_at_ms = None;
            job.finished_at_ms = None;
            job.result = None;
            job.error = None;
            Ok(job.clone())
        })
        .await
    }

    pub(crate) async fn approve(
        &self,
        agent_id: &str,
        id: &str,
        revision: u64,
    ) -> Result<AgentJobRecord, JobError> {
        let (agent_id, id) = (agent_id.to_owned(), id.to_owned());
        self.mutate(move |state| {
            let goal_id = checked_job(state, &agent_id, &id, revision)?
                .goal_id
                .clone();
            goals::check_link(state, goal_id.as_deref(), true)?;
            ensure_capacity(&state.jobs)?;
            let job = checked_job(state, &agent_id, &id, revision)?;
            if job.status != AgentJobStatus::AwaitingApproval {
                return Err(JobError::Conflict(
                    "Only pending proposals can be approved".into(),
                ));
            }
            job.status = AgentJobStatus::Queued;
            advance(job);
            job.approved_at_ms = Some(job.updated_at_ms);
            Ok(job.clone())
        })
        .await
    }

    pub(crate) async fn review_output(
        &self,
        agent_id: &str,
        id: &str,
        revision: u64,
        decision: JobReviewDecision,
        note: &str,
    ) -> Result<AgentJobRecord, JobError> {
        if note.len() > 4000
            || (decision == JobReviewDecision::ChangesRequested && note.trim().is_empty())
        {
            return Err(JobError::Validation(
                "Feedback must be at most 4000 bytes; requesting changes requires a note".into(),
            ));
        }
        let (agent_id, id, note) = (agent_id.to_owned(), id.to_owned(), note.to_owned());
        self.mutate(move |state| {
            let job = checked_job(state, &agent_id, &id, revision)?;
            if job.status != AgentJobStatus::Completed {
                return Err(JobError::Conflict(
                    "Only the latest completed output can be reviewed".into(),
                ));
            }
            job.preserve_legacy_attempt();
            if job
                .attempts
                .last()
                .is_none_or(|a| a.attempt != job.attempt || a.review.is_some())
            {
                return Err(JobError::Conflict(
                    "Output is missing or has already been reviewed".into(),
                ));
            }
            advance(job);
            job.attempts.last_mut().unwrap().review = Some(JobOutputReview {
                decision,
                note,
                reviewed_at_ms: job.updated_at_ms,
            });
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
            let mut queued: Vec<_> = {
                let state = self.state.read().await;
                state
                    .jobs
                    .values()
                    .filter(|job| {
                        job.status == AgentJobStatus::Queued && goals::dispatch_allowed(&state, job)
                    })
                    .cloned()
                    .collect()
            };
            queued.sort_by_key(|j| (j.created_at_ms, j.id.clone()));
            for candidate in queued {
                if agents.contains(&candidate.agent_id)
                    || self.runs.is_agent_busy(&candidate.agent_id)
                {
                    continue;
                }
                if !self.runs.has_available_permit() {
                    break;
                }
                // Take the room, a slot, and the permit before the durable claim so a
                // claimed job always starts; none of the three waits.
                let Ok(ticket) = self
                    .runs
                    .try_ticket(&candidate.agent_id, &format!("job:{}", candidate.id))
                    .await
                else {
                    continue;
                };
                let id = candidate.id.clone();
                let claimed = self
                    .mutate(move |state| {
                        let candidate = state.jobs.get(&id).ok_or(JobError::NotFound)?;
                        if !goals::dispatch_allowed(state, candidate) {
                            return Err(JobError::Conflict("Goal is paused or unavailable".into()));
                        }
                        let job = state.jobs.get_mut(&id).ok_or(JobError::NotFound)?;
                        if job.status != AgentJobStatus::Queued {
                            return Err(JobError::Conflict("Job is no longer queued".into()));
                        }
                        if job.attempt >= job.max_attempts
                            || (job.requires_approval && job.approved_at_ms.is_none())
                        {
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
                    service.execute(job, ticket).await;
                });
                task_agents.insert(task.id(), agent);
            }
        }
        while active.join_next().await.is_some() {}
    }

    async fn execute(&self, job: AgentJobRecord, ticket: crate::agent_runs::RunTicket) {
        let commit_id = job.id.clone();
        let rollback_job = job.clone();
        let revision = job.revision;
        // The run's room comes from the ticket that locked it, so the two cannot drift.
        let room = RunRoom::Stable(ticket.room_id().to_string());
        let run = self
            .runs
            .run_ticketed_with_commit_and_rollback(
                AgentRunRequest {
                    agent_id: job.agent_id.clone(),
                    content: Content {
                        text: job_prompt(&job),
                        attachments: None,
                        metadata: None,
                    },
                    room,
                    idempotency_key: Some(format!("job:{}:attempt:{}", job.id, job.attempt)),
                    source: RunSource::Job,
                    source_ref: Some(format!("{}:{}", job.id, job.attempt)),
                    parent: None,
                },
                ticket,
                move |state, outcome| {
                    let result = &outcome.result;
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
                    current.preserve_legacy_attempt();
                    if let Some(attempt) = current.attempts.last_mut() {
                        attempt.result_truncated =
                            result.data.as_ref().is_some_and(|c| c.text.len() > 65536);
                    }
                    Ok(())
                },
                move |state| {
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
    job.preserve_legacy_attempt();
}
fn job_prompt(job: &AgentJobRecord) -> String {
    let feedback = job
        .attempts
        .iter()
        .rev()
        .filter_map(|a| a.review.as_ref())
        .find(|r| r.decision == JobReviewDecision::ChangesRequested);
    match feedback {
        Some(review) => format!(
            "{}\n\nOwner feedback on the previous attempt:\n{}",
            job.prompt, review.note
        ),
        None => job.prompt.clone(),
    }
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
