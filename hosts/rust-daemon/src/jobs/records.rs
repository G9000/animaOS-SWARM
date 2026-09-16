use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentJobStatus {
    AwaitingApproval,
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
    #[serde(default = "default_max_attempts")]
    pub(crate) max_attempts: u32,
    #[serde(default)]
    pub(crate) requires_approval: bool,
    #[serde(default)]
    pub(crate) approved_at_ms: Option<u64>,
    #[serde(default)]
    pub(crate) attempts: Vec<AgentJobAttempt>,
    #[serde(default)]
    pub(crate) goal_id: Option<String>,
}

fn default_max_attempts() -> u32 {
    3
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum JobReviewDecision {
    Accepted,
    ChangesRequested,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JobOutputReview {
    pub(crate) decision: JobReviewDecision,
    pub(crate) note: String,
    pub(crate) reviewed_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentJobAttempt {
    pub(crate) attempt: u32,
    pub(crate) status: AgentJobStatus,
    pub(crate) started_at_ms: u64,
    pub(crate) finished_at_ms: u64,
    pub(crate) result: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) result_truncated: bool,
    pub(crate) review: Option<JobOutputReview>,
}

impl AgentJobRecord {
    pub(crate) fn preserve_legacy_attempt(&mut self) {
        if matches!(
            self.status,
            AgentJobStatus::Completed | AgentJobStatus::Failed | AgentJobStatus::NeedsReview
        ) && !self.attempts.iter().any(|a| a.attempt == self.attempt)
        {
            if let (Some(start), Some(finish)) = (self.started_at_ms, self.finished_at_ms) {
                self.attempts.push(AgentJobAttempt {
                    attempt: self.attempt,
                    status: self.status,
                    started_at_ms: start,
                    finished_at_ms: finish,
                    result: self.result.clone(),
                    error: self.error.clone(),
                    // Legacy outputs did not retain truncation metadata. Conservatively mark a full preview.
                    result_truncated: self.result.as_ref().is_some_and(|r| r.len() >= 65533),
                    review: None,
                });
            }
        }
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if !(1..=3).contains(&self.max_attempts)
            || self.attempt > self.max_attempts
            || self.attempts.len() > self.max_attempts as usize
            || self.approved_at_ms.is_some_and(|t| {
                !self.requires_approval || t < self.created_at_ms || t > self.updated_at_ms
            })
            || (self.requires_approval
                && matches!(
                    self.status,
                    AgentJobStatus::Queued
                        | AgentJobStatus::Running
                        | AgentJobStatus::Completed
                        | AgentJobStatus::Failed
                        | AgentJobStatus::NeedsReview
                )
                && self.approved_at_ms.is_none())
            || (self.status == AgentJobStatus::AwaitingApproval
                && (!self.requires_approval || self.approved_at_ms.is_some()))
        {
            return Err("Invalid job attempt budget or approval state".into());
        }
        let mut previous = 0;
        for attempt in &self.attempts {
            if attempt.attempt <= previous
                || attempt.attempt > self.attempt
                || !matches!(
                    attempt.status,
                    AgentJobStatus::Completed
                        | AgentJobStatus::Failed
                        | AgentJobStatus::NeedsReview
                )
                || attempt.started_at_ms < self.created_at_ms
                || attempt.finished_at_ms < attempt.started_at_ms
                || attempt.finished_at_ms > self.updated_at_ms
                || attempt.result.as_ref().is_some_and(|r| r.len() > 65536)
                || attempt.error.as_ref().is_some_and(|r| r.len() > 65536)
                || (attempt.result_truncated && attempt.result.is_none())
                || (self.status == AgentJobStatus::Running && attempt.attempt == self.attempt)
            {
                return Err("Invalid job attempt history".into());
            }
            if let Some(review) = &attempt.review {
                if attempt.status != AgentJobStatus::Completed
                    || review.note.len() > 4000
                    || (review.decision == JobReviewDecision::ChangesRequested
                        && review.note.trim().is_empty())
                    || review.reviewed_at_ms < attempt.finished_at_ms
                    || review.reviewed_at_ms > self.updated_at_ms
                {
                    return Err("Invalid job output review".into());
                }
            }
            if attempt.attempt == self.attempt
                && matches!(
                    self.status,
                    AgentJobStatus::Completed
                        | AgentJobStatus::Failed
                        | AgentJobStatus::NeedsReview
                )
                && (attempt.status != self.status
                    || Some(attempt.started_at_ms) != self.started_at_ms
                    || Some(attempt.finished_at_ms) != self.finished_at_ms
                    || attempt.result != self.result
                    || attempt.error != self.error)
            {
                return Err("Latest attempt disagrees with saved output".into());
            }
            previous = attempt.attempt;
        }
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
                .finished_at_ms
                .zip(self.started_at_ms)
                .is_some_and(|(finish, start)| finish < start)
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
            AgentJobStatus::Queued | AgentJobStatus::AwaitingApproval
                if self.attempt >= self.max_attempts
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
