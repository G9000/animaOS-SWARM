use anima_core::HealthStatus;

use crate::http::Response;

pub(crate) fn handle_health() -> Response {
    Response::json("HTTP/1.1 200 OK", HealthStatus::ok().as_json().to_string())
}
