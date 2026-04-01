use std::collections::BTreeMap;

use crate::agent::{AgentConfig, TokenUsage};
use crate::primitives::{Content, DataValue, Message};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelStopReason {
    End,
    ToolCall,
    MaxTokens,
}

impl ModelStopReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::End => "end",
            Self::ToolCall => "tool_call",
            Self::MaxTokens => "max_tokens",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelGenerateRequest {
    pub system: String,
    pub messages: Vec<Message>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelGenerateResponse {
    pub content: Content,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub usage: TokenUsage,
    pub stop_reason: ModelStopReason,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: BTreeMap<String, DataValue>,
}

pub trait ModelAdapter: Send + Sync {
    fn provider(&self) -> &str;

    fn generate(
        &self,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String>;
}

#[cfg(test)]
mod tests {
    use super::ModelStopReason;

    #[test]
    fn model_stop_reason_matches_ts_contract() {
        assert_eq!(ModelStopReason::End.as_str(), "end");
        assert_eq!(ModelStopReason::ToolCall.as_str(), "tool_call");
        assert_eq!(ModelStopReason::MaxTokens.as_str(), "max_tokens");
    }
}
