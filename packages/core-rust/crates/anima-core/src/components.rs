use std::collections::BTreeMap;

use async_trait::async_trait;

use crate::primitives::{Content, DataValue, Message};
use crate::runtime::AgentRuntime;

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    /// Higher values run earlier. Ties preserve registration order.
    fn priority(&self) -> u8 {
        0
    }
    async fn get(
        &self,
        runtime: &AgentRuntime,
        message: &Message,
    ) -> Result<ProviderResult, String>;
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProviderResult {
    pub text: String,
    pub metadata: Option<BTreeMap<String, DataValue>>,
}

#[async_trait]
pub trait Evaluator: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    /// Higher values run earlier. Ties preserve registration order.
    fn priority(&self) -> u8 {
        0
    }
    async fn validate(&self, runtime: &AgentRuntime, message: &Message) -> Result<bool, String>;
    async fn evaluate(
        &self,
        runtime: &AgentRuntime,
        message: &Message,
        response: &Content,
    ) -> Result<EvaluatorResult, String>;
}

/// Explicit evaluator verdict for the current response.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum EvaluatorDecision {
    #[default]
    Accept,
    Retry {
        feedback: String,
    },
    Abort {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct EvaluatorResult {
    /// Accept the response, request a correction pass, or abort the task.
    pub decision: EvaluatorDecision,
    pub score: Option<f64>,
    pub follow_up: Option<Content>,
    pub metadata: Option<BTreeMap<String, DataValue>>,
}

impl Default for EvaluatorResult {
    fn default() -> Self {
        Self {
            decision: EvaluatorDecision::Accept,
            score: None,
            follow_up: None,
            metadata: None,
        }
    }
}

impl EvaluatorResult {
    /// Accept the current response and continue normal completion.
    pub fn accept() -> Self {
        Self::default()
    }

    /// Ask the runtime to regenerate once more with the provided correction feedback.
    pub fn retry(feedback: impl Into<String>) -> Self {
        Self {
            decision: EvaluatorDecision::Retry {
                feedback: feedback.into(),
            },
            ..Self::default()
        }
    }

    /// Reject the current response and fail the task with the provided reason.
    pub fn abort(reason: impl Into<String>) -> Self {
        Self {
            decision: EvaluatorDecision::Abort {
                reason: reason.into(),
            },
            ..Self::default()
        }
    }
}
