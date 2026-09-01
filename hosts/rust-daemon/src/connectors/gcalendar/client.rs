use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::{url_encode, CalendarEventDraft, GoogleOAuthConfig};
use super::store::GoogleOAuthTokens;

const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_CALENDAR_API_BASE: &str = "https://www.googleapis.com/calendar/v3";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GoogleTransportError {
    Configuration,
    /// 401/403 — the access token was rejected or the grant was revoked.
    Unauthorized,
    /// 404 — the event or calendar no longer exists upstream.
    NotFound,
    /// 429 — rate limited.
    RateLimited,
    Unavailable,
}

impl std::fmt::Display for GoogleTransportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Configuration => "google calendar client is not configured",
            Self::Unauthorized => "google calendar rejected the credential",
            Self::NotFound => "google calendar resource was not found",
            Self::RateLimited => "google calendar rate limited the request",
            Self::Unavailable => "google calendar is unavailable",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GoogleCalendarEvent {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) start: String,
    pub(crate) end: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
}

#[async_trait]
pub(crate) trait GoogleCalendarTransport: Send + Sync {
    async fn exchange_code(
        &self,
        config: &GoogleOAuthConfig,
        code: &str,
    ) -> Result<GoogleOAuthTokens, GoogleTransportError>;

    async fn refresh_tokens(
        &self,
        config: &GoogleOAuthConfig,
        refresh_token: &str,
    ) -> Result<GoogleOAuthTokens, GoogleTransportError>;

    /// Non-secret label for the connected account (the primary calendar id,
    /// which is normally the account email).
    async fn primary_calendar(
        &self,
        access_token: &str,
    ) -> Result<String, GoogleTransportError>;

    async fn list_events(
        &self,
        access_token: &str,
        calendar_id: &str,
        time_min: &str,
        time_max: &str,
    ) -> Result<Vec<GoogleCalendarEvent>, GoogleTransportError>;

    async fn create_event(
        &self,
        access_token: &str,
        draft: &CalendarEventDraft,
    ) -> Result<String, GoogleTransportError>;

    async fn update_event(
        &self,
        access_token: &str,
        draft: &CalendarEventDraft,
    ) -> Result<(), GoogleTransportError>;

    async fn delete_event(
        &self,
        access_token: &str,
        draft: &CalendarEventDraft,
    ) -> Result<(), GoogleTransportError>;
}

pub(crate) struct GoogleCalendarClient {
    client: Client,
}

/// Transport used when Google OAuth is not configured on this daemon. Every
/// call fails with `Configuration`; the manager surfaces `Unconfigured`
/// before reaching it.
pub(crate) struct UnconfiguredGoogleTransport;

#[async_trait]
impl GoogleCalendarTransport for UnconfiguredGoogleTransport {
    async fn exchange_code(
        &self,
        _config: &GoogleOAuthConfig,
        _code: &str,
    ) -> Result<GoogleOAuthTokens, GoogleTransportError> {
        Err(GoogleTransportError::Configuration)
    }

    async fn refresh_tokens(
        &self,
        _config: &GoogleOAuthConfig,
        _refresh_token: &str,
    ) -> Result<GoogleOAuthTokens, GoogleTransportError> {
        Err(GoogleTransportError::Configuration)
    }

    async fn primary_calendar(
        &self,
        _access_token: &str,
    ) -> Result<String, GoogleTransportError> {
        Err(GoogleTransportError::Configuration)
    }

    async fn list_events(
        &self,
        _access_token: &str,
        _calendar_id: &str,
        _time_min: &str,
        _time_max: &str,
    ) -> Result<Vec<GoogleCalendarEvent>, GoogleTransportError> {
        Err(GoogleTransportError::Configuration)
    }

    async fn create_event(
        &self,
        _access_token: &str,
        _draft: &CalendarEventDraft,
    ) -> Result<String, GoogleTransportError> {
        Err(GoogleTransportError::Configuration)
    }

    async fn update_event(
        &self,
        _access_token: &str,
        _draft: &CalendarEventDraft,
    ) -> Result<(), GoogleTransportError> {
        Err(GoogleTransportError::Configuration)
    }

    async fn delete_event(
        &self,
        _access_token: &str,
        _draft: &CalendarEventDraft,
    ) -> Result<(), GoogleTransportError> {
        Err(GoogleTransportError::Configuration)
    }
}

