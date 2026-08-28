use std::fmt;
use std::time::Duration;

use futures::StreamExt;
use reqwest::redirect::Policy;
use reqwest::{Client, Response, Url};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::credentials::TelegramBotToken;
use super::{TelegramBotIdentity, TelegramChatKind, TelegramChatMetadata, TelegramSenderMetadata};

const TELEGRAM_API_BASE: &str = "https://api.telegram.org/";
const DEFAULT_MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const DEFAULT_LONG_POLL_SECONDS: u64 = 30;
const MAX_CHAT_ID_LENGTH: usize = 32;

pub(crate) const TELEGRAM_MESSAGE_CHARACTER_LIMIT: usize = 4096;

#[derive(Clone, Copy)]
struct TelegramClientConfig {
    connect_timeout: Duration,
    request_timeout: Duration,
    long_poll_request_timeout: Duration,
    long_poll_seconds: u64,
    max_response_bytes: usize,
}

impl Default for TelegramClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(15),
            long_poll_request_timeout: Duration::from_secs(40),
            long_poll_seconds: DEFAULT_LONG_POLL_SECONDS,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
        }
    }
}

impl TelegramClientConfig {
    #[cfg(test)]
    fn test_defaults() -> Self {
        Self::for_tests(Duration::from_secs(2), DEFAULT_MAX_RESPONSE_BYTES)
    }

    #[cfg(test)]
    fn for_tests(request_timeout: Duration, max_response_bytes: usize) -> Self {
        Self {
            connect_timeout: request_timeout,
            request_timeout,
            long_poll_request_timeout: request_timeout,
            long_poll_seconds: 1,
            max_response_bytes,
        }
    }
}

pub(crate) struct TelegramClient {
    client: Client,
    base_url: Url,
    config: TelegramClientConfig,
}

impl TelegramClient {
    pub(crate) fn new() -> Result<Self, TelegramTransportError> {
        let base_url =
            Url::parse(TELEGRAM_API_BASE).map_err(|_| TelegramTransportError::Configuration)?;
        Self::build(base_url, TelegramClientConfig::default())
    }

    fn build(base_url: Url, config: TelegramClientConfig) -> Result<Self, TelegramTransportError> {
        if config.connect_timeout.is_zero()
            || config.request_timeout.is_zero()
            || config.long_poll_request_timeout.is_zero()
            || config.long_poll_seconds == 0
            || config.max_response_bytes == 0
        {
            return Err(TelegramTransportError::Configuration);
        }
        let client = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(config.connect_timeout)
            .build()
            .map_err(|_| TelegramTransportError::Configuration)?;
        Ok(Self {
            client,
            base_url,
            config,
        })
    }

    #[cfg(test)]
    fn for_test_base(
        base_url: &str,
        config: TelegramClientConfig,
    ) -> Result<Self, TelegramTransportError> {
        let mut base_url =
            Url::parse(base_url).map_err(|_| TelegramTransportError::Configuration)?;
        if base_url.cannot_be_a_base() || !matches!(base_url.scheme(), "http" | "https") {
            return Err(TelegramTransportError::Configuration);
        }
        base_url.set_query(None);
        base_url.set_fragment(None);
        if !base_url.path().ends_with('/') {
            base_url.set_path(&format!("{}/", base_url.path()));
        }
        Self::build(base_url, config)
    }

    #[cfg(test)]
    fn test_base(&self) -> &str {
        self.base_url.as_str()
    }

    pub(crate) async fn get_me(
        &self,
        token: &TelegramBotToken,
    ) -> Result<TelegramBotIdentity, TelegramTransportError> {
        let response: TelegramUser = self
            .post(
                token,
                "getMe",
                &EmptyRequest {},
                self.config.request_timeout,
            )
            .await?;
        response
            .into_bot_identity()
            .ok_or(TelegramTransportError::InvalidResponse)
    }

