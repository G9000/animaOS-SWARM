use std::collections::HashMap;
use std::io::{self, Write};

use axum::body::to_bytes;
use axum::extract::Request as AxumRequest;
use axum::http::{header, HeaderValue, Request as HttpRequest, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response as AxumResponse};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::ser::{CharEscape, CompactFormatter, Formatter};
use tracing::error;
use tracing::info_span;

use super::ApiError;

/// Optional API-key gate. Reads `ANIMAOS_RS_API_KEY` once on first call and
/// caches the result for the process lifetime.
///
/// - If the env var is **unset or empty**, all requests pass through (default,
///   matches prior behavior).
/// - If the env var is set, every request must carry a matching credential in
///   either `Authorization: Bearer <key>` or `X-Api-Key: <key>`. Health
///   probes (`/health`, `/api/health`, `/ready`, `/api/ready`, `/metrics`,
///   `/openapi.json`, `/docs`, `/docs/`) are exempt so external monitoring
///   keeps working.
pub(super) async fn enforce_api_key(
    request: AxumRequest,
    next: Next,
) -> Result<AxumResponse, AxumResponse> {
    use std::sync::OnceLock;
    static EXPECTED_KEY: OnceLock<Option<String>> = OnceLock::new();
    let expected = EXPECTED_KEY.get_or_init(|| {
        std::env::var("ANIMAOS_RS_API_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    });
    let Some(expected) = expected.as_deref() else {
        return Ok(next.run(request).await);
    };

    let path = request.uri().path();
    if matches!(
        path,
        "/health"
            | "/api/health"
            | "/ready"
            | "/api/ready"
            | "/metrics"
            | "/openapi.json"
            | "/docs"
            | "/docs/"
    ) {
        return Ok(next.run(request).await);
    }

    let presented = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .or_else(|| {
            request
                .headers()
                .get("x-api-key")
                .and_then(|value| value.to_str().ok())
        })
        .unwrap_or_default();

    if constant_time_eq(presented.as_bytes(), expected.as_bytes()) {
        Ok(next.run(request).await)
    } else {
        Err(json_response(
            StatusCode::UNAUTHORIZED,
            &super::contracts::ErrorBody {
                error: "unauthorized".into(),
            },
        ))
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

const INTERNAL_SERVER_ERROR_JSON: &str = "{\"error\":\"internal server error\"}";

pub(super) fn make_http_span<B>(request: &HttpRequest<B>) -> tracing::Span {
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();

    info_span!(
        "http_request",
        method = %request.method(),
        uri = %request.uri(),
        request_id = %request_id,
    )
}

pub(in super::super) fn parse_json_body<T: DeserializeOwned>(body: Vec<u8>) -> Result<T, ApiError> {
    let body = std::str::from_utf8(&body)
        .map_err(|_| ApiError::bad_request_static("request body must be valid UTF-8"))?;
    serde_json::from_str(body)
        .map_err(|_| ApiError::bad_request_static("request body must be valid JSON"))
}

pub(super) async fn read_limited_body(
    request: AxumRequest,
    limit: usize,
) -> Result<Vec<u8>, AxumResponse> {
    to_bytes(request.into_body(), limit)
        .await
        .map(|body| body.to_vec())
        .map_err(|_| ApiError::malformed_request().into_response())
}

pub(in super::super) fn serialize_json<T: Serialize>(value: &T) -> String {
    match try_serialize_json(value) {
        Ok(body) => body,
        Err(error) => {
            error!(error = %error, "failed to serialize JSON payload");
            INTERNAL_SERVER_ERROR_JSON.to_string()
        }
    }
}

pub(super) fn json_response<T: Serialize>(status: StatusCode, value: &T) -> AxumResponse {
    match try_serialize_json(value) {
        Ok(body) => json_response_with_body(status, body),
        Err(error) => {
            error!(error = %error, "failed to serialize JSON response body");
            json_response_with_body(
                StatusCode::INTERNAL_SERVER_ERROR,
                INTERNAL_SERVER_ERROR_JSON.to_string(),
            )
        }
    }
}

fn try_serialize_json<T: Serialize>(value: &T) -> Result<String, String> {
    let mut body = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut body, ContractJsonFormatter::default());
    value
        .serialize(&mut serializer)
        .map_err(|error| error.to_string())?;
    String::from_utf8(body).map_err(|error| error.to_string())
}

fn json_response_with_body(status: StatusCode, body: String) -> AxumResponse {
    (
        status,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )],
        body,
    )
        .into_response()
}

pub(super) fn request_query(uri: &Uri) -> Result<HashMap<String, String>, ()> {
    parse_query_string(uri.query().unwrap_or_default())
}

fn parse_query_string(query: &str) -> Result<HashMap<String, String>, ()> {
    let mut params = HashMap::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        params.insert(percent_decode(key)?, percent_decode(value)?);
    }
    Ok(params)
}

fn percent_decode(value: &str) -> Result<String, ()> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' => {
                if index + 2 >= bytes.len() {
                    return Err(());
                }
                decoded.push((hex_value(bytes[index + 1])? << 4) | hex_value(bytes[index + 2])?);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8(decoded).map_err(|_| ())
}

fn hex_value(byte: u8) -> Result<u8, ()> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(()),
    }
}

#[derive(Default)]
struct ContractJsonFormatter {
    inner: CompactFormatter,
}

impl Formatter for ContractJsonFormatter {
    fn write_char_escape<W>(&mut self, writer: &mut W, char_escape: CharEscape) -> io::Result<()>
    where
        W: ?Sized + Write,
    {
        match char_escape {
            CharEscape::Backspace => writer.write_all(b"\\u0008"),
            CharEscape::FormFeed => writer.write_all(b"\\u000c"),
            CharEscape::AsciiControl(byte) => {
                write!(writer, "\\u{byte:04x}")
            }
            _ => self.inner.write_char_escape(writer, char_escape),
        }
    }
}
