//! Thin host binary bridging Telegram to the anima daemon HTTP API.
//!
//! All pure logic lives in the library crate; this file only wires config,
//! HTTP calls, and the two concurrent tokio loops.

use std::collections::HashSet;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anima_telegram_gateway::{
    advance_offset, build_run_request, chunk_message, extract_message, format_outbox_message,
    reply_text, unauthorized_notice, AgentsResponse, Config, GetUpdatesResponse, IncomingMessage,
    OutboxMessage, OutboxResponse, RunResponse, TELEGRAM_MAX_MESSAGE_LEN,
};
use reqwest::{Client, StatusCode};
use tokio::time::sleep;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const RETRY_BACKOFF: Duration = Duration::from_secs(5);
const TELEGRAM_LONG_POLL_TIMEOUT_SECS: i64 = 30;
const OUTBOX_PAGE_LIMIT: u64 = 100;
/// Generous client timeout: must cover the 30s Telegram long poll and
/// potentially slow daemon agent runs.
const HTTP_TIMEOUT: Duration = Duration::from_secs(120);

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();

    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            error!(error = %error, "invalid telegram-gateway configuration");
            eprintln!("configuration error: {error}");
            return ExitCode::from(2);
        }
    };

    let client = match Client::builder().timeout(HTTP_TIMEOUT).build() {
        Ok(client) => client,
        Err(error) => {
            error!(error = %error, "failed to build HTTP client");
            eprintln!("startup error: failed to build HTTP client: {error}");
            return ExitCode::FAILURE;
        }
    };

    let agent_id = match resolve_agent_id(&client, &config).await {
        Ok(agent_id) => agent_id,
        Err(error) => {
            error!(error = %error, "failed to resolve assistant agent");
            eprintln!("startup error: {error}");
            return ExitCode::FAILURE;
        }
    };

    info!(
        daemon_url = %config.daemon_url,
        assistant_name = %config.assistant_name,
        agent_id = %agent_id,
        allowed_chat_ids = ?config.allowed_chat_ids,
        outbox_poll_secs = config.outbox_poll.as_secs(),
        "anima-telegram-gateway starting"
    );
    if config.allowed_chat_ids.is_empty() {
        warn!(
            "TELEGRAM_ALLOWED_CHAT_IDS is empty: no chat is authorized and outbox pushes are \
             disabled; message the bot to discover your chat id"
        );
    }

    let updates = telegram_updates_loop(client.clone(), config.clone(), agent_id);
    let outbox = outbox_loop(client.clone(), config);

    tokio::select! {
        _ = updates => {},
        _ = outbox => {},
        _ = tokio::signal::ctrl_c() => {
            info!("shutdown signal received");
        }
    }
    ExitCode::SUCCESS
}

/// Resolve the configured assistant name to an agent id via the daemon.
async fn resolve_agent_id(client: &Client, config: &Config) -> Result<String, String> {
    let url = format!("{}/api/agents", config.daemon_url);
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|error| format!("failed to reach daemon at {url}: {error}"))?
        .error_for_status()
        .map_err(|error| format!("daemon GET {url} failed: {error}"))?;
    let body: AgentsResponse = response
        .json()
        .await
        .map_err(|error| format!("daemon GET {url} returned an unexpected body: {error}"))?;
    body.agents
        .iter()
        .find(|entry| entry.state.name == config.assistant_name)
        .map(|entry| entry.state.id.clone())
        .ok_or_else(|| {
            format!(
                "no agent named '{}' exists on the daemon at {}",
                config.assistant_name, config.daemon_url
            )
        })
}

/// Errors from a Telegram `getUpdates` call.
enum PollError {
    /// HTTP 409: another poller holds the webhook/long-poll. Just continue.
    Conflict,
    /// Anything else: retry after a backoff.
    Other(String),
}

async fn get_updates(
    client: &Client,
    api_base: &str,
    offset: i64,
) -> Result<Vec<anima_telegram_gateway::Update>, PollError> {
    let url = format!("{api_base}/getUpdates");
    let response = client
        .get(&url)
        .query(&[
            ("offset", offset),
            ("timeout", TELEGRAM_LONG_POLL_TIMEOUT_SECS),
        ])
        .send()
        .await
        .map_err(|error| PollError::Other(error.to_string()))?;
    if response.status() == StatusCode::CONFLICT {
        return Err(PollError::Conflict);
    }
    let response = response
        .error_for_status()
        .map_err(|error| PollError::Other(error.to_string()))?;
    let body: GetUpdatesResponse = response
        .json()
        .await
        .map_err(|error| PollError::Other(error.to_string()))?;
    if !body.ok {
        return Err(PollError::Other(
            body.description
                .unwrap_or_else(|| "telegram returned ok=false".to_string()),
        ));
    }
    Ok(body.result)
}

