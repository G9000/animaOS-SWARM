pub type AgentId = String;

pub const HEALTH_OK_JSON: &str = "{\"status\":\"ok\"}";

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskResult<T> {
    pub status: &'static str,
    pub data: Option<T>,
    pub error: Option<String>,
    pub duration_ms: u128,
}
