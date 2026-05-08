//! JSON adapters for the runtime persistence layer.
//!
//! Translates between the runtime's internal `DataValue` / `Content` /
//! `TaskResult` types and `serde_json::Value`, and back. Lives next to
//! `runtime.rs` so the runtime stays focused on the execution loop.

use std::collections::BTreeMap;

use crate::model::ToolCall;
use crate::persistence::{Step, StepStatus};
use crate::primitives::{
    Attachment, AttachmentType, Content, DataValue, TaskResult, TaskStatus,
};

pub(crate) fn tool_step_input_json(tool_call: &ToolCall) -> serde_json::Value {
    serde_json::json!({
        "name": tool_call.name,
        "args": data_value_to_json(&DataValue::Object(tool_call.args.clone())),
    })
}

pub(crate) fn tool_step_output_json(result: &TaskResult<Content>) -> serde_json::Value {
    data_value_to_json(&task_result_data_value(result))
}

pub(crate) fn persisted_task_result(
    step: &Step,
) -> Result<Option<TaskResult<Content>>, String> {
    match step.status {
        StepStatus::Pending => Ok(None),
        StepStatus::Done | StepStatus::Failed => {
            let output = step.output.as_ref().ok_or_else(|| {
                format!(
                    "persisted step {} has terminal status without output",
                    step.id
                )
            })?;
            task_result_from_json(output)
                .ok_or_else(|| format!("persisted step {} has unreadable output", step.id))
                .map(Some)
        }
    }
}

fn task_result_from_json(value: &serde_json::Value) -> Option<TaskResult<Content>> {
    let object = value.as_object()?;
    let status = match object.get("status")?.as_str()? {
        "success" => TaskStatus::Success,
        "error" => TaskStatus::Error,
        _ => return None,
    };
    let data = match object.get("data") {
        Some(serde_json::Value::Null) | None => None,
        Some(content) => content_from_json(content),
    };
    let error = object
        .get("error")
        .and_then(|error| error.as_str().map(ToOwned::to_owned));
    let duration_ms = object.get("durationMs").and_then(json_u64).unwrap_or(0);

    Some(TaskResult {
        status,
        data,
        error,
        duration_ms,
    })
}

fn content_from_json(value: &serde_json::Value) -> Option<Content> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::String(text) => Some(Content {
            text: text.clone(),
            ..Content::default()
        }),
        serde_json::Value::Object(object) => Some(Content {
            text: object
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            attachments: object.get("attachments").and_then(attachments_from_json),
            metadata: object.get("metadata").and_then(metadata_from_json),
        }),
        _ => None,
    }
}

fn attachments_from_json(value: &serde_json::Value) -> Option<Vec<Attachment>> {
    let values = value.as_array()?;
    let attachments = values
        .iter()
        .filter_map(attachment_from_json)
        .collect::<Vec<_>>();

    if attachments.is_empty() {
        None
    } else {
        Some(attachments)
    }
}

fn attachment_from_json(value: &serde_json::Value) -> Option<Attachment> {
    let object = value.as_object()?;
    let attachment_type = attachment_type_from_str(object.get("type")?.as_str()?)?;
    let name = object.get("name")?.as_str()?.to_string();
    let data = object.get("data")?.as_str()?.to_string();

    Some(Attachment {
        attachment_type,
        name,
        data,
    })
}

fn attachment_type_from_str(value: &str) -> Option<AttachmentType> {
    match value {
        "file" => Some(AttachmentType::File),
        "image" => Some(AttachmentType::Image),
        "url" => Some(AttachmentType::Url),
        _ => None,
    }
}

fn metadata_from_json(value: &serde_json::Value) -> Option<BTreeMap<String, DataValue>> {
    match json_to_data_value(value) {
        Some(DataValue::Object(metadata)) => Some(metadata),
        _ => None,
    }
}

