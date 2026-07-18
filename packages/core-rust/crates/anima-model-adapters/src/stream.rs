use std::collections::{btree_map::Entry, BTreeMap};

use anima_core::{
    AgentConfig, Content, DataValue, ModelAdapter, ModelGenerateRequest, ModelGenerateResponse,
    ModelStopReason, ModelStreamFrame, ModelStreamSink, TokenUsage, ToolCall,
    MAX_CAPABILITY_ARGUMENT_BYTES, MAX_CAPABILITY_ID_BYTES,
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
    let mut accumulator = StreamAccumulator::default();
    consume_sse_events(response, |payload| accumulator.openai(payload), sink).await?;
    sink.emit(ModelStreamFrame::Final(accumulator.finish()?))
        .await
        .map_err(|_| "provider stream consumer failed".to_owned())
}

pub(crate) async fn consume_anthropic_sse(
    response: reqwest::Response,
    sink: &dyn ModelStreamSink,
) -> Result<(), String> {
    let mut accumulator = StreamAccumulator::default();
    consume_sse_events(response, |payload| accumulator.anthropic(payload), sink).await?;
    sink.emit(ModelStreamFrame::Final(accumulator.finish()?))
        .await
        .map_err(|_| "provider stream consumer failed".to_owned())
}

const MAX_STREAM_BYTES: usize = 4 * 1024 * 1024;
const MAX_STREAM_EVENT_BYTES: usize = 256 * 1024;
const MAX_STREAM_TEXT_BYTES: usize = 1024 * 1024;
const MAX_STREAM_TOOL_CALLS: usize = 32;

#[derive(Default)]
struct StreamAccumulator {
    text: String,
    stop_reason: Option<ModelStopReason>,
    usage: StreamUsage,
    tools: BTreeMap<usize, ToolCallAccumulator>,
}

#[derive(Default)]
struct StreamUsage {
    prompt: Option<u64>,
    completion: Option<u64>,
    total: Option<u64>,
}

#[derive(Default)]
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
    kind: Option<String>,
    started: bool,
    stopped: bool,
}

impl StreamAccumulator {
    fn openai(&mut self, payload: &Value) -> Result<Option<String>, String> {
        self.usage.merge_openai(payload.get("usage"))?;
        let choices = payload
            .get("choices")
            .and_then(Value::as_array)
            .ok_or_else(stream_parse_error)?;
        if choices.is_empty() {
            return Ok(None);
        }
        if choices.len() != 1 || choices[0].get("index").and_then(Value::as_u64).unwrap_or(0) != 0 {
            return Err(stream_parse_error());
        }
        let choice = &choices[0];
        if let Some(reason) = choice.get("finish_reason").filter(|value| !value.is_null()) {
            self.set_stop_reason(parse_stop_reason(reason.as_str(), false)?)?;
        }
        let delta = choice
            .get("delta")
            .and_then(Value::as_object)
            .ok_or_else(stream_parse_error)?;
        let text_delta = match delta.get("content") {
            Some(Value::String(delta)) => {
                append_bounded(&mut self.text, delta, MAX_STREAM_TEXT_BYTES)?;
                Some(delta.clone())
            }
            Some(Value::Null) | None => None,
            _ => return Err(stream_parse_error()),
        };
        if let Some(tool_chunks) = delta.get("tool_calls") {
            let tool_chunks = tool_chunks.as_array().ok_or_else(stream_tool_error)?;
            for chunk in tool_chunks {
                let index = bounded_index(chunk.get("index"))?;
                let tool = self.tool(index)?;
                if let Some(kind) = chunk.get("type") {
                    tool.set_kind(kind.as_str().ok_or_else(stream_tool_error)?)?;
                }
                if let Some(id) = chunk.get("id") {
                    append_bounded(
                        &mut tool.id,
                        id.as_str().ok_or_else(stream_tool_error)?,
                        MAX_CAPABILITY_ID_BYTES,
                    )?;
                }
                if let Some(function) = chunk.get("function") {
                    let function = function.as_object().ok_or_else(stream_tool_error)?;
                    if let Some(name) = function.get("name") {
                        append_bounded(
                            &mut tool.name,
                            name.as_str().ok_or_else(stream_tool_error)?,
                            MAX_CAPABILITY_ID_BYTES,
                        )?;
                    }
                    if let Some(arguments) = function.get("arguments") {
                        append_bounded(
                            &mut tool.arguments,
                            arguments.as_str().ok_or_else(stream_tool_error)?,
                            MAX_CAPABILITY_ARGUMENT_BYTES,
                        )?;
                    }
                }
                tool.started = true;
            }
        }
        Ok(text_delta)
    }

