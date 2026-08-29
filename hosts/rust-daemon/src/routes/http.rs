use std::collections::{BTreeSet, HashMap};
use std::io::{self, Write};
use std::net::IpAddr;

use axum::body::to_bytes;
use axum::extract::{Request as AxumRequest, State};
use axum::http::{header, HeaderValue, Request as HttpRequest, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response as AxumResponse};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::ser::{CharEscape, CompactFormatter, Formatter};
use tracing::error;
use tracing::info_span;
use zeroize::Zeroizing;

use super::ApiError;

const DEFAULT_LOCAL_UI_ORIGINS: [&str; 4] = [
    "http://localhost:4200",
    "http://127.0.0.1:4200",
    "http://localhost:4201",
    "http://127.0.0.1:4201",
];
const MAX_LOCAL_ADMIN_TOKEN_BYTES: usize = 4096;

/// Immutable, startup-computed policy for operations that can mutate local
/// credentials or trigger agent/external side effects.
#[derive(Clone)]
pub(super) struct LocalOwnerPolicy {
    bind_is_loopback: bool,
    allowed_origins: BTreeSet<String>,
    admin_token: Option<Zeroizing<String>>,
}

#[derive(Clone)]
pub(super) struct ApiKeyPolicy {
    expected: Option<Zeroizing<String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LocalOwnerRejection {
    LocalAdminRequired,
    OriginRejected,
}

impl LocalOwnerPolicy {
    pub(super) fn from_env(bind_is_loopback: bool) -> Self {
        let mut allowed_origins = DEFAULT_LOCAL_UI_ORIGINS
            .into_iter()
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        if let Ok(configured) = std::env::var("ANIMA_ALLOWED_UI_ORIGINS") {
            allowed_origins.extend(
                configured
                    .split(',')
                    .filter_map(normalize_serialized_origin),
            );
        }
        let admin_token = std::env::var("ANIMA_LOCAL_ADMIN_TOKEN")
            .ok()
            .filter(|value| !value.is_empty() && value.len() <= MAX_LOCAL_ADMIN_TOKEN_BYTES)
            .map(Zeroizing::new);
        Self {
            bind_is_loopback,
            allowed_origins,
            admin_token,
        }
    }

    #[cfg(test)]
    pub(super) fn for_test(bind_is_loopback: bool, admin_token: Option<&str>) -> Self {
        Self {
            bind_is_loopback,
            allowed_origins: DEFAULT_LOCAL_UI_ORIGINS
                .into_iter()
                .map(str::to_string)
                .collect(),
            admin_token: admin_token.map(|value| Zeroizing::new(value.to_string())),
        }
    }

    pub(super) fn authorize(
        &self,
        headers: &axum::http::HeaderMap,
    ) -> Result<(), LocalOwnerRejection> {
        if !self.bind_is_loopback
            || !request_host_is_loopback(headers)
            || has_forwarding_headers(headers)
        {
            return Err(LocalOwnerRejection::LocalAdminRequired);
        }

        if let Some(origin) = headers.get(header::ORIGIN) {
            let Ok(origin) = origin.to_str() else {
                return Err(LocalOwnerRejection::OriginRejected);
            };
            return normalize_serialized_origin(origin)
                .filter(|origin| self.allowed_origins.contains(origin))
                .map(|_| ())
                .ok_or(LocalOwnerRejection::OriginRejected);
        }

        let Some(expected) = self.admin_token.as_deref() else {
            return Err(LocalOwnerRejection::LocalAdminRequired);
        };
        let Some(presented) = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
        else {
            return Err(LocalOwnerRejection::LocalAdminRequired);
        };
        if constant_time_local_admin_eq(presented.as_bytes(), expected.as_bytes()) {
            Ok(())
        } else {
            Err(LocalOwnerRejection::LocalAdminRequired)
        }
    }
}

impl ApiKeyPolicy {
    pub(super) fn from_env() -> Self {
        let expected = std::env::var("ANIMAOS_RS_API_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(Zeroizing::new);
        Self { expected }
    }

    #[cfg(test)]
    pub(super) fn for_test(expected: Option<&str>) -> Self {
        Self {
            expected: expected.map(|value| Zeroizing::new(value.to_string())),
        }
    }

    fn authorize(&self, headers: &axum::http::HeaderMap) -> bool {
        let Some(expected) = self.expected.as_deref() else {
            return true;
        };
        let authorization = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .unwrap_or_default();
        let x_api_key = headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        constant_time_eq(authorization.as_bytes(), expected.as_bytes())
            | constant_time_eq(x_api_key.as_bytes(), expected.as_bytes())
    }
}

pub(crate) fn configured_bind_is_loopback() -> bool {
    let host = std::env::var("ANIMAOS_RS_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    host_is_loopback(host.trim())
}

fn normalize_serialized_origin(origin: &str) -> Option<String> {
    let origin = origin.trim();
    if origin.is_empty() || origin == "null" || origin.contains('*') {
        return None;
    }
    let parsed = reqwest::Url::parse(origin).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
    {
        return None;
    }
    Some(parsed.origin().ascii_serialization())
}

fn request_host_is_loopback(headers: &axum::http::HeaderMap) -> bool {
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Ok(authority) = host.parse::<axum::http::uri::Authority>() else {
        return false;
    };
    host_is_loopback(authority.host())
}

fn host_is_loopback(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>().is_ok_and(|address| match address {
        IpAddr::V4(address) => address.is_loopback(),
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map_or_else(|| address.is_loopback(), |address| address.is_loopback()),
    })
}

fn has_forwarding_headers(headers: &axum::http::HeaderMap) -> bool {
    headers.keys().any(|name| {
        let name = name.as_str();
        name == "forwarded"
            || name == "via"
            || name.starts_with("x-forwarded-")
            || matches!(
                name,
                "x-real-ip" | "client-ip" | "true-client-ip" | "cf-connecting-ip"
            )
    })
}

fn constant_time_local_admin_eq(presented: &[u8], expected: &[u8]) -> bool {
    if presented.len() > MAX_LOCAL_ADMIN_TOKEN_BYTES || expected.len() > MAX_LOCAL_ADMIN_TOKEN_BYTES
    {
        return false;
    }
    let mut diff = presented.len() ^ expected.len();
    for index in 0..MAX_LOCAL_ADMIN_TOKEN_BYTES {
        diff |= usize::from(
            presented.get(index).copied().unwrap_or_default()
                ^ expected.get(index).copied().unwrap_or_default(),
        );
    }
    diff == 0
}

/// Optional API-key gate. The immutable policy is computed once while the
/// router is built.
///
/// - If the env var is **unset or empty**, all requests pass through (default,
///   matches prior behavior).
/// - If the env var is set, every request must carry a matching credential in
///   either `Authorization: Bearer <key>` or `X-Api-Key: <key>`. Health
///   probes (`/health`, `/api/health`, `/ready`, `/api/ready`, `/metrics`,
///   `/openapi.json`, `/docs`, `/docs/`) are exempt so external monitoring
///   keeps working.
pub(super) async fn enforce_api_key(
    State(policy): State<ApiKeyPolicy>,
    request: AxumRequest,
    next: Next,
) -> Result<AxumResponse, AxumResponse> {
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

    if policy.authorize(request.headers()) {
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
    let max_len = a.len().max(b.len());
    let mut diff = a.len() ^ b.len();
    for index in 0..max_len {
        diff |= usize::from(
            a.get(index).copied().unwrap_or_default() ^ b.get(index).copied().unwrap_or_default(),
        );
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