fn json_to_data_value(value: &serde_json::Value) -> Option<DataValue> {
    match value {
        serde_json::Value::Null => Some(DataValue::Null),
        serde_json::Value::Bool(value) => Some(DataValue::Bool(*value)),
        serde_json::Value::Number(value) => value.as_f64().map(DataValue::Number),
        serde_json::Value::String(value) => Some(DataValue::String(value.clone())),
        serde_json::Value::Array(values) => values
            .iter()
            .map(json_to_data_value)
            .collect::<Option<Vec<_>>>()
            .map(DataValue::Array),
        serde_json::Value::Object(values) => values
            .iter()
            .map(|(key, value)| json_to_data_value(value).map(|value| (key.clone(), value)))
            .collect::<Option<BTreeMap<_, _>>>()
            .map(DataValue::Object),
    }
}

fn json_u64(value: &serde_json::Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value
            .as_f64()
            .filter(|value| *value >= 0.0)
            .map(|value| value as u64)
    })
}

pub(crate) fn tool_result_text_data_value(result: &TaskResult<Content>) -> DataValue {
    match result.status {
        TaskStatus::Success => match result.data.as_ref() {
            Some(content) => DataValue::String(content.text.clone()),
            None => DataValue::Null,
        },
        TaskStatus::Error => match result.error.as_ref() {
            Some(error) => DataValue::String(error.clone()),
            None => DataValue::Null,
        },
    }
}

pub(crate) fn task_result_data_value(result: &TaskResult<Content>) -> DataValue {
    let mut value = BTreeMap::new();
    value.insert(
        "status".into(),
        DataValue::String(result.status.as_str().to_string()),
    );
    value.insert("data".into(), content_data_value(result.data.as_ref()));
    value.insert(
        "error".into(),
        match &result.error {
            Some(error) => DataValue::String(error.clone()),
            None => DataValue::Null,
        },
    );
    value.insert(
        "durationMs".into(),
        DataValue::Number(result.duration_ms as f64),
    );
    DataValue::Object(value)
}

fn content_data_value(content: Option<&Content>) -> DataValue {
    let Some(content) = content else {
        return DataValue::Null;
    };

    let mut value = BTreeMap::new();
    value.insert("text".into(), DataValue::String(content.text.clone()));
    value.insert(
        "attachments".into(),
        match content.attachments.as_deref() {
            Some(attachments) => DataValue::Array(
                attachments
                    .iter()
                    .map(|attachment| {
                        let mut attachment_value = BTreeMap::new();
                        attachment_value.insert(
                            "type".into(),
                            DataValue::String(match attachment.attachment_type {
                                AttachmentType::File => "file".into(),
                                AttachmentType::Image => "image".into(),
                                AttachmentType::Url => "url".into(),
                            }),
                        );
                        attachment_value
                            .insert("name".into(), DataValue::String(attachment.name.clone()));
                        attachment_value
                            .insert("data".into(), DataValue::String(attachment.data.clone()));
                        DataValue::Object(attachment_value)
                    })
                    .collect(),
            ),
            None => DataValue::Null,
        },
    );
    value.insert(
        "metadata".into(),
        match &content.metadata {
            Some(metadata) => DataValue::Object(metadata.clone()),
            None => DataValue::Null,
        },
    );
    DataValue::Object(value)
}

pub(crate) fn data_value_json(value: &DataValue) -> String {
    serde_json::to_string(&data_value_to_json(value)).unwrap_or_else(|_| "null".into())
}

pub(crate) fn data_value_to_json(value: &DataValue) -> serde_json::Value {
    match value {
        DataValue::Null => serde_json::Value::Null,
        DataValue::Bool(v) => serde_json::Value::Bool(*v),
        // Non-finite floats (NaN / ±Inf) are not representable in JSON. Drop
        // to null so the serializer never emits invalid JSON.
        DataValue::Number(v) => serde_json::Number::from_f64(*v)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        DataValue::String(v) => serde_json::Value::String(v.clone()),
        DataValue::Array(vs) => {
            serde_json::Value::Array(vs.iter().map(data_value_to_json).collect())
        }
        DataValue::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), data_value_to_json(v)))
                .collect(),
        ),
    }
}
