use std::collections::BTreeMap;

use anima_core::{Attachment, AttachmentType, Content, DataValue, TaskResult, TokenUsage};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct ErrorBody {
    pub(crate) error: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct DeleteResponse {
    pub(crate) deleted: bool,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct HealthResponse {
    pub(crate) status: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TokenUsageResponse {
    pub(crate) prompt_tokens: u64,
    pub(crate) completion_tokens: u64,
    pub(crate) total_tokens: u64,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttachmentResponse {
    #[serde(rename = "type")]
    pub(crate) attachment_type: String,
    pub(crate) name: String,
    pub(crate) data: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContentResponse {
    pub(crate) text: String,
    pub(crate) attachments: Option<Vec<AttachmentResponse>>,
    pub(crate) metadata: Option<BTreeMap<String, Value>>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TaskResultResponse {
    pub(crate) status: String,
    pub(crate) data: Option<ContentResponse>,
    pub(crate) error: Option<String>,
    pub(crate) duration_ms: u128,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttachmentRequest {
    #[serde(rename = "type")]
    pub(crate) attachment_type: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) data: Option<String>,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TaskRequest {
    pub(crate) text: Option<String>,
    pub(crate) task: Option<String>,
    pub(crate) attachments: Option<Vec<AttachmentRequest>>,
    pub(crate) metadata: Option<BTreeMap<String, Value>>,
}

impl TaskRequest {
    pub(crate) fn into_domain(self) -> Result<Content, &'static str> {
        let text = self
            .text
            .filter(|value| !value.is_empty())
            .or_else(|| self.task.filter(|value| !value.is_empty()))
            .ok_or("text is required")?;

        let attachments = self
            .attachments
            .map(|attachments| {
                attachments
                    .into_iter()
                    .map(AttachmentRequest::into_domain)
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;
        let metadata = self
            .metadata
            .map(|metadata| {
                metadata
                    .into_iter()
                    .map(|(key, value)| {
                        Ok::<(String, DataValue), &'static str>((key, json_to_data_value(value)?))
                    })
                    .collect::<Result<BTreeMap<_, _>, _>>()
            })
            .transpose()?;

        Ok(Content {
            text,
            attachments,
            metadata,
        })
    }
}

impl AttachmentRequest {
    fn into_domain(self) -> Result<Attachment, &'static str> {
        let attachment_type = match self.attachment_type.as_deref() {
            Some("file") => AttachmentType::File,
            Some("image") => AttachmentType::Image,
            Some("url") => AttachmentType::Url,
            Some(_) => return Err("attachment type must be file, image, or url"),
            None => return Err("attachment type is required"),
        };
        let name = required_string(self.name, "attachment name is required")?;
        let data = self.data.ok_or("attachment data is required")?;

        Ok(Attachment {
            attachment_type,
            name,
            data,
        })
    }
}

impl From<&TokenUsage> for TokenUsageResponse {
    fn from(value: &TokenUsage) -> Self {
        Self {
            prompt_tokens: value.prompt_tokens,
            completion_tokens: value.completion_tokens,
            total_tokens: value.total_tokens,
        }
    }
}

impl From<&Attachment> for AttachmentResponse {
    fn from(value: &Attachment) -> Self {
        Self {
            attachment_type: match value.attachment_type {
                AttachmentType::File => "file",
                AttachmentType::Image => "image",
                AttachmentType::Url => "url",
            }
            .to_string(),
            name: value.name.clone(),
            data: value.data.clone(),
        }
    }
}

impl From<&Content> for ContentResponse {
    fn from(value: &Content) -> Self {
        Self {
            text: value.text.clone(),
            attachments: value
                .attachments
                .as_ref()
                .map(|attachments| attachments.iter().map(AttachmentResponse::from).collect()),
            metadata: value.metadata.as_ref().map(|metadata| {
                metadata
                    .iter()
                    .map(|(key, value)| (key.clone(), data_value_to_json(value)))
                    .collect()
            }),
        }
    }
}

impl From<&TaskResult<Content>> for TaskResultResponse {
    fn from(value: &TaskResult<Content>) -> Self {
        Self {
            status: value.status.as_str().to_string(),
            data: value.data.as_ref().map(ContentResponse::from),
            error: value.error.clone(),
            duration_ms: value.duration_ms,
        }
    }
}

pub(in crate::routes::contracts) fn required_string(
    value: Option<String>,
    message: &'static str,
) -> Result<String, &'static str> {
    match value {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(message),
    }
}

pub(in crate::routes::contracts) fn parse_usize(value: &str) -> Result<usize, &'static str> {
    value
        .parse::<usize>()
        .map_err(|_| "limit must be an integer")
}

pub(in crate::routes::contracts) fn parse_importance(value: &str) -> Result<f64, &'static str> {
    let parsed = value
        .parse::<f64>()
        .map_err(|_| "minImportance must be a number")?;
    if parsed.is_finite() && (0.0..=1.0).contains(&parsed) {
        Ok(parsed)
    } else {
        Err("minImportance must be between 0 and 1")
    }
}

pub(in crate::routes::contracts) fn number_value(
    value: Value,
    field: &'static str,
) -> Result<f64, &'static str> {
    match value {
        Value::Number(number) => number.as_f64().ok_or(match field {
            "temperature" => "temperature must be a number",
            _ => "field must be a number",
        }),
        _ => Err(match field {
            "temperature" => "temperature must be a number",
            _ => "field must be a number",
        }),
    }
}

pub(in crate::routes::contracts) fn u32_value(
    value: Value,
    field: &'static str,
) -> Result<u32, &'static str> {
    match value {
        Value::Number(number) => number
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .ok_or(match field {
                "maxTokens" => "maxTokens must be a positive integer",
                "maxRetries" => "maxRetries must be a positive integer",
                _ => "field must be a positive integer",
            }),
        _ => Err(match field {
            "maxTokens" => "maxTokens must be a positive integer",
            "maxRetries" => "maxRetries must be a positive integer",
            _ => "field must be a positive integer",
        }),
    }
}

pub(in crate::routes::contracts) fn u64_value(
    value: Value,
    field: &'static str,
) -> Result<u64, &'static str> {
    match value {
        Value::Number(number) => number.as_u64().ok_or(match field {
            "timeout" => "timeout must be a positive integer",
            _ => "field must be a positive integer",
        }),
        _ => Err(match field {
            "timeout" => "timeout must be a positive integer",
            _ => "field must be a positive integer",
        }),
    }
}

pub(in crate::routes::contracts) fn json_to_data_value(
    value: Value,
) -> Result<DataValue, &'static str> {
    match value {
        Value::Null => Ok(DataValue::Null),
        Value::Bool(value) => Ok(DataValue::Bool(value)),
        Value::Number(value) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(DataValue::Number)
            .ok_or("settings values must be finite"),
        Value::String(value) => Ok(DataValue::String(value)),
        Value::Array(values) => values
            .into_iter()
            .map(json_to_data_value)
            .collect::<Result<Vec<_>, _>>()
            .map(DataValue::Array),
        Value::Object(values) => values
            .into_iter()
            .map(|(key, value)| {
                Ok::<(String, DataValue), &'static str>((key, json_to_data_value(value)?))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map(DataValue::Object),
    }
}

pub(in crate::routes::contracts) fn data_value_to_json(value: &DataValue) -> Value {
    match value {
        DataValue::Null => Value::Null,
        DataValue::Bool(value) => Value::Bool(*value),
        DataValue::Number(value) => serde_json::Number::from_f64(*value)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        DataValue::String(value) => Value::String(value.clone()),
        DataValue::Array(values) => Value::Array(values.iter().map(data_value_to_json).collect()),
        DataValue::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), data_value_to_json(value)))
                .collect(),
        ),
    }
}
