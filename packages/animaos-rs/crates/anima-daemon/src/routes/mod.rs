mod health;
mod memories;

use std::sync::{Arc, Mutex};

use crate::http::{Request, Response};
use crate::state::DaemonState;

const NOT_FOUND_JSON: &str = "{\"error\":\"not found\"}";

pub(crate) fn route_request(request: Request, state: &Arc<Mutex<DaemonState>>) -> Response {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") | ("GET", "/api/health") => health::handle_health(),
        ("POST", "/api/memories") => memories::handle_create_memory(request.body, state),
        ("GET", "/api/memories/search") => memories::handle_search_memories(request.query, state),
        ("GET", "/api/memories/recent") => memories::handle_recent_memories(request.query, state),
        _ => Response::json("HTTP/1.1 404 Not Found", NOT_FOUND_JSON.to_string()),
    }
}
