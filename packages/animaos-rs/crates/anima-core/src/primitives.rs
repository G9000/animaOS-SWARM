use std::collections::BTreeMap;

pub type UuidString = String;
pub type AgentId = String;
pub type RoomId = String;
pub type MessageId = String;

pub const HEALTH_OK_JSON: &str = "{\"status\":\"ok\"}";

#[derive(Clone, Debug, PartialEq)]
pub enum DataValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<DataValue>),
    Object(BTreeMap<String, DataValue>),
}

impl Default for DataValue {
    fn default() -> Self {
        Self::Null
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentType {
    File,
    Image,
    Url,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub attachment_type: AttachmentType,
    pub name: String,
    pub data: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Content {
    pub text: String,
    pub attachments: Option<Vec<Attachment>>,
    pub metadata: Option<BTreeMap<String, DataValue>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub id: MessageId,
    pub agent_id: AgentId,
    pub room_id: RoomId,
    pub content: Content,
    pub role: MessageRole,
    pub created_at: u128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskStatus {
    Success,
    Error,
}

impl TaskStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HealthStatus {
    pub status: &'static str,
}

impl HealthStatus {
    pub const fn ok() -> Self {
        Self { status: "ok" }
    }

    pub const fn as_json(self) -> &'static str {
        HEALTH_OK_JSON
    }
}

impl Default for HealthStatus {
    fn default() -> Self {
        Self::ok()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TaskResult<T> {
    pub status: TaskStatus,
    pub data: Option<T>,
    pub error: Option<String>,
    pub duration_ms: u128,
}

impl<T> TaskResult<T> {
    pub fn success(data: T, duration_ms: u128) -> Self {
        Self {
            status: TaskStatus::Success,
            data: Some(data),
            error: None,
            duration_ms,
        }
    }

    pub fn error(message: impl Into<String>, duration_ms: u128) -> Self {
        Self {
            status: TaskStatus::Error,
            data: None,
            error: Some(message.into()),
            duration_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HealthStatus, TaskResult, TaskStatus, HEALTH_OK_JSON};

    #[test]
    fn health_status_matches_existing_daemon_contract() {
        assert_eq!(HealthStatus::ok().status, "ok");
        assert_eq!(HealthStatus::ok().as_json(), HEALTH_OK_JSON);
    }

    #[test]
    fn task_result_builders_set_expected_status() {
        let success = TaskResult::success("done", 42);
        assert_eq!(success.status, TaskStatus::Success);
        assert_eq!(success.data, Some("done"));
        assert_eq!(success.error, None);

        let error: TaskResult<()> = TaskResult::error("boom", 7);
        assert_eq!(error.status, TaskStatus::Error);
        assert_eq!(error.error.as_deref(), Some("boom"));
    }
}
