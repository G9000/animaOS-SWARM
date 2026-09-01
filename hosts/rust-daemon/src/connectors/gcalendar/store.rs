//! Google OAuth token storage in the OS credential vault, mirroring the
//! Telegram bot-token store: secrets live only in the keyring, are zeroized
//! in memory, and are redacted in logs.

use std::collections::HashMap;
use std::fmt;
use std::sync::RwLock;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::super::credentials::{account_for, CredentialStoreError};

const KEYRING_SERVICE: &str = "animaos.connector.gcalendar";
const CREDENTIAL_PAYLOAD_VERSION: u64 = 1;

pub(crate) struct GoogleOAuthTokens {
    access_token: Zeroizing<String>,
    refresh_token: Zeroizing<String>,
    expires_at_ms: u64,
}

impl GoogleOAuthTokens {
    pub(crate) fn new(
        access_token: Zeroizing<String>,
        refresh_token: Zeroizing<String>,
        expires_at_ms: u64,
    ) -> Self {
        Self {
            access_token,
            refresh_token,
            expires_at_ms,
        }
    }

    pub(crate) fn access_token(&self) -> &str {
        self.access_token.as_str()
    }

    pub(crate) fn refresh_token(&self) -> &str {
        self.refresh_token.as_str()
    }

    pub(crate) fn expires_at_ms(&self) -> u64 {
        self.expires_at_ms
    }
}

impl Clone for GoogleOAuthTokens {
    fn clone(&self) -> Self {
        Self::new(
            Zeroizing::new(self.access_token.to_string()),
            Zeroizing::new(self.refresh_token.to_string()),
            self.expires_at_ms,
        )
    }
}

impl fmt::Debug for GoogleOAuthTokens {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GoogleOAuthTokens")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

#[async_trait]
pub(crate) trait GoogleCredentialStore: Send + Sync {
    async fn load(
        &self,
        connector_id: &str,
    ) -> Result<Option<GoogleOAuthTokens>, CredentialStoreError>;
    async fn put(
        &self,
        connector_id: &str,
        tokens: GoogleOAuthTokens,
    ) -> Result<(), CredentialStoreError>;
    async fn delete(&self, connector_id: &str) -> Result<(), CredentialStoreError>;
}

#[derive(Default)]
pub(crate) struct InMemoryGoogleCredentialStore {
    values: RwLock<HashMap<String, GoogleOAuthTokens>>,
}

#[async_trait]
impl GoogleCredentialStore for InMemoryGoogleCredentialStore {
    async fn load(
        &self,
        connector_id: &str,
    ) -> Result<Option<GoogleOAuthTokens>, CredentialStoreError> {
        let account = account_for(connector_id)?;
        Ok(self
            .values
            .read()
            .map_err(|_| CredentialStoreError::BackendUnavailable)?
            .get(&account)
            .cloned())
    }

    async fn put(
        &self,
        connector_id: &str,
        tokens: GoogleOAuthTokens,
    ) -> Result<(), CredentialStoreError> {
        let account = account_for(connector_id)?;
        self.values
            .write()
            .map_err(|_| CredentialStoreError::BackendUnavailable)?
            .insert(account, tokens);
        Ok(())
    }

    async fn delete(&self, connector_id: &str) -> Result<(), CredentialStoreError> {
        let account = account_for(connector_id)?;
        self.values
            .write()
            .map_err(|_| CredentialStoreError::BackendUnavailable)?
            .remove(&account);
        Ok(())
    }
}

pub(crate) struct OsKeyringGoogleCredentialStore;

impl OsKeyringGoogleCredentialStore {
    pub(crate) fn new() -> Self {
        Self
    }
}

#[derive(Serialize)]
struct StoredGoogleCredential<'a> {
    version: u64,
    #[serde(borrow)]
    access_token: &'a str,
    #[serde(borrow)]
    refresh_token: &'a str,
    expires_at_ms: u64,
}

#[derive(Deserialize)]
struct LoadedGoogleCredential {
    version: u64,
    access_token: String,
    refresh_token: String,
    expires_at_ms: u64,
}

fn keyring_entry(connector_id: &str) -> Result<keyring::Entry, CredentialStoreError> {
    let account = account_for(connector_id)?;
    keyring::Entry::new(KEYRING_SERVICE, &account)
        .map_err(|_| CredentialStoreError::BackendUnavailable)
}