    pub(crate) async fn get_updates(
        &self,
        token: &TelegramBotToken,
        offset: i64,
    ) -> Result<Vec<TelegramTextUpdate>, TelegramTransportError> {
        let updates: Vec<TelegramUpdate> = self
            .post(
                token,
                "getUpdates",
                &GetUpdatesRequest {
                    offset,
                    timeout: self.config.long_poll_seconds,
                    allowed_updates: ["message"],
                },
                self.config.long_poll_request_timeout,
            )
            .await?;
        Ok(updates
            .into_iter()
            .filter_map(TelegramUpdate::into_text_update)
            .collect())
    }

    pub(crate) async fn send_message(
        &self,
        token: &TelegramBotToken,
        chat_id: &str,
        text: &str,
    ) -> Result<Vec<TelegramSentMessage>, TelegramTransportError> {
        validate_chat_id(chat_id)?;
        let chunks = chunk_telegram_text(text);
        if chunks.is_empty() {
            return Err(TelegramTransportError::InvalidMessage);
        }

        let mut sent = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            let message: TelegramMessage = self
                .post(
                    token,
                    "sendMessage",
                    &SendMessageRequest {
                        chat_id,
                        text: chunk,
                    },
                    self.config.request_timeout,
                )
                .await?;
            sent.push(TelegramSentMessage {
                message_id: message.message_id.to_string(),
                chat: message
                    .chat
                    .into_metadata()
                    .ok_or(TelegramTransportError::InvalidResponse)?,
            });
        }
        Ok(sent)
    }

    async fn post<RequestBody, ResultBody>(
        &self,
        token: &TelegramBotToken,
        method: &str,
        body: &RequestBody,
        timeout: Duration,
    ) -> Result<ResultBody, TelegramTransportError>
    where
        RequestBody: Serialize + ?Sized,
        ResultBody: DeserializeOwned,
    {
        let endpoint = self.endpoint(token, method)?;
        let response = self
            .client
            .post(endpoint)
            .timeout(timeout)
            .json(body)
            .send()
            .await
            .map_err(map_reqwest_error)?;
        self.decode_response(response).await
    }

    fn endpoint(
        &self,
        token: &TelegramBotToken,
        method: &str,
    ) -> Result<Url, TelegramTransportError> {
        let mut endpoint = self.base_url.clone();
        let bot_segment = Zeroizing::new(format!("bot{}", token.expose()));
        endpoint
            .path_segments_mut()
            .map_err(|_| TelegramTransportError::Configuration)?
            .pop_if_empty()
            .push(&bot_segment)
            .push(method);
        Ok(endpoint)
    }

    async fn decode_response<T: DeserializeOwned>(
        &self,
        response: Response,
    ) -> Result<T, TelegramTransportError> {
        let status = response.status();
        if !status.is_success() {
            return Err(TelegramTransportError::HttpStatus {
                status: status.as_u16(),
            });
        }
        if response
            .content_length()
            .is_some_and(|length| length > self.config.max_response_bytes as u64)
        {
            return Err(TelegramTransportError::ResponseTooLarge);
        }

        let mut body = Zeroizing::new(Vec::new());
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(map_reqwest_error)?;
            let next_len = body
                .len()
                .checked_add(chunk.len())
                .ok_or(TelegramTransportError::ResponseTooLarge)?;
            if next_len > self.config.max_response_bytes {
                return Err(TelegramTransportError::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }

        let envelope: TelegramEnvelope<T> =
            serde_json::from_slice(&body).map_err(|_| TelegramTransportError::InvalidResponse)?;
        if !envelope.ok {
            return Err(TelegramTransportError::UpstreamApi {
                code: envelope.error_code,
            });
        }
        envelope
            .result
            .ok_or(TelegramTransportError::InvalidResponse)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub(crate) enum TelegramTransportError {
    Configuration,
    Transport,
    Timeout,
    HttpStatus { status: u16 },
    ResponseTooLarge,
    InvalidResponse,
    UpstreamApi { code: Option<i64> },
    InvalidChatId,
    InvalidMessage,
}

impl TelegramTransportError {
    #[cfg(test)]
    const ALL: &'static [Self] = &[
        Self::Configuration,
        Self::Transport,
        Self::Timeout,
        Self::HttpStatus { status: 500 },
        Self::ResponseTooLarge,
        Self::InvalidResponse,
        Self::UpstreamApi { code: Some(401) },
        Self::InvalidChatId,
        Self::InvalidMessage,
    ];
}

impl fmt::Display for TelegramTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration => formatter.write_str("Telegram transport configuration failed"),
            Self::Transport => formatter.write_str("Telegram transport request failed"),
            Self::Timeout => formatter.write_str("Telegram transport request timed out"),
            Self::HttpStatus { status } => {
                write!(formatter, "Telegram returned HTTP status {status}")
            }
            Self::ResponseTooLarge => {
                formatter.write_str("Telegram response exceeded the size limit")
            }
            Self::InvalidResponse => formatter.write_str("Telegram returned an invalid response"),
            Self::UpstreamApi { code: Some(code) } => write!(
                formatter,
                "Telegram API rejected the request with code {code}"
            ),
            Self::UpstreamApi { code: None } => {
                formatter.write_str("Telegram API rejected the request")
            }
            Self::InvalidChatId => formatter.write_str("Telegram chat identifier is invalid"),
            Self::InvalidMessage => formatter.write_str("Telegram message is empty"),
        }
    }
}