    fn anthropic(&mut self, payload: &Value) -> Result<Option<String>, String> {
        let event_type = payload
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(stream_parse_error)?;
        match event_type {
            "ping" | "message_stop" => Ok(None),
            "message_start" => {
                self.usage.merge_anthropic_start(
                    payload
                        .get("message")
                        .and_then(|message| message.get("usage")),
                )?;
                Ok(None)
            }
            "content_block_start" => {
                let index = bounded_index(payload.get("index"))?;
                let block = payload
                    .get("content_block")
                    .and_then(Value::as_object)
                    .ok_or_else(stream_tool_error)?;
                if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                    return Ok(None);
                }
                let tool = self.tool(index)?;
                if tool.started {
                    return Err(stream_tool_error());
                }
                tool.set_kind("tool_use")?;
                tool.id = bounded_string(block.get("id"), MAX_CAPABILITY_ID_BYTES)?;
                tool.name = bounded_string(block.get("name"), MAX_CAPABILITY_ID_BYTES)?;
                let input = block.get("input").ok_or_else(stream_tool_error)?;
                if !input.is_object() {
                    return Err(stream_tool_error());
                }
                if input.as_object().is_some_and(|input| !input.is_empty()) {
                    tool.arguments =
                        serde_json::to_string(input).map_err(|_| stream_tool_error())?;
                }
                tool.started = true;
                Ok(None)
            }
            "content_block_delta" => {
                let index = bounded_index(payload.get("index"))?;
                let delta = payload
                    .get("delta")
                    .and_then(Value::as_object)
                    .ok_or_else(stream_parse_error)?;
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        let value = bounded_string(delta.get("text"), MAX_STREAM_TEXT_BYTES)?;
                        append_bounded(&mut self.text, &value, MAX_STREAM_TEXT_BYTES)?;
                        Ok(Some(value))
                    }
                    Some("input_json_delta") => {
                        let partial = bounded_string(
                            delta.get("partial_json"),
                            MAX_CAPABILITY_ARGUMENT_BYTES,
                        )?;
                        let tool = self.tools.get_mut(&index).ok_or_else(stream_tool_error)?;
                        if !tool.started || tool.stopped {
                            return Err(stream_tool_error());
                        }
                        append_bounded(
                            &mut tool.arguments,
                            &partial,
                            MAX_CAPABILITY_ARGUMENT_BYTES,
                        )?;
                        Ok(None)
                    }
                    _ => Err(stream_parse_error()),
                }
            }
            "content_block_stop" => {
                let index = bounded_index(payload.get("index"))?;
                if let Some(tool) = self.tools.get_mut(&index) {
                    if !tool.started || tool.stopped {
                        return Err(stream_tool_error());
                    }
                    tool.stopped = true;
                }
                Ok(None)
            }
            "message_delta" => {
                self.usage.merge_anthropic_delta(payload.get("usage"))?;
                if let Some(reason) = payload
                    .get("delta")
                    .and_then(|delta| delta.get("stop_reason"))
                    .filter(|reason| !reason.is_null())
                {
                    self.set_stop_reason(parse_stop_reason(reason.as_str(), true)?)?;
                }
                Ok(None)
            }
            "error" => Err("provider stream failed".to_owned()),
            _ => Err(stream_parse_error()),
        }
    }

    fn tool(&mut self, index: usize) -> Result<&mut ToolCallAccumulator, String> {
        if index >= MAX_STREAM_TOOL_CALLS {
            return Err(stream_tool_error());
        }
        if !self.tools.contains_key(&index) && self.tools.len() >= MAX_STREAM_TOOL_CALLS {
            return Err(stream_tool_error());
        }
        Ok(match self.tools.entry(index) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(ToolCallAccumulator::default()),
        })
    }

    fn set_stop_reason(&mut self, reason: ModelStopReason) -> Result<(), String> {
        if self.stop_reason.is_some_and(|existing| existing != reason) {
            return Err(stream_parse_error());
        }
        self.stop_reason = Some(reason);
        Ok(())
    }

    fn finish(self) -> Result<ModelGenerateResponse, String> {
        let mut calls = Vec::with_capacity(self.tools.len());
        for (_, tool) in self.tools {
            if !tool.started
                || tool.id.trim().is_empty()
                || tool.name.trim().is_empty()
                || !matches!(tool.kind.as_deref(), Some("function" | "tool_use"))
                || tool.kind.as_deref() == Some("tool_use") && !tool.stopped
            {
                return Err(stream_tool_error());
            }
            let arguments: Value =
                serde_json::from_str(&tool.arguments).map_err(|_| stream_tool_error())?;
            let arguments = arguments.as_object().ok_or_else(stream_tool_error)?;
            let args = arguments
                .iter()
                .map(|(key, value)| {
                    json_to_data_value(value)
                        .map(|value| (key.clone(), value))
                        .ok_or_else(stream_tool_error)
                })
                .collect::<Result<BTreeMap<_, _>, _>>()?;
            calls.push(ToolCall {
                id: tool.id,
                name: tool.name,
                args,
            });
        }
        let usage = self.usage.finish()?;
        Ok(ModelGenerateResponse {
            content: Content {
                text: self.text,
                attachments: None,
                metadata: None,
            },
            tool_calls: (!calls.is_empty()).then_some(calls),
            usage,
            stop_reason: self.stop_reason.unwrap_or(ModelStopReason::End),
        })
    }
}

