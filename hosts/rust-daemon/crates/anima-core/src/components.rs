use std::collections::BTreeMap;

use async_trait::async_trait;

use crate::primitives::{Content, DataValue, Message};
use crate::runtime::AgentRuntime;

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
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
    async fn validate(&self, runtime: &AgentRuntime, message: &Message) -> Result<bool, String>;
    async fn evaluate(
        &self,
        runtime: &AgentRuntime,
        message: &Message,
        response: &Content,
    ) -> Result<EvaluatorResult, String>;
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EvaluatorResult {
    pub score: Option<f64>,
    pub feedback: Option<String>,
    pub follow_up: Option<Content>,
    pub metadata: Option<BTreeMap<String, DataValue>>,
}
