use anima_core::{
    AgentConfig, Content, ModelAdapter, ModelGenerateRequest, ModelGenerateResponse,
    ModelStopReason, ModelStreamFrame, ModelStreamSink, TokenUsage,
};
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;

/// A host-controlled adapter for deterministic integration tests and local demonstrations.
#[derive(Clone, Debug)]
pub struct DeterministicModelAdapter {
    text: String,
}

impl DeterministicModelAdapter {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    fn response(&self) -> ModelGenerateResponse {
        ModelGenerateResponse {
            content: Content {
                text: self.text.clone(),
                attachments: None,
                metadata: None,
            },
            tool_calls: None,
            usage: TokenUsage::default(),
            stop_reason: ModelStopReason::End,
        }
    }
}

#[async_trait]
impl ModelAdapter for DeterministicModelAdapter {
    fn provider(&self) -> &str {
        "deterministic"
    }

    async fn generate(
        &self,
        _config: &AgentConfig,
        _request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        Ok(self.response())
    }

    async fn stream(
        &self,
        _config: &AgentConfig,
        _request: &ModelGenerateRequest,
        sink: &dyn ModelStreamSink,
    ) -> Result<(), String> {
        for delta in self.text.split_inclusive(' ') {
            let _ = sink
                .emit(ModelStreamFrame::TextDelta(delta.to_owned()))
                .await;
        }
        let _ = sink.emit(ModelStreamFrame::Final(self.response())).await;
        Ok(())
    }
}

pub(crate) async fn consume_openai_sse(
    response: reqwest::Response,
    sink: &dyn ModelStreamSink,
) -> Result<(), String> {
    consume_sse(response, sink, |payload, text, stop_reason| {
        let choice = payload.get("choices")?.as_array()?.first()?;
        if let Some(delta) = choice
            .get("delta")
            .and_then(|delta| delta.get("content"))
            .and_then(Value::as_str)
        {
            text.push_str(delta);
            Some(delta.to_owned())
        } else {
            if choice.get("finish_reason").and_then(Value::as_str) == Some("length") {
                *stop_reason = ModelStopReason::MaxTokens;
            }
            None
        }
    })
    .await
}

pub(crate) async fn consume_anthropic_sse(
    response: reqwest::Response,
    sink: &dyn ModelStreamSink,
) -> Result<(), String> {
    consume_sse(response, sink, |payload, text, stop_reason| {
        let event_type = payload.get("type").and_then(Value::as_str);
        if event_type == Some("content_block_delta") {
            let delta = payload
                .get("delta")
                .and_then(|delta| delta.get("text"))
                .and_then(Value::as_str)?;
            text.push_str(delta);
            return Some(delta.to_owned());
        }
        if event_type == Some("message_delta")
            && payload
                .get("delta")
                .and_then(|delta| delta.get("stop_reason"))
                .and_then(Value::as_str)
                == Some("max_tokens")
        {
            *stop_reason = ModelStopReason::MaxTokens;
        }
        None
    })
    .await
}

async fn consume_sse(
    response: reqwest::Response,
    sink: &dyn ModelStreamSink,
    mut parse: impl FnMut(&Value, &mut String, &mut ModelStopReason) -> Option<String>,
) -> Result<(), String> {
    let mut body = response.bytes_stream();
    let mut pending = String::new();
    let mut text = String::new();
    let mut stop_reason = ModelStopReason::End;
    while let Some(chunk) = body.next().await {
        let chunk = chunk
            .map_err(|error| format!("provider stream read failed: {}", error.without_url()))?;
        pending.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(boundary) = pending.find("\n\n") {
            let event = pending[..boundary].to_owned();
            pending.drain(..boundary + 2);
            for line in event.lines() {
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data == "[DONE]" {
                    continue;
                }
                let payload: Value = serde_json::from_str(data)
                    .map_err(|error| format!("provider stream parse failed: {error}"))?;
                if let Some(delta) = parse(&payload, &mut text, &mut stop_reason) {
                    let _ = sink.emit(ModelStreamFrame::TextDelta(delta)).await;
                }
            }
        }
    }
    let _ = sink
        .emit(ModelStreamFrame::Final(ModelGenerateResponse {
            content: Content {
                text,
                attachments: None,
                metadata: None,
            },
            tool_calls: None,
            usage: TokenUsage::default(),
            stop_reason,
        }))
        .await;
    Ok(())
}