impl ToolCallAccumulator {
    fn set_kind(&mut self, kind: &str) -> Result<(), String> {
        if !matches!(kind, "function" | "tool_use")
            || self.kind.as_deref().is_some_and(|current| current != kind)
        {
            return Err(stream_tool_error());
        }
        self.kind = Some(kind.to_owned());
        Ok(())
    }
}

impl StreamUsage {
    fn merge_openai(&mut self, usage: Option<&Value>) -> Result<(), String> {
        let Some(usage) = usage else { return Ok(()) };
        self.merge(
            usage.get("prompt_tokens"),
            usage.get("completion_tokens"),
            usage.get("total_tokens"),
        )
    }

    fn merge_anthropic_start(&mut self, usage: Option<&Value>) -> Result<(), String> {
        let Some(usage) = usage else { return Ok(()) };
        self.merge(usage.get("input_tokens"), None, None)
    }

    fn merge_anthropic_delta(&mut self, usage: Option<&Value>) -> Result<(), String> {
        let Some(usage) = usage else { return Ok(()) };
        self.merge(None, usage.get("output_tokens"), None)
    }

    fn merge(
        &mut self,
        prompt: Option<&Value>,
        completion: Option<&Value>,
        total: Option<&Value>,
    ) -> Result<(), String> {
        merge_usage_value(&mut self.prompt, prompt)?;
        merge_usage_value(&mut self.completion, completion)?;
        merge_usage_value(&mut self.total, total)
    }

    fn finish(self) -> Result<TokenUsage, String> {
        let prompt_tokens = self.prompt.unwrap_or(0);
        let completion_tokens = self.completion.unwrap_or(0);
        let computed = prompt_tokens
            .checked_add(completion_tokens)
            .ok_or_else(stream_parse_error)?;
        let total_tokens = self.total.unwrap_or(computed);
        if self.total.is_some() && total_tokens != computed {
            return Err(stream_parse_error());
        }
        Ok(TokenUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens,
        })
    }
}

fn merge_usage_value(target: &mut Option<u64>, value: Option<&Value>) -> Result<(), String> {
    let Some(value) = value else { return Ok(()) };
    let value = value.as_u64().ok_or_else(stream_parse_error)?;
    if target.is_some_and(|current| current != value) {
        return Err(stream_parse_error());
    }
    *target = Some(value);
    Ok(())
}

