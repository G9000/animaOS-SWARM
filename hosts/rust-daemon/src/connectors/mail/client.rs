#[path = "mime.rs"]
mod mime;

use super::*;
use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
#[async_trait]
pub(crate) trait MailTransport: Send + Sync {
    async fn exchange(
        &self,
        config: &OAuthConfig,
        code: &str,
        verifier: &str,
    ) -> Result<GoogleOAuthTokens, MailError>;
    async fn refresh(
        &self,
        config: &OAuthConfig,
        refresh: &str,
    ) -> Result<GoogleOAuthTokens, MailError>;
    async fn account(&self, p: Provider, access: &str) -> Result<String, MailError>;
    async fn inbox(&self, p: Provider, access: &str) -> Result<Vec<MailMessage>, MailError>;
    async fn send(&self, p: Provider, access: &str, draft: &MailDraft) -> Result<(), MailError>;
}
pub(crate) struct MailClient {
    http: reqwest::Client,
}
impl MailClient {
    pub(crate) fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("TLS mail client"),
        }
    }
    async fn json(
        &self,
        request: reqwest::RequestBuilder,
        oauth: bool,
    ) -> Result<Value, MailError> {
        let mut response = request.send().await.map_err(|_| MailError::Upstream)?;
        let status = response.status();
        if !status.is_success() {
            return Err(
                if status.as_u16() == 401
                    || status.as_u16() == 403
                    || (oauth && status.as_u16() == 400)
                {
                    MailError::Unauthorized
                } else {
                    MailError::Upstream
                },
            );
        }
        let mut bytes = vec![];
        while let Some(chunk) = response.chunk().await.map_err(|_| MailError::Upstream)? {
            if bytes.len() + chunk.len() > 1_048_576 {
                return Err(MailError::Upstream);
            }
            bytes.extend_from_slice(&chunk)
        }
        serde_json::from_slice(&bytes).map_err(|_| MailError::Upstream)
    }
    async fn tokens(
        &self,
        c: &OAuthConfig,
        extra: &[(&str, &str)],
        previous: Option<&str>,
    ) -> Result<GoogleOAuthTokens, MailError> {
        let mut form = vec![
            ("client_id", c.client_id.as_str()),
            ("client_secret", c.secret.as_str()),
            ("redirect_uri", c.redirect.as_str()),
        ];
        form.extend_from_slice(extra);
        let v = self
            .json(self.http.post(c.token_url()).form(&form), true)
            .await?;
        let access = v["access_token"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(MailError::Unauthorized)?;
        let refresh = v["refresh_token"]
            .as_str()
            .or(previous)
            .filter(|s| !s.is_empty())
            .ok_or(MailError::Unauthorized)?;
        Ok(GoogleOAuthTokens::new(
            access.to_string().into(),
            refresh.to_string().into(),
            now_ms() + v["expires_in"].as_u64().unwrap_or(3600).min(86400) * 1000,
        ))
    }
}
fn text(v: &Value, key: &str, max: usize) -> String {
    v[key]
        .as_str()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .take(max)
        .collect()
}
#[async_trait]
impl MailTransport for MailClient {
    async fn exchange(
        &self,
        c: &OAuthConfig,
        code: &str,
        verifier: &str,
    ) -> Result<GoogleOAuthTokens, MailError> {
        self.tokens(
            c,
            &[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("code_verifier", verifier),
            ],
            None,
        )
        .await
    }
    async fn refresh(
        &self,
        c: &OAuthConfig,
        refresh: &str,
    ) -> Result<GoogleOAuthTokens, MailError> {
        self.tokens(
            c,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh),
                ("scope", c.scopes()),
            ],
            Some(refresh),
        )
        .await
    }
    async fn account(&self, p: Provider, access: &str) -> Result<String, MailError> {
        let url = if p == Provider::Gmail {
            "https://gmail.googleapis.com/gmail/v1/users/me/profile"
        } else {
            "https://graph.microsoft.com/v1.0/me?$select=mail,userPrincipalName"
        };
        let v = self
            .json(self.http.get(url).bearer_auth(access), false)
            .await?;
        let key = if p == Provider::Gmail {
            "emailAddress"
        } else if v["mail"].is_string() {
            "mail"
        } else {
            "userPrincipalName"
        };
        Ok(text(&v, key, 254))
    }
    async fn inbox(&self, p: Provider, access: &str) -> Result<Vec<MailMessage>, MailError> {
        match p {
            Provider::Outlook => {
                let v = self
                    .json(
                        self.http
                            .get("https://graph.microsoft.com/v1.0/me/mailFolders/inbox/messages")
                            .query(&[
                                ("$top", "25"),
                                ("$select", "id,from,subject,bodyPreview,receivedDateTime"),
                                ("$orderby", "receivedDateTime desc"),
                            ])
                            .header("Prefer", "outlook.body-content-type=\"text\"")
                            .bearer_auth(access),
                        false,
                    )
                    .await?;
                Ok(v["value"]
                    .as_array()
                    .ok_or(MailError::Upstream)?
                    .iter()
                    .take(25)
                    .map(|m| MailMessage {
                        id: text(m, "id", 512),
                        from: text(&m["from"]["emailAddress"], "address", 254),
                        subject: text(m, "subject", 998),
                        preview: text(m, "bodyPreview", 2000),
                        received_at: text(m, "receivedDateTime", 64),
                    })
                    .collect())
            }
            Provider::Gmail => {
                let v = self
                    .json(
                        self.http
                            .get("https://gmail.googleapis.com/gmail/v1/users/me/messages")
                            .query(&[("labelIds", "INBOX"), ("maxResults", "25")])
                            .bearer_auth(access),
                        false,
                    )
                    .await?;
                let mut result = vec![];
                if let Some(messages) = v["messages"].as_array() {
                    for m in messages.iter().take(25) {
                        let id = text(m, "id", 256);
                        if !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
                            continue;
                        }
                        let detail=self.json(self.http.get(format!("https://gmail.googleapis.com/gmail/v1/users/me/messages/{id}")).query(&[("format","metadata"),("metadataHeaders","From"),("metadataHeaders","Subject")]).bearer_auth(access),false).await?;
                        let headers = detail["payload"]["headers"].as_array();
                        let header = |name: &str| max_header(headers, name);
                        let millis = detail["internalDate"]
                            .as_str()
                            .and_then(|s| s.parse::<i64>().ok())
                            .unwrap_or(0);
                        result.push(MailMessage {
                            id,
                            from: header("From"),
                            subject: header("Subject"),
                            preview: text(&detail, "snippet", 2000),
                            received_at: chrono::DateTime::from_timestamp_millis(millis)
                                .map(|d| d.to_rfc3339())
                                .unwrap_or_default(),
                        });
                    }
                }
                Ok(result)
            }
        }
    }
    async fn send(&self, p: Provider, access: &str, d: &MailDraft) -> Result<(), MailError> {
        let request=match p{
 Provider::Gmail=>{let mime=mime::serialize(&d.to, &d.subject, &d.body);self.http.post("https://gmail.googleapis.com/gmail/v1/users/me/messages/send").json(&json!({"raw":URL_SAFE_NO_PAD.encode(mime)}))},
 Provider::Outlook=>self.http.post("https://graph.microsoft.com/v1.0/me/sendMail").json(&json!({"message":{"subject":d.subject,"body":{"contentType":"Text","content":d.body},"toRecipients":d.to.iter().map(|to|json!({"emailAddress":{"address":to}})).collect::<Vec<_>>()},"saveToSentItems":true}))};
        // A connection error, timeout or 5xx may occur after acceptance. Never retry a send.
        let response = request
            .bearer_auth(access)
            .send()
            .await
            .map_err(|_| MailError::Unknown)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else if status.is_server_error() || status.as_u16() == 408 {
            Err(MailError::Unknown)
        } else if status.as_u16() == 401 || status.as_u16() == 403 {
            Err(MailError::Unauthorized)
        } else {
            Err(MailError::Upstream)
        }
    }
}
fn max_header(headers: Option<&Vec<Value>>, name: &str) -> String {
    headers
        .and_then(|h| {
            h.iter().find(|v| {
                v["name"]
                    .as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case(name))
            })
        })
        .map(|v| text(v, "value", 998))
        .unwrap_or_default()
}
