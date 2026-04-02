use crate::json::escape_json;

pub(crate) struct Response {
    pub(crate) status_line: &'static str,
    pub(crate) body: String,
}

impl Response {
    pub(crate) fn json(status_line: &'static str, body: String) -> Self {
        Self { status_line, body }
    }

    pub(crate) fn error(status_line: &'static str, message: &'static str) -> Self {
        Self::json(
            status_line,
            format!("{{\"error\":\"{}\"}}", escape_json(message)),
        )
    }
}