impl std::error::Error for TelegramTransportError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TelegramTextUpdate {
    pub(crate) update_id: i64,
    pub(crate) text: String,
    pub(crate) sender: TelegramSenderMetadata,
    pub(crate) chat: TelegramChatMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TelegramSentMessage {
    pub(crate) message_id: String,
    pub(crate) chat: TelegramChatMetadata,
}

pub(crate) fn chunk_telegram_text(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut chunks = Vec::with_capacity(
        text.chars()
            .count()
            .div_ceil(TELEGRAM_MESSAGE_CHARACTER_LIMIT),
    );
    let mut chunk_start = 0;
    for (character_index, (byte_index, _)) in text.char_indices().enumerate() {
        if character_index > 0 && character_index % TELEGRAM_MESSAGE_CHARACTER_LIMIT == 0 {
            chunks.push(&text[chunk_start..byte_index]);
            chunk_start = byte_index;
        }
    }
    chunks.push(&text[chunk_start..]);
    chunks
}

fn validate_chat_id(chat_id: &str) -> Result<(), TelegramTransportError> {
    if chat_id.is_empty() || chat_id.len() > MAX_CHAT_ID_LENGTH || chat_id.parse::<i64>().is_err() {
        return Err(TelegramTransportError::InvalidChatId);
    }
    Ok(())
}

fn map_reqwest_error(error: reqwest::Error) -> TelegramTransportError {
    if error.is_timeout() {
        TelegramTransportError::Timeout
    } else {
        TelegramTransportError::Transport
    }
}

#[derive(Serialize)]
struct EmptyRequest {}

#[derive(Serialize)]
struct GetUpdatesRequest<'a> {
    offset: i64,
    timeout: u64,
    allowed_updates: [&'a str; 1],
}

#[derive(Serialize)]
struct SendMessageRequest<'a> {
    chat_id: &'a str,
    text: &'a str,
}

#[derive(Deserialize)]
struct TelegramEnvelope<T> {
    ok: bool,
    #[serde(default = "no_result")]
    result: Option<T>,
    #[serde(default)]
    error_code: Option<i64>,
}

fn no_result<T>() -> Option<T> {
    None
}

#[derive(Deserialize)]
struct TelegramUpdate {
    update_id: i64,
    #[serde(default)]
    message: Option<TelegramMessage>,
}

impl TelegramUpdate {
    fn into_text_update(self) -> Option<TelegramTextUpdate> {
        let message = self.message?;
        let text = message.text?;
        let sender = message.sender?.into_sender_identity();
        let chat = message.chat.into_metadata()?;
        Some(TelegramTextUpdate {
            update_id: self.update_id,
            text,
            sender,
            chat,
        })
    }
}

#[derive(Deserialize)]
struct TelegramMessage {
    message_id: i64,
    #[serde(default, rename = "from")]
    sender: Option<TelegramUser>,
    chat: TelegramChat,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct TelegramUser {
    id: i64,
    #[serde(default)]
    is_bot: bool,
    first_name: String,
    #[serde(default)]
    last_name: Option<String>,
    #[serde(default)]
    username: Option<String>,
}

impl TelegramUser {
    fn display_name(&self) -> Option<String> {
        join_name(Some(self.first_name.as_str()), self.last_name.as_deref())
    }

