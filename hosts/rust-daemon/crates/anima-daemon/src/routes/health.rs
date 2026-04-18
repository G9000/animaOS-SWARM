use crate::routes::contracts::HealthResponse;

pub(crate) fn handle_health() -> HealthResponse {
    HealthResponse {
        status: "ok".to_string(),
    }
}