#[async_trait]
impl GoogleCredentialStore for OsKeyringGoogleCredentialStore {
    async fn load(
        &self,
        connector_id: &str,
    ) -> Result<Option<GoogleOAuthTokens>, CredentialStoreError> {
        let connector_id = connector_id.to_string();
        tokio::task::spawn_blocking(move || {
            let entry = keyring_entry(&connector_id)?;
            let payload = match entry.get_password() {
                Ok(payload) => payload,
                Err(keyring::Error::NoEntry) => return Ok(None),
                Err(_) => return Err(CredentialStoreError::BackendUnavailable),
            };
            let decoded: LoadedGoogleCredential = serde_json::from_str(&payload)
                .map_err(|_| CredentialStoreError::InvalidPayload)?;
            if decoded.version != CREDENTIAL_PAYLOAD_VERSION {
                return Err(CredentialStoreError::UnsupportedPayloadVersion);
            }
            Ok(Some(GoogleOAuthTokens::new(
                Zeroizing::new(decoded.access_token),
                Zeroizing::new(decoded.refresh_token),
                decoded.expires_at_ms,
            )))
        })
        .await
        .map_err(|_| CredentialStoreError::OperationCancelled)?
    }

    async fn put(
        &self,
        connector_id: &str,
        tokens: GoogleOAuthTokens,
    ) -> Result<(), CredentialStoreError> {
        let connector_id = connector_id.to_string();
        let payload = Zeroizing::new(
            serde_json::to_string(&StoredGoogleCredential {
                version: CREDENTIAL_PAYLOAD_VERSION,
                access_token: tokens.access_token(),
                refresh_token: tokens.refresh_token(),
                expires_at_ms: tokens.expires_at_ms(),
            })
            .map_err(|_| CredentialStoreError::InvalidPayload)?,
        );
        tokio::task::spawn_blocking(move || {
            let entry = keyring_entry(&connector_id)?;
            entry
                .set_password(payload.as_str())
                .map_err(|_| CredentialStoreError::BackendUnavailable)?;
            let observed = entry
                .get_password()
                .map_err(|_| CredentialStoreError::CredentialStateUncertain)?;
            if observed == *payload {
                Ok(())
            } else {
                Err(CredentialStoreError::CredentialStateUncertain)
            }
        })
        .await
        .map_err(|_| CredentialStoreError::CredentialStateUncertain)?
    }

    async fn delete(&self, connector_id: &str) -> Result<(), CredentialStoreError> {
        let connector_id = connector_id.to_string();
        tokio::task::spawn_blocking(move || {
            let entry = keyring_entry(&connector_id)?;
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(_) => return Err(CredentialStoreError::BackendUnavailable),
            }
            match entry.get_password() {
                Err(keyring::Error::NoEntry) => Ok(()),
                _ => Err(CredentialStoreError::CredentialStateUncertain),
            }
        })
        .await
        .map_err(|_| CredentialStoreError::CredentialStateUncertain)?
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{
        GoogleOAuthTokens, GoogleCredentialStore, InMemoryGoogleCredentialStore,
    };
    use zeroize::Zeroizing;

    fn tokens(access: &str, refresh: &str) -> GoogleOAuthTokens {
        GoogleOAuthTokens::new(
            Zeroizing::new(access.to_string()),
            Zeroizing::new(refresh.to_string()),
            42,
        )
    }

    #[test]
    fn tokens_debug_is_redacted() {
        let value = tokens("google-access-sentinel", "google-refresh-sentinel");
        let debug = format!("{value:?}");
        assert!(!debug.contains("google-access-sentinel"));
        assert!(!debug.contains("google-refresh-sentinel"));
        assert!(debug.contains("REDACTED"));
    }

    #[tokio::test]
    async fn in_memory_store_put_load_replace_and_delete() {
        let store = InMemoryGoogleCredentialStore::default();
        assert!(store.load("gcalendar-1").await.unwrap().is_none());
        store
            .put("gcalendar-1", tokens("access-1", "refresh-1"))
            .await
            .unwrap();
        let loaded = store.load("gcalendar-1").await.unwrap().unwrap();
        assert_eq!(loaded.access_token(), "access-1");
        assert_eq!(loaded.refresh_token(), "refresh-1");
        assert_eq!(loaded.expires_at_ms(), 42);
        store
            .put("gcalendar-1", tokens("access-2", "refresh-2"))
            .await
            .unwrap();
        assert_eq!(
            store.load("gcalendar-1").await.unwrap().unwrap().access_token(),
            "access-2"
        );
        store.delete("gcalendar-1").await.unwrap();
        assert!(store.load("gcalendar-1").await.unwrap().is_none());
    }
}