impl GoogleCalendarClient {
    pub(crate) fn new() -> Result<Self, GoogleTransportError> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|_| GoogleTransportError::Configuration)?;
        Ok(Self { client })
    }
}

#[derive(Deserialize)]
struct GoogleTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

#[derive(Deserialize)]
struct GoogleCalendarResource {
    id: String,
}

#[derive(Deserialize)]
struct GoogleEventListResponse {
    #[serde(default)]
    items: Vec<GoogleEventResource>,
}

#[derive(Deserialize)]
struct GoogleEventResource {
    id: String,
    summary: Option<String>,
    location: Option<String>,
    description: Option<String>,
    start: Option<GoogleEventDateTime>,
    end: Option<GoogleEventDateTime>,
}

#[derive(Deserialize)]
struct GoogleEventDateTime {
    #[serde(rename = "dateTime")]
    date_time: Option<String>,
    date: Option<String>,
}

impl GoogleEventDateTime {
    fn value(self) -> String {
        self.date_time.or(self.date).unwrap_or_default()
    }
}

#[derive(Serialize)]
struct GoogleEventBody<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start: Option<GoogleEventBodyTime<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    end: Option<GoogleEventBodyTime<'a>>,
}

#[derive(Serialize)]
struct GoogleEventBodyTime<'a> {
    #[serde(rename = "dateTime")]
    date_time: &'a str,
}

async fn read_bounded_json<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T, GoogleTransportError> {
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(GoogleTransportError::Unauthorized);
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(GoogleTransportError::NotFound);
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(GoogleTransportError::RateLimited);
    }
    if !status.is_success() {
        return Err(GoogleTransportError::Unavailable);
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| GoogleTransportError::Unavailable)?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(GoogleTransportError::Unavailable);
    }
    serde_json::from_slice(&bytes).map_err(|_| GoogleTransportError::Unavailable)
}

fn tokens_from_response(
    response: GoogleTokenResponse,
    existing_refresh_token: Option<&str>,
) -> Result<GoogleOAuthTokens, GoogleTransportError> {
    let refresh_token = response
        .refresh_token
        .or_else(|| existing_refresh_token.map(str::to_string))
        .ok_or(GoogleTransportError::Unavailable)?;
    let expires_at_ms = super::now_ms() + response.expires_in.unwrap_or(3600).saturating_mul(1000);
    Ok(GoogleOAuthTokens::new(
        Zeroizing::new(response.access_token),
        Zeroizing::new(refresh_token),
        expires_at_ms,
    ))
}

#[async_trait]
impl GoogleCalendarTransport for GoogleCalendarClient {
    async fn exchange_code(
        &self,
        config: &GoogleOAuthConfig,
        code: &str,
    ) -> Result<GoogleOAuthTokens, GoogleTransportError> {
        let response = self
            .client
            .post(GOOGLE_TOKEN_URL)
            .form(&[
                ("code", code),
                ("client_id", config.client_id.as_str()),
                ("client_secret", config.client_secret()),
                ("redirect_uri", config.redirect_uri.as_str()),
                ("grant_type", "authorization_code"),
            ])
            .send()
            .await
            .map_err(|_| GoogleTransportError::Unavailable)?;
        let parsed: GoogleTokenResponse = read_bounded_json(response).await?;
        tokens_from_response(parsed, None)
    }