fn parse_stop_reason(value: Option<&str>, anthropic: bool) -> Result<ModelStopReason, String> {
    match (value, anthropic) {
        (Some("length" | "max_tokens"), _) => Ok(ModelStopReason::MaxTokens),
        (Some("tool_calls"), false) | (Some("tool_use"), true) => Ok(ModelStopReason::ToolCall),
        (Some("stop"), false) | (Some("end_turn" | "stop_sequence"), true) => {
            Ok(ModelStopReason::End)
        }
        _ => Err(stream_parse_error()),
    }
}

fn bounded_index(value: Option<&Value>) -> Result<usize, String> {
    let index = value
        .and_then(Value::as_u64)
        .and_then(|index| usize::try_from(index).ok())
        .ok_or_else(stream_tool_error)?;
    (index < MAX_STREAM_TOOL_CALLS)
        .then_some(index)
        .ok_or_else(stream_tool_error)
}

fn bounded_string(value: Option<&Value>, maximum: usize) -> Result<String, String> {
    let value = value
        .and_then(Value::as_str)
        .ok_or_else(stream_tool_error)?;
    if value.len() > maximum {
        return Err(stream_tool_error());
    }
    Ok(value.to_owned())
}

fn append_bounded(target: &mut String, value: &str, maximum: usize) -> Result<(), String> {
    if target.len().saturating_add(value.len()) > maximum {
        return Err(stream_tool_error());
    }
    target.push_str(value);
    Ok(())
}

fn json_to_data_value(value: &Value) -> Option<DataValue> {
    match value {
        Value::Null => Some(DataValue::Null),
        Value::Bool(value) => Some(DataValue::Bool(*value)),
        Value::Number(value) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(DataValue::Number),
        Value::String(value) => Some(DataValue::String(value.clone())),
        Value::Array(values) => values
            .iter()
            .map(json_to_data_value)
            .collect::<Option<Vec<_>>>()
            .map(DataValue::Array),
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| json_to_data_value(value).map(|value| (key.clone(), value)))
            .collect::<Option<BTreeMap<_, _>>>()
            .map(DataValue::Object),
    }
}

fn stream_parse_error() -> String {
    "provider stream parse failed".to_owned()
}

fn stream_tool_error() -> String {
    "provider stream tool call invalid".to_owned()
}

async fn consume_sse_events(
    response: reqwest::Response,
    mut parse: impl FnMut(&Value) -> Result<Option<String>, String>,
    sink: &dyn ModelStreamSink,
) -> Result<(), String> {
    let mut body = response.bytes_stream();
    let mut pending = Vec::new();
    let mut total_bytes = 0usize;
    while let Some(chunk) = body.next().await {
        let chunk = chunk
            .map_err(|error| format!("provider stream read failed: {}", error.without_url()))?;
        total_bytes = total_bytes.saturating_add(chunk.len());
        if total_bytes > MAX_STREAM_BYTES
            || pending.len().saturating_add(chunk.len()) > MAX_STREAM_EVENT_BYTES
        {
            return Err(stream_parse_error());
        }
        pending.extend_from_slice(&chunk);
        while let Some((boundary, delimiter)) = event_boundary(&pending) {
            let event = std::str::from_utf8(&pending[..boundary])
                .map_err(|_| stream_parse_error())?
                .to_owned();
            pending.drain(..boundary + delimiter);
            for line in event.lines() {
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data == "[DONE]" {
                    continue;
                }
                let payload: Value =
                    serde_json::from_str(data).map_err(|_| stream_parse_error())?;
                if let Some(delta) = parse(&payload)? {
                    let _ = sink.emit(ModelStreamFrame::TextDelta(delta)).await;
                }
            }
        }
    }
    if pending.iter().any(|byte| !byte.is_ascii_whitespace()) {
        return Err(stream_parse_error());
    }
    Ok(())
}

fn event_boundary(bytes: &[u8]) -> Option<(usize, usize)> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| (index, 4))
        .or_else(|| {
            bytes
                .windows(2)
                .position(|window| window == b"\n\n")
                .map(|index| (index, 2))
        })
}