    fn into_bot_identity(self) -> Option<TelegramBotIdentity> {
        if !self.is_bot {
            return None;
        }
        let display_name = self.display_name();
        Some(TelegramBotIdentity {
            id: self.id.to_string(),
            username: self.username,
            display_name,
        })
    }

    fn into_sender_identity(self) -> TelegramSenderMetadata {
        let display_name = self.display_name();
        TelegramSenderMetadata {
            id: self.id.to_string(),
            username: self.username,
            display_name,
        }
    }
}

#[derive(Deserialize)]
struct TelegramChat {
    id: i64,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    first_name: Option<String>,
    #[serde(default)]
    last_name: Option<String>,
}

impl TelegramChat {
    fn into_metadata(self) -> Option<TelegramChatMetadata> {
        let kind = match self.kind.as_str() {
            "private" => TelegramChatKind::Private,
            "group" => TelegramChatKind::Group,
            "supergroup" => TelegramChatKind::Supergroup,
            "channel" => TelegramChatKind::Channel,
            _ => return None,
        };
        let title = self
            .title
            .or_else(|| join_name(self.first_name.as_deref(), self.last_name.as_deref()));
        Some(TelegramChatMetadata {
            id: self.id.to_string(),
            kind,
            title,
            username: self.username,
        })
    }
}

fn join_name(first_name: Option<&str>, last_name: Option<&str>) -> Option<String> {
    let mut name = String::new();
    if let Some(first_name) = first_name.filter(|value| !value.is_empty()) {
        name.push_str(first_name);
    }
    if let Some(last_name) = last_name.filter(|value| !value.is_empty()) {
        if !name.is_empty() {
            name.push(' ');
        }
        name.push_str(last_name);
    }
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use axum::body::Body;
    use axum::extract::{Request, State};
    use axum::http::{header, Response, StatusCode};
    use axum::routing::{any, get};
    use axum::Router;
    use http_body_util::BodyExt;
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    use crate::connectors::credentials::TelegramBotToken;

    use super::{
        chunk_telegram_text, TelegramClient, TelegramClientConfig, TelegramTransportError,
        TELEGRAM_MESSAGE_CHARACTER_LIMIT,
    };

    const SENTINEL: &str = "telegram-secret-sentinel";

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<CapturedRequest>>>);

    #[derive(Clone, Debug)]
    struct CapturedRequest {
        path_and_query: String,
        body: String,
    }

    fn token() -> TelegramBotToken {
        TelegramBotToken::parse(SENTINEL).unwrap()
    }