    async fn refresh_tokens(
        &self,
        config: &GoogleOAuthConfig,
        refresh_token: &str,
    ) -> Result<GoogleOAuthTokens, GoogleTransportError> {
        let response = self
            .client
            .post(GOOGLE_TOKEN_URL)
            .form(&[
                ("refresh_token", refresh_token),
                ("client_id", config.client_id.as_str()),
                ("client_secret", config.client_secret()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .map_err(|_| GoogleTransportError::Unavailable)?;
        let parsed: GoogleTokenResponse = read_bounded_json(response).await?;
        tokens_from_response(parsed, Some(refresh_token))
    }

    async fn primary_calendar(
        &self,
        access_token: &str,
    ) -> Result<String, GoogleTransportError> {
        let response = self
            .client
            .get(format!("{GOOGLE_CALENDAR_API_BASE}/calendars/primary"))
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|_| GoogleTransportError::Unavailable)?;
        let parsed: GoogleCalendarResource = read_bounded_json(response).await?;
        Ok(parsed.id)
    }

    async fn list_events(
        &self,
        access_token: &str,
        calendar_id: &str,
        time_min: &str,
        time_max: &str,
    ) -> Result<Vec<GoogleCalendarEvent>, GoogleTransportError> {
        let url = format!(
            "{}/calendars/{}/events?timeMin={}&timeMax={}&singleEvents=true&orderBy=startTime&maxResults=50",
            GOOGLE_CALENDAR_API_BASE,
            url_encode(calendar_id),
            url_encode(time_min),
            url_encode(time_max),
        );
        let response = self
            .client
            .get(url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|_| GoogleTransportError::Unavailable)?;
        let parsed: GoogleEventListResponse = read_bounded_json(response).await?;
        Ok(parsed
            .items
            .into_iter()
            .map(|item| GoogleCalendarEvent {
                id: item.id,
                title: item.summary.unwrap_or_else(|| "(untitled)".to_string()),
                start: item.start.map(GoogleEventDateTime::value).unwrap_or_default(),
                end: item.end.map(GoogleEventDateTime::value).unwrap_or_default(),
                location: item.location,
                description: item.description,
            })
            .collect())
    }

    async fn create_event(
        &self,
        access_token: &str,
        draft: &CalendarEventDraft,
    ) -> Result<String, GoogleTransportError> {
        let body = GoogleEventBody {
            summary: Some(draft.title.as_str()),
            location: draft.location.as_deref(),
            description: draft.description.as_deref(),
            start: Some(GoogleEventBodyTime {
                date_time: draft.start.as_str(),
            }),
            end: Some(GoogleEventBodyTime {
                date_time: draft.end.as_str(),
            }),
        };
        let response = self
            .client
            .post(format!(
                "{}/calendars/{}/events",
                GOOGLE_CALENDAR_API_BASE,
                url_encode(&draft.calendar_id)
            ))
            .bearer_auth(access_token)
            .json(&body)
            .send()
            .await
            .map_err(|_| GoogleTransportError::Unavailable)?;
        let parsed: GoogleCalendarResource = read_bounded_json(response).await?;
        Ok(parsed.id)
    }

    async fn update_event(
        &self,
        access_token: &str,
        draft: &CalendarEventDraft,
    ) -> Result<(), GoogleTransportError> {
        let event_id = draft
            .event_id
            .as_deref()
            .ok_or(GoogleTransportError::Configuration)?;
        let body = GoogleEventBody {
            summary: (!draft.title.is_empty()).then_some(draft.title.as_str()),
            location: draft.location.as_deref(),
            description: draft.description.as_deref(),
            start: (!draft.start.is_empty()).then_some(GoogleEventBodyTime {
                date_time: draft.start.as_str(),
            }),
            end: (!draft.end.is_empty()).then_some(GoogleEventBodyTime {
                date_time: draft.end.as_str(),
            }),
        };
        let response = self
            .client
            .patch(format!(
                "{}/calendars/{}/events/{}",
                GOOGLE_CALENDAR_API_BASE,
                url_encode(&draft.calendar_id),
                url_encode(event_id)
            ))
            .bearer_auth(access_token)
            .json(&body)
            .send()
            .await
            .map_err(|_| GoogleTransportError::Unavailable)?;
        let _: serde_json::Value = read_bounded_json(response).await?;
        Ok(())
    }

    async fn delete_event(
        &self,
        access_token: &str,
        draft: &CalendarEventDraft,
    ) -> Result<(), GoogleTransportError> {
        let event_id = draft
            .event_id
            .as_deref()
            .ok_or(GoogleTransportError::Configuration)?;
        let response = self
            .client
            .delete(format!(
                "{}/calendars/{}/events/{}",
                GOOGLE_CALENDAR_API_BASE,
                url_encode(&draft.calendar_id),
                url_encode(event_id)
            ))
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|_| GoogleTransportError::Unavailable)?;
        let status = response.status();
        if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
            // Deleting an already-gone event converges to the desired state.
            return Ok(());
        }
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(GoogleTransportError::Unauthorized);
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(GoogleTransportError::RateLimited);
        }
        Err(GoogleTransportError::Unavailable)
    }
}
