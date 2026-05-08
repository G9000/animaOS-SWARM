use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Current Unix-epoch milliseconds as `u64`.
///
/// Saturates to `u64::MAX` if the system clock is before the Unix epoch
/// (the only case where `SystemTime::duration_since` returns Err).
pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Locks a `Mutex`, recovering the inner guard if the mutex is poisoned.
///
/// Production runtimes embed user-supplied callbacks (model adapters, tool
/// handlers, agent factories) that may panic. A panic while holding one of
/// the runtime's internal mutexes poisons the lock — and the rest of the
/// runtime still has useful state. Recovering the guard via `into_inner()`
/// keeps a single bad actor from cascade-failing the entire process.
///
/// **Caveat:** recovering a poisoned guard does not restore broken
/// invariants. If the panic occurred mid-update (a counter incremented but
/// the matching list push never happened, for example), the inner state may
/// be inconsistent. Callers that hold a lock should keep their critical
/// sections short and atomic so partial updates are unlikely; this helper
/// trades "guaranteed cascade panic" for "best-effort continuation," not for
/// "guaranteed correctness."
pub fn lock_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Method-style alias for [`lock_recover`].
pub trait LockRecover<T> {
    fn lock_recover(&self) -> MutexGuard<'_, T>;
}

impl<T> LockRecover<T> for Mutex<T> {
    fn lock_recover(&self) -> MutexGuard<'_, T> {
        lock_recover(self)
    }
}

pub type UuidString = String;
pub type AgentId = String;
pub type RoomId = String;
pub type MessageId = String;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachmentType {
    File,
    Image,
    Url,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub attachment_type: AttachmentType,
    pub name: String,
    pub data: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Content {
    pub text: String,
    pub attachments: Option<Vec<Attachment>>,
    pub metadata: Option<BTreeMap<String, DataValue>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub agent_id: AgentId,
    pub room_id: RoomId,
    pub content: Content,
    pub role: MessageRole,
    pub created_at_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskResult<T> {
    pub status: TaskStatus,
    pub data: Option<T>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

impl<T> TaskResult<T> {
    pub fn success(data: T, duration_ms: u64) -> Self {
        Self {
            status: TaskStatus::Success,
            data: Some(data),
            error: None,
            duration_ms,
        }
    }

    pub fn error(message: impl Into<String>, duration_ms: u64) -> Self {
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
    use super::{TaskResult, TaskStatus};

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