    async fn capture_request(State(capture): State<Capture>, request: Request) -> Response<Body> {
        let path_and_query = request.uri().to_string();
        let body = request.into_body().collect().await.unwrap().to_bytes();
        capture.0.lock().await.push(CapturedRequest {
            path_and_query,
            body: String::from_utf8(body.to_vec()).unwrap(),
        });
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"ok":true,"result":{"id":42,"is_bot":true,"first_name":"Anima","username":"anima_bot"}}"#))
            .unwrap()
    }

    async fn spawn(app: Router) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{address}")
    }

    async fn client_for(app: Router) -> TelegramClient {
        let base = spawn(app).await;
        TelegramClient::for_test_base(&base, TelegramClientConfig::test_defaults()).unwrap()
    }

    #[test]
    fn production_client_has_only_the_fixed_telegram_origin() {
        let client = TelegramClient::new().unwrap();
        assert_eq!(client.test_base(), "https://api.telegram.org/");
    }

    #[tokio::test]
    async fn get_me_maps_safe_bot_identity_and_uses_expected_path() {
        let capture = Capture::default();
        let client = client_for(
            Router::new()
                .fallback(any(capture_request))
                .with_state(capture.clone()),
        )
        .await;

        let bot = client.get_me(&token()).await.unwrap();
        assert_eq!(bot.id, "42");
        assert_eq!(bot.username.as_deref(), Some("anima_bot"));
        assert_eq!(bot.display_name.as_deref(), Some("Anima"));
        assert_eq!(
            capture.0.lock().await[0].path_and_query,
            format!("/bot{SENTINEL}/getMe")
        );
    }

    #[tokio::test]
    async fn get_updates_sends_offset_and_normalizes_only_text_messages() {
        let response = r#"{"ok":true,"result":[{"update_id":10,"message":{"message_id":2,"from":{"id":7,"first_name":"Ada","last_name":"Lovelace","username":"ada"},"chat":{"id":9,"type":"private","first_name":"Ada","username":"ada"},"text":"hello"}},{"update_id":11,"message":{"message_id":3,"chat":{"id":9,"type":"private"},"photo":[{}]}},{"update_id":12,"callback_query":{"id":"x"}}]}"#;
        let capture = Capture::default();
        let response = response.to_owned();
        let app = Router::new()
            .fallback(any(
                move |State(capture): State<Capture>, request: Request| {
                    let response = response.clone();
                    async move {
                        let path_and_query = request.uri().to_string();
                        let body = request.into_body().collect().await.unwrap().to_bytes();
                        capture.0.lock().await.push(CapturedRequest {
                            path_and_query,
                            body: String::from_utf8(body.to_vec()).unwrap(),
                        });
                        Response::new(Body::from(response))
                    }
                },
            ))
            .with_state(capture.clone());
        let client = client_for(app).await;

        let updates = client.get_updates(&token(), 8).await.unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].update_id, 10);
        assert_eq!(updates[0].text, "hello");
        assert_eq!(updates[0].sender.id, "7");
        assert_eq!(
            updates[0].sender.display_name.as_deref(),
            Some("Ada Lovelace")
        );
        assert_eq!(updates[0].chat.id, "9");
        let request = &capture.0.lock().await[0];
        assert!(request.path_and_query.ends_with("/getUpdates"));
        let json: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(json["offset"], 8);
        assert!(json["timeout"].as_u64().unwrap() > 0);
        assert_eq!(json["allowed_updates"], serde_json::json!(["message"]));
    }

    #[tokio::test]
    async fn send_message_chunks_in_order_and_uses_json_body() {
        let capture = Capture::default();
        let app = Router::new().fallback(any(move |State(capture): State<Capture>, request: Request| async move {
            let path_and_query = request.uri().to_string();
            let body = request.into_body().collect().await.unwrap().to_bytes();
            capture.0.lock().await.push(CapturedRequest { path_and_query, body: String::from_utf8(body.to_vec()).unwrap() });
            Response::new(Body::from(r#"{"ok":true,"result":{"message_id":123,"chat":{"id":9,"type":"private"},"text":"ok"}}"#))
        })).with_state(capture.clone());
        let client = client_for(app).await;
        let text = format!("{}tail", "x".repeat(TELEGRAM_MESSAGE_CHARACTER_LIMIT));

        let sent = client.send_message(&token(), "9", &text).await.unwrap();
        assert_eq!(sent.len(), 2);
        let requests = capture.0.lock().await;
        let bodies: Vec<serde_json::Value> = requests
            .iter()
            .map(|item| serde_json::from_str(&item.body).unwrap())
            .collect();
        assert_eq!(bodies[0]["chat_id"], "9");
        assert_eq!(
            bodies[0]["text"].as_str().unwrap().chars().count(),
            TELEGRAM_MESSAGE_CHARACTER_LIMIT
        );
        assert_eq!(bodies[1]["text"], "tail");
        assert!(requests
            .iter()
            .all(|item| item.path_and_query.ends_with("/sendMessage")));
    }

    #[tokio::test]
    async fn empty_messages_are_rejected_without_a_request() {
        let capture = Capture::default();
        let client = client_for(
            Router::new()
                .fallback(any(capture_request))
                .with_state(capture.clone()),
        )
        .await;

        let error = client.send_message(&token(), "9", "").await.unwrap_err();
        assert_eq!(error, TelegramTransportError::InvalidMessage);
        assert!(capture.0.lock().await.is_empty());
        assert_sanitized(&error);
    }

    #[test]
    fn chunking_is_empty_aware_utf8_safe_and_character_bounded() {
        assert!(chunk_telegram_text("").is_empty());
        assert_eq!(chunk_telegram_text("short"), vec!["short"]);
        let unicode = "🦀".repeat(TELEGRAM_MESSAGE_CHARACTER_LIMIT + 1);
        let chunks = chunk_telegram_text(&unicode);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].chars().count(), TELEGRAM_MESSAGE_CHARACTER_LIMIT);
        assert_eq!(chunks[1], "🦀");
        assert_eq!(chunks.concat(), unicode);
    }

    #[tokio::test]
    async fn redirect_is_not_followed() {
        let redirected = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let redirected_state = redirected.clone();
        let app = Router::new()
            .route(
                "/redirected",
                get(move || {
                    let redirected_state = redirected_state.clone();
                    async move {
                        redirected_state.store(true, std::sync::atomic::Ordering::SeqCst);
                        "should not be reached"
                    }
                }),
            )
            .fallback(any(|| async {
                (StatusCode::FOUND, [(header::LOCATION, "/redirected")])
            }));
        let client = client_for(app).await;

        let error = client.get_me(&token()).await.unwrap_err();
        assert_eq!(error, TelegramTransportError::HttpStatus { status: 302 });
        assert!(!redirected.load(std::sync::atomic::Ordering::SeqCst));
        assert_sanitized(&error);
    }

    #[tokio::test]
    async fn request_timeout_is_typed_and_sanitized() {
        let app = Router::new().fallback(any(|| async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            r#"{"ok":true,"result":{}}"#
        }));
        let base = spawn(app).await;
        let client = TelegramClient::for_test_base(
            &base,
            TelegramClientConfig::for_tests(Duration::from_millis(20), 1024),
        )
        .unwrap();

        let error = client.get_me(&token()).await.unwrap_err();
        assert_eq!(error, TelegramTransportError::Timeout);
        assert_sanitized(&error);
    }

    #[tokio::test]
    async fn malformed_and_oversized_responses_are_rejected_safely() {
        let malformed =
            client_for(Router::new().fallback(any(|| async { format!("not-json-{SENTINEL}") })))
                .await;
        let error = malformed.get_me(&token()).await.unwrap_err();
        assert_eq!(error, TelegramTransportError::InvalidResponse);
        assert_sanitized(&error);

        let oversized =
            client_for(Router::new().fallback(any(|| async { "x".repeat(2048) }))).await;
        let base = oversized.test_base().to_owned();
        let bounded = TelegramClient::for_test_base(
            &base,
            TelegramClientConfig::for_tests(Duration::from_secs(1), 128),
        )
        .unwrap();
        let error = bounded.get_me(&token()).await.unwrap_err();
        assert_eq!(error, TelegramTransportError::ResponseTooLarge);
        assert_sanitized(&error);
    }

    #[tokio::test]
    async fn http_and_upstream_errors_do_not_reflect_secret_bodies() {
        let http = client_for(Router::new().fallback(any(|| async {
            (
                StatusCode::BAD_GATEWAY,
                format!("upstream leaked {SENTINEL}"),
            )
        })))
        .await;
        let error = http.get_me(&token()).await.unwrap_err();
        assert_eq!(error, TelegramTransportError::HttpStatus { status: 502 });
        assert_sanitized(&error);

        let upstream = client_for(Router::new().fallback(any(|| async {
            format!(r#"{{"ok":false,"error_code":401,"description":"bad {SENTINEL}"}}"#)
        })))
        .await;
        let error = upstream.get_me(&token()).await.unwrap_err();
        assert_eq!(
            error,
            TelegramTransportError::UpstreamApi { code: Some(401) }
        );
        assert_sanitized(&error);
    }

    #[test]
    fn every_transport_error_format_is_sanitized() {
        for error in TelegramTransportError::ALL {
            assert_sanitized(error);
        }
    }

    fn assert_sanitized(error: &TelegramTransportError) {
        assert!(!format!("{error:?}").contains(SENTINEL));
        assert!(!format!("{error}").contains(SENTINEL));
        assert!(!serde_json::to_string(error).unwrap().contains(SENTINEL));
    }
}