/// Send a text to a Telegram chat, splitting into <=4000-char chunks.
async fn send_message(
    client: &Client,
    api_base: &str,
    chat_id: i64,
    text: &str,
) -> Result<(), String> {
    let url = format!("{api_base}/sendMessage");
    for chunk in chunk_message(text, TELEGRAM_MAX_MESSAGE_LEN) {
        client
            .post(&url)
            .json(&serde_json::json!({ "chat_id": chat_id, "text": chunk }))
            .send()
            .await
            .map_err(|error| error.to_string())?
            .error_for_status()
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Task 1: long-poll Telegram for updates and forward allowed messages to the
/// daemon run endpoint.
async fn telegram_updates_loop(client: Client, config: Config, agent_id: String) {
    let api_base = config.telegram_api_base();
    let mut offset: i64 = 0;
    loop {
        match get_updates(&client, &api_base, offset).await {
            Ok(updates) => {
                for update in &updates {
                    offset = advance_offset(offset, update.update_id);
                    let Some(message) = extract_message(update) else {
                        continue;
                    };
                    if let Err(error) = handle_incoming(&client, &config, &agent_id, &message).await
                    {
                        warn!(
                            error = %error,
                            chat_id = message.chat_id,
                            "failed to handle telegram message; backing off"
                        );
                        sleep(RETRY_BACKOFF).await;
                    }
                }
            }
            Err(PollError::Conflict) => {
                warn!(
                    "telegram getUpdates conflict (409): another poller may be active; continuing"
                );
            }
            Err(PollError::Other(error)) => {
                warn!(error = %error, "telegram getUpdates failed; retrying after backoff");
                sleep(RETRY_BACKOFF).await;
            }
        }
    }
}

/// Forward a single incoming message to the daemon and reply with the result.
async fn handle_incoming(
    client: &Client,
    config: &Config,
    agent_id: &str,
    message: &IncomingMessage,
) -> Result<(), String> {
    let api_base = config.telegram_api_base();
    if config.allowed_chat_ids.is_empty() {
        warn!(
            chat_id = message.chat_id,
            "allowlist is empty; replying with unauthorized notice"
        );
        return send_message(
            client,
            &api_base,
            message.chat_id,
            &unauthorized_notice(message.chat_id),
        )
        .await;
    }
    if !config.allowed_chat_ids.contains(&message.chat_id) {
        warn!(
            chat_id = message.chat_id,
            "ignoring message from non-allowlisted chat"
        );
        return Ok(());
    }

    info!(
        chat_id = message.chat_id,
        user = message.user_name.as_deref().unwrap_or("unknown"),
        "forwarding telegram message to daemon"
    );
    let url = format!("{}/api/agents/{agent_id}/run", config.daemon_url);
    let reply = match client
        .post(&url)
        .json(&build_run_request(message))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
    {
        Ok(response) => match response.json::<RunResponse>().await {
            Ok(run) => reply_text(&run),
            Err(error) => format!("daemon returned an unexpected response: {error}"),
        },
        Err(error) => format!("daemon error: {error}"),
    };
    send_message(client, &api_base, message.chat_id, &reply).await
}

/// Task 2: poll the daemon assistant outbox and push new messages to every
/// allowlisted chat.
async fn outbox_loop(client: Client, config: Config) {
    let api_base = config.telegram_api_base();
    let mut since_ms = now_millis();
    let mut previous_batch_ids: HashSet<String> = HashSet::new();
    loop {
        if config.allowed_chat_ids.is_empty() {
            // No authorized recipients; skip outbox pushes entirely.
            sleep(config.outbox_poll).await;
            continue;
        }
        match get_outbox(&client, &config.daemon_url, since_ms, OUTBOX_PAGE_LIMIT).await {
            Ok(messages) => {
                let mut batch_ids: HashSet<String> = HashSet::new();
                for message in &messages {
                    batch_ids.insert(message.id.clone());
                    since_ms = since_ms.max(message.created_at_ms);
                    if previous_batch_ids.contains(&message.id) {
                        continue;
                    }
                    push_outbox_message(&client, &api_base, &config.allowed_chat_ids, message)
                        .await;
                }
                previous_batch_ids = batch_ids;
            }
            Err(error) => {
                warn!(error = %error, "assistant outbox poll failed; retrying after backoff");
                sleep(RETRY_BACKOFF).await;
                continue;
            }
        }
        sleep(config.outbox_poll).await;
    }
}

async fn get_outbox(
    client: &Client,
    daemon_url: &str,
    since_ms: u64,
    limit: u64,
) -> Result<Vec<OutboxMessage>, String> {
    let url = format!("{daemon_url}/api/assistant/outbox");
    let response = client
        .get(&url)
        .query(&[("since", since_ms), ("limit", limit)])
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    let body: OutboxResponse = response.json().await.map_err(|error| error.to_string())?;
    Ok(body.messages)
}

async fn push_outbox_message(
    client: &Client,
    api_base: &str,
    allowed_chat_ids: &[i64],
    message: &OutboxMessage,
) {
    let text = format_outbox_message(message);
    info!(job = %message.job, message_id = %message.id, "pushing outbox message");
    for chat_id in allowed_chat_ids {
        if let Err(error) = send_message(client, api_base, *chat_id, &text).await {
            warn!(
                error = %error,
                chat_id = *chat_id,
                "failed to push outbox message to chat"
            );
        }
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("anima_telegram_gateway=info"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .compact()
        .try_init();
}
