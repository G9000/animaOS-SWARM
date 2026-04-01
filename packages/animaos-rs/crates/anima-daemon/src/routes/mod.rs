mod agents;
mod health;
mod memories;

use std::sync::{Arc, Mutex};

use crate::http::{Request, Response};
use crate::state::DaemonState;

const NOT_FOUND_JSON: &str = "{\"error\":\"not found\"}";

pub(crate) async fn route_request(request: Request, state: &Arc<Mutex<DaemonState>>) -> Response {
    if request.method == "GET" && (request.path == "/health" || request.path == "/api/health") {
        return health::handle_health();
    }

    if request.method == "POST" && request.path == "/api/memories" {
        return memories::handle_create_memory(request.body, state);
    }

    if request.method == "GET" && request.path == "/api/memories/search" {
        return memories::handle_search_memories(request.query, state);
    }

    if request.method == "GET" && request.path == "/api/memories/recent" {
        return memories::handle_recent_memories(request.query, state);
    }

    agents::route_agent_request(request, state)
        .await
        .unwrap_or_else(|| Response::json("HTTP/1.1 404 Not Found", NOT_FOUND_JSON.to_string()))
}
