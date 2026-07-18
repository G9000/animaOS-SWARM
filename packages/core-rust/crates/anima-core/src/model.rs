use std::collections::BTreeMap;

use async_trait::async_trait;

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

/// Normalized frames delivered by an adapter while generation is in flight.
#[derive(Clone, Debug, PartialEq)]
pub enum ModelStreamFrame {
    TextDelta(String),
    Final(ModelGenerateResponse),
}

#[async_trait]
pub trait ModelStreamSink: Send + Sync {
    async fn emit(&self, frame: ModelStreamFrame) -> Result<(), String>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: BTreeMap<String, DataValue>,
}

#[async_trait]
pub trait ModelAdapter: Send + Sync {
    fn provider(&self) -> &str;

    async fn generate(
        &self,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String>;

    /// Default streaming preserves existing adapters by emitting their final normalized result.
    async fn stream(
        &self,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
        sink: &dyn ModelStreamSink,
    ) -> Result<(), String> {
        let _ = sink
            .emit(ModelStreamFrame::Final(
                self.generate(config, request).await?,
            ))
            .await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ModelAdapter, ModelGenerateRequest, ModelGenerateResponse, ModelStopReason,
        ModelStreamFrame, ModelStreamSink,
    };
    use crate::{AgentConfig, Content, TokenUsage};
    use async_trait::async_trait;

    struct RecordingSink(std::sync::Mutex<Vec<ModelStreamFrame>>);

    #[async_trait]
    impl ModelStreamSink for RecordingSink {
        async fn emit(&self, frame: ModelStreamFrame) -> Result<(), String> {
            self.0.lock().unwrap().push(frame);
            Ok(())
        }
    }

    struct FinalOnlyAdapter;

    fn config() -> AgentConfig {
        AgentConfig {
            name: "test".into(),
            model: "test".into(),
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
        }
    }

    #[async_trait]
    impl ModelAdapter for FinalOnlyAdapter {
        fn provider(&self) -> &str {
            "test"
        }

        async fn generate(
            &self,
            _config: &AgentConfig,
            _request: &ModelGenerateRequest,
        ) -> Result<ModelGenerateResponse, String> {
            Ok(ModelGenerateResponse {
                content: Content {
                    text: "done".into(),
                    attachments: None,
                    metadata: None,
                },
                tool_calls: None,
                usage: TokenUsage::default(),
                stop_reason: ModelStopReason::End,
            })
        }
    }

    #[test]
    fn model_stop_reason_matches_ts_contract() {
        assert_eq!(ModelStopReason::End.as_str(), "end");
        assert_eq!(ModelStopReason::ToolCall.as_str(), "tool_call");
        assert_eq!(ModelStopReason::MaxTokens.as_str(), "max_tokens");
    }

    #[tokio::test]
    async fn default_stream_emits_one_final_frame_from_generate() {
        let sink = RecordingSink(std::sync::Mutex::new(Vec::new()));
        FinalOnlyAdapter
            .stream(
                &config(),
                &ModelGenerateRequest {
                    system: String::new(),
                    messages: vec![],
                    temperature: None,
                    max_tokens: None,
                },
                &sink,
            )
            .await
            .unwrap();

        assert_eq!(
            sink.0.lock().unwrap().as_slice(),
            &[ModelStreamFrame::Final(ModelGenerateResponse {
                content: Content {
                    text: "done".into(),
                    attachments: None,
                    metadata: None
                },
                tool_calls: None,
                usage: TokenUsage::default(),
                stop_reason: ModelStopReason::End,
            })]
        );
    }
}
