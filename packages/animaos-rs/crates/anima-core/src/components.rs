use std::collections::BTreeMap;

use crate::primitives::{Content, DataValue, Message};
use crate::runtime::AgentRuntime;

pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn get(&self, runtime: &AgentRuntime, message: &Message) -> Result<ProviderResult, String>;
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProviderResult {
    pub text: String,
    pub metadata: Option<BTreeMap<String, DataValue>>,
}

pub trait Evaluator: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn validate(&self, runtime: &AgentRuntime, message: &Message) -> Result<bool, String>;
    fn evaluate(
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
