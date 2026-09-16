use super::*;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GoalStatus {
    Active,
    Paused,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GoalRecord {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) objective: String,
    pub(crate) request_key: String,
    pub(crate) max_attempts: u32,
    pub(crate) status: GoalStatus,
    pub(crate) revision: u64,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GoalView {
    #[serde(flatten)]
    pub(crate) record: GoalRecord,
    pub(crate) consumed_attempts: u32,
    pub(crate) reserved_attempts: u32,
    pub(crate) remaining_attempts: u32,
    pub(crate) job_count: u32,
    pub(crate) accepted_outputs: u32,
}

fn accepted(job: &AgentJobRecord) -> bool {
    job.status == AgentJobStatus::Completed
        && job
            .attempts
            .last()
            .filter(|a| a.attempt == job.attempt && a.status == AgentJobStatus::Completed)
            .and_then(|a| a.review.as_ref())
            .is_some_and(|r| r.decision == JobReviewDecision::Accepted)
}

fn view<'a>(record: &GoalRecord, jobs: impl Iterator<Item = &'a AgentJobRecord>) -> GoalView {
    let mut result = GoalView {
        record: record.clone(),
        consumed_attempts: 0,
        reserved_attempts: 0,
        remaining_attempts: 0,
        job_count: 0,
        accepted_outputs: 0,
    };
    for job in jobs.filter(|j| j.goal_id.as_deref() == Some(record.id.as_str())) {
        result.consumed_attempts = result.consumed_attempts.saturating_add(job.attempt);
        result.reserved_attempts += u32::from(job.status == AgentJobStatus::Queued);
        result.accepted_outputs += u32::from(accepted(job));
        result.job_count += 1;
    }
    result.remaining_attempts = record.max_attempts.saturating_sub(
        result
            .consumed_attempts
            .saturating_add(result.reserved_attempts),
    );
    result
}

pub(crate) fn validate_goals(goals: &[GoalRecord], jobs: &[AgentJobRecord]) -> Result<(), String> {
    if goals.len() > 50 {
        return Err("Goal history exceeds capacity".into());
    }
    let mut ids = std::collections::HashSet::new();
    let mut keys = std::collections::HashSet::new();
    for goal in goals {
        if goal.id.trim().is_empty()
            || !ids.insert(goal.id.as_str())
            || !keys.insert(goal.request_key.as_str())
            || goal.title.trim().is_empty()
            || goal.title.chars().count() > 160
            || goal.objective.trim().is_empty()
            || goal.objective.len() > 32768
            || goal.request_key.trim().is_empty()
            || goal.request_key.len() > 128
            || !(1..=100).contains(&goal.max_attempts)
            || goal.revision == 0
            || goal.updated_at_ms < goal.created_at_ms
        {
            return Err("Invalid durable goal".into());
        }
        let current = view(goal, jobs.iter());
        if current
            .consumed_attempts
            .saturating_add(current.reserved_attempts)
            > goal.max_attempts
        {
            return Err("Goal attempt budget exceeded".into());
        }
        if goal.status == GoalStatus::Completed
            && (current.accepted_outputs == 0
                || jobs
                    .iter()
                    .filter(|j| j.goal_id.as_deref() == Some(goal.id.as_str()))
                    .any(|j| j.status != AgentJobStatus::Cancelled && !accepted(j)))
        {
            return Err("Completed goal has unfinished or unaccepted work".into());
        }
    }
    if jobs
        .iter()
        .any(|j| j.goal_id.as_deref().is_some_and(|id| !ids.contains(id)))
    {
        return Err("Job links a missing goal".into());
    }
    Ok(())
}

// Called under the same control-plane transaction as the enqueue mutation.
pub(super) fn check_link(
    state: &DaemonState,
    id: Option<&str>,
    reserve: bool,
) -> Result<(), JobError> {
    let Some(id) = id else {
        return Ok(());
    };
    let goal = state.goals.get(id).ok_or(JobError::NotFound)?;
    if goal.status == GoalStatus::Completed {
        return Err(JobError::Conflict(
            "Completed goals cannot accept work".into(),
        ));
    }
    if reserve {
        if goal.status != GoalStatus::Active {
            return Err(JobError::Conflict(
                "Resume the goal before queueing work".into(),
            ));
        }
        if view(goal, state.jobs.values()).remaining_attempts == 0 {
            return Err(JobError::Conflict(
                "Goal attempt budget is exhausted".into(),
            ));
        }
    }
    Ok(())
}

pub(super) fn dispatch_allowed(state: &DaemonState, job: &AgentJobRecord) -> bool {
    job.goal_id.as_deref().is_none_or(|id| {
        state
            .goals
            .get(id)
            .is_some_and(|goal| goal.status == GoalStatus::Active)
    })
}

impl JobService {
    pub(crate) async fn list_goals(&self) -> Result<Vec<GoalView>, JobError> {
        let _transaction = self.runs.control_plane_transaction().await;
        let state = self.state.read().await;
        let mut goals: Vec<_> = state
            .goals
            .values()
            .map(|g| view(g, state.jobs.values()))
            .collect();
        goals.sort_by(|a, b| {
            b.record
                .created_at_ms
                .cmp(&a.record.created_at_ms)
                .then_with(|| a.record.id.cmp(&b.record.id))
        });
        Ok(goals)
    }

    pub(crate) async fn create_goal(
        &self,
        title: &str,
        objective: &str,
        key: &str,
        max_attempts: u32,
    ) -> Result<GoalView, JobError> {
        if title.trim().is_empty()
            || title.chars().count() > 160
            || objective.trim().is_empty()
            || objective.len() > 32768
            || key.trim().is_empty()
            || key.len() > 128
            || !(1..=100).contains(&max_attempts)
        {
            return Err(JobError::Validation("Require title up to 160 characters, objective up to 32 KiB, request key up to 128 bytes, and 1 to 100 attempts".into()));
        }
        let (title, objective, key) = (title.to_owned(), objective.to_owned(), key.to_owned());
        self.mutate(move |state| {
            if let Some(goal) = state.goals.values().find(|g| g.request_key == key) {
                return if goal.title == title
                    && goal.objective == objective
                    && goal.max_attempts == max_attempts
                {
                    Ok(view(goal, state.jobs.values()))
                } else {
                    Err(JobError::Conflict(
                        "Goal request key was already used for different input".into(),
                    ))
                };
            }
            if state.goals.len() >= 50 {
                return Err(JobError::Conflict("Goal capacity is exhausted".into()));
            }
            let now = now_ms();
            let record = GoalRecord {
                id: uuid::Uuid::new_v4().to_string(),
                title,
                objective,
                request_key: key,
                max_attempts,
                status: GoalStatus::Active,
                revision: 1,
                created_at_ms: now,
                updated_at_ms: now,
            };
            let result = view(&record, state.jobs.values());
            state.goals.insert(record.id.clone(), record);
            Ok(result)
        })
        .await
    }

    pub(crate) async fn change_goal_status(
        &self,
        id: &str,
        revision: u64,
        status: GoalStatus,
    ) -> Result<GoalView, JobError> {
        let id = id.to_owned();
        self.mutate(move |state| {
            let record = state.goals.get(&id).ok_or(JobError::NotFound)?;
            if record.revision != revision {
                return Err(JobError::Conflict(
                    "Goal revision changed; refresh before editing".into(),
                ));
            }
            if record.status == GoalStatus::Completed {
                return Err(JobError::Conflict(
                    "Completed goals cannot change status".into(),
                ));
            }
            if status == GoalStatus::Completed {
                let current = view(record, state.jobs.values());
                let unfinished = state.jobs.values()
                    .filter(|job| job.goal_id.as_deref() == Some(id.as_str()))
                    .any(|job| job.status != AgentJobStatus::Cancelled && !accepted(job));
                if current.accepted_outputs == 0 || unfinished {
                    return Err(JobError::Conflict(
                        "Complete or cancel remaining work and accept its outputs before completing this goal".into(),
                    ));
                }
            }
            let record = state.goals.get_mut(&id).unwrap();
            record.status = status;
            record.revision = record.revision.saturating_add(1);
            record.updated_at_ms = record.updated_at_ms.max(now_ms());
            Ok(view(record, state.jobs.values()))
        })
        .await
    }

    pub(crate) async fn goal_jobs(&self, id: &str) -> Result<Vec<AgentJobRecord>, JobError> {
        let _transaction = self.runs.control_plane_transaction().await;
        let state = self.state.read().await;
        if !state.goals.contains_key(id) {
            return Err(JobError::NotFound);
        }
        let mut jobs: Vec<_> = state
            .jobs
            .values()
            .filter(|j| j.goal_id.as_deref() == Some(id))
            .cloned()
            .collect();
        jobs.sort_by(|a, b| {
            a.created_at_ms
                .cmp(&b.created_at_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(jobs)
    }
}
