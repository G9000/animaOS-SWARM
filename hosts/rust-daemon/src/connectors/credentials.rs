use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

const KEYRING_SERVICE: &str = "animaos.connector.telegram";
const KEYRING_ACCOUNT_PREFIX: &str = "connector:";
const CREDENTIAL_PAYLOAD_VERSION: u64 = 1;
const MAX_CONNECTOR_ID_LENGTH: usize = 128;
const MAX_TELEGRAM_TOKEN_LENGTH: usize = 256;

pub(crate) struct TelegramBotToken(Zeroizing<String>);

impl TelegramBotToken {
    pub(crate) fn parse(value: impl Into<String>) -> Result<Self, CredentialStoreError> {
        let value = Zeroizing::new(value.into());
        let valid = !value.is_empty()
            && value.len() <= MAX_TELEGRAM_TOKEN_LENGTH
            && value.is_ascii()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-'));
        if !valid {
            return Err(CredentialStoreError::InvalidToken);
        }
        Ok(Self(value))
    }

    pub(crate) fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl Clone for TelegramBotToken {
    fn clone(&self) -> Self {
        Self(Zeroizing::new(self.0.to_string()))
    }
}

impl PartialEq for TelegramBotToken {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_bytes() == other.0.as_bytes()
    }
}

impl Eq for TelegramBotToken {}

impl fmt::Debug for TelegramBotToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TelegramBotToken([REDACTED])")
    }
}

impl fmt::Display for TelegramBotToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CredentialStoreError {
    InvalidToken,
    InvalidConnectorId,
    BackendUnavailable,
    CredentialStateUncertain,
    InvalidPayload,
    UnsupportedPayloadVersion,
    OperationCancelled,
}

impl CredentialStoreError {
    #[cfg(test)]
    const ALL: &'static [Self] = &[
        Self::InvalidToken,
        Self::InvalidConnectorId,
        Self::BackendUnavailable,
        Self::CredentialStateUncertain,
        Self::InvalidPayload,
        Self::UnsupportedPayloadVersion,
        Self::OperationCancelled,
    ];
}

impl fmt::Display for CredentialStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidToken => "invalid Telegram bot token",
            Self::InvalidConnectorId => "invalid connector identifier",
            Self::BackendUnavailable => "credential vault unavailable",
            Self::CredentialStateUncertain => "credential vault state could not be confirmed",
            Self::InvalidPayload => "credential payload is invalid",
            Self::UnsupportedPayloadVersion => "credential payload version is unsupported",
            Self::OperationCancelled => "credential vault operation did not complete",
        })
    }
}

impl std::error::Error for CredentialStoreError {}

#[async_trait]
pub(crate) trait ConnectorCredentialStore: Send + Sync {
    async fn load(
        &self,
        connector_id: &str,
    ) -> Result<Option<TelegramBotToken>, CredentialStoreError>;
    /// Stores a credential in the OS vault.
    ///
    /// The Task 5 connector manager must own this mutation future to completion rather than
    /// attaching it directly to an HTTP request task. Once the blocking vault operation starts,
    /// dropping the caller future cannot cancel the OS mutation and also discards its outcome.
    async fn put(
        &self,
        connector_id: &str,
        token: TelegramBotToken,
    ) -> Result<(), CredentialStoreError>;
    /// Deletes a credential from the OS vault.
    ///
    /// The Task 5 connector manager must own this mutation future to completion rather than
    /// attaching it directly to an HTTP request task. Once the blocking vault operation starts,
    /// dropping the caller future cannot cancel the OS mutation and also discards its outcome.
    async fn delete(&self, connector_id: &str) -> Result<(), CredentialStoreError>;
}

#[derive(Default)]
pub(crate) struct InMemoryCredentialStore {
    values: RwLock<HashMap<String, TelegramBotToken>>,
}

#[async_trait]
impl ConnectorCredentialStore for InMemoryCredentialStore {
    async fn load(
        &self,
        connector_id: &str,
    ) -> Result<Option<TelegramBotToken>, CredentialStoreError> {
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
        token: TelegramBotToken,
    ) -> Result<(), CredentialStoreError> {
        let account = account_for(connector_id)?;
        self.values
            .write()
            .map_err(|_| CredentialStoreError::BackendUnavailable)?
            .insert(account, token);
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

pub(crate) struct OsKeyringCredentialStore {
    backend: Arc<dyn CredentialBackend>,
}

impl OsKeyringCredentialStore {
    pub(crate) fn new() -> Self {
        Self {
            backend: Arc::new(OsKeyringBackend),
        }
    }

    #[cfg(test)]
    fn with_backend(backend: Arc<dyn CredentialBackend>) -> Self {
        Self { backend }
    }
}

impl Default for OsKeyringCredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ConnectorCredentialStore for OsKeyringCredentialStore {
    async fn load(
        &self,
        connector_id: &str,
    ) -> Result<Option<TelegramBotToken>, CredentialStoreError> {
        let account = account_for(connector_id)?;
        let backend = Arc::clone(&self.backend);
        let payload = tokio::task::spawn_blocking(move || backend.load(KEYRING_SERVICE, &account))
            .await
            .map_err(|_| CredentialStoreError::OperationCancelled)?
            .map_err(map_backend_error)?;
        let Some(payload) = payload else {
            return Ok(None);
        };

        let decoded: StoredCredential<'_> =
            serde_json::from_str(&payload).map_err(|_| CredentialStoreError::InvalidPayload)?;
        if decoded.version != CREDENTIAL_PAYLOAD_VERSION {
            return Err(CredentialStoreError::UnsupportedPayloadVersion);
        }
        TelegramBotToken::parse(decoded.token).map(Some)
    }

    async fn put(
        &self,
        connector_id: &str,
        token: TelegramBotToken,
    ) -> Result<(), CredentialStoreError> {
        let account = account_for(connector_id)?;
        let payload = Zeroizing::new(
            serde_json::to_string(&StoredCredential {
                version: CREDENTIAL_PAYLOAD_VERSION,
                token: token.expose(),
            })
            .map_err(|_| CredentialStoreError::InvalidPayload)?,
        );
        let backend = Arc::clone(&self.backend);
        tokio::task::spawn_blocking(move || -> Result<(), CredentialStoreError> {
            let previous = backend
                .load(KEYRING_SERVICE, &account)
                .map_err(map_backend_error)?;
            match backend.put(KEYRING_SERVICE, &account, &payload) {
                Ok(()) => {
                    let observed = backend
                        .load(KEYRING_SERVICE, &account)
                        .map_err(|_| CredentialStoreError::CredentialStateUncertain)?;
                    if credential_payload_matches(&observed, Some(payload.as_str())) {
                        Ok(())
                    } else {
                        Err(CredentialStoreError::CredentialStateUncertain)
                    }
                }
                Err(_) => {
                    let compensated = match previous.as_ref() {
                        Some(previous) => backend.put(KEYRING_SERVICE, &account, previous),
                        None => match backend.delete(KEYRING_SERVICE, &account) {
                            Ok(()) | Err(BackendError::NotFound) => Ok(()),
                            Err(error) => Err(error),
                        },
                    };
                    if compensated.is_err() {
                        return Err(CredentialStoreError::CredentialStateUncertain);
                    }

                    let observed = backend
                        .load(KEYRING_SERVICE, &account)
                        .map_err(|_| CredentialStoreError::CredentialStateUncertain)?;
                    if credential_payload_matches(
                        &observed,
                        previous.as_ref().map(|value| value.as_str()),
                    ) {
                        Err(CredentialStoreError::BackendUnavailable)
                    } else {
                        Err(CredentialStoreError::CredentialStateUncertain)
                    }
                }
            }
        })
        .await
        .map_err(|_| CredentialStoreError::CredentialStateUncertain)?
    }

    async fn delete(&self, connector_id: &str) -> Result<(), CredentialStoreError> {
        let account = account_for(connector_id)?;
        let backend = Arc::clone(&self.backend);
        tokio::task::spawn_blocking(move || -> Result<(), CredentialStoreError> {
            let previous = backend
                .load(KEYRING_SERVICE, &account)
                .map_err(map_backend_error)?;
            let deletion = backend.delete(KEYRING_SERVICE, &account);
            let observed = backend
                .load(KEYRING_SERVICE, &account)
                .map_err(|_| CredentialStoreError::CredentialStateUncertain)?;

            if observed.is_none() {
                return Ok(());
            }
            if deletion.is_ok() {
                return Err(CredentialStoreError::CredentialStateUncertain);
            }
            if credential_payload_matches(&observed, previous.as_ref().map(|value| value.as_str()))
            {
                Err(CredentialStoreError::BackendUnavailable)
            } else {
                Err(CredentialStoreError::CredentialStateUncertain)
            }
        })
        .await
        .map_err(|_| CredentialStoreError::CredentialStateUncertain)?
    }
}

#[derive(Serialize, Deserialize)]
struct StoredCredential<'a> {
    version: u64,
    #[serde(borrow)]
    token: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BackendError {
    NotFound,
    Unavailable,
}

trait CredentialBackend: Send + Sync {
    fn load(&self, service: &str, account: &str)
        -> Result<Option<Zeroizing<String>>, BackendError>;
    fn put(&self, service: &str, account: &str, payload: &str) -> Result<(), BackendError>;
    fn delete(&self, service: &str, account: &str) -> Result<(), BackendError>;
}

struct OsKeyringBackend;

impl OsKeyringBackend {
    fn entry(service: &str, account: &str) -> Result<keyring::Entry, BackendError> {
        keyring::Entry::new(service, account).map_err(|_| BackendError::Unavailable)
    }
}

impl CredentialBackend for OsKeyringBackend {
    fn load(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<Zeroizing<String>>, BackendError> {
        match Self::entry(service, account)?.get_password() {
            Ok(payload) => Ok(Some(Zeroizing::new(payload))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(BackendError::Unavailable),
        }
    }

    fn put(&self, service: &str, account: &str, payload: &str) -> Result<(), BackendError> {
        Self::entry(service, account)?
            .set_password(payload)
            .map_err(|_| BackendError::Unavailable)
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), BackendError> {
        match Self::entry(service, account)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Err(BackendError::NotFound),
            Err(_) => Err(BackendError::Unavailable),
        }
    }
}

fn account_for(connector_id: &str) -> Result<String, CredentialStoreError> {
    let valid = !connector_id.is_empty()
        && connector_id.len() <= MAX_CONNECTOR_ID_LENGTH
        && connector_id.is_ascii()
        && connector_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'));
    if !valid {
        return Err(CredentialStoreError::InvalidConnectorId);
    }
    Ok(format!("{KEYRING_ACCOUNT_PREFIX}{connector_id}"))
}

fn map_backend_error(_error: BackendError) -> CredentialStoreError {
    CredentialStoreError::BackendUnavailable
}

fn credential_payload_matches(
    observed: &Option<Zeroizing<String>>,
    expected: Option<&str>,
) -> bool {
    observed.as_ref().map(|value| value.as_str()) == expected
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::sync::{mpsc, Arc, Condvar, Mutex};
    use std::time::Duration;

    use super::{
        BackendError, ConnectorCredentialStore, CredentialBackend, CredentialStoreError,
        InMemoryCredentialStore, OsKeyringCredentialStore, TelegramBotToken,
    };
    use zeroize::Zeroizing;

    const SENTINEL: &str = "telegram-secret-sentinel";

    fn token(value: &str) -> TelegramBotToken {
        TelegramBotToken::parse(value).expect("valid test token")
    }

    #[test]
    fn credential_store_trait_is_object_safe() {
        let _store: Arc<dyn ConnectorCredentialStore> =
            Arc::new(InMemoryCredentialStore::default());
    }

    #[tokio::test]
    async fn in_memory_store_put_load_replace_and_delete() {
        let store = InMemoryCredentialStore::default();

        assert_eq!(store.load("primary").await.unwrap(), None);
        store.put("primary", token("first-token")).await.unwrap();
        assert_eq!(
            store.load("primary").await.unwrap().unwrap().expose(),
            "first-token"
        );
        store
            .put("primary", token("replacement-token"))
            .await
            .unwrap();
        assert_eq!(
            store.load("primary").await.unwrap().unwrap().expose(),
            "replacement-token"
        );
        store.delete("primary").await.unwrap();
        assert_eq!(store.load("primary").await.unwrap(), None);
    }

    #[test]
    fn token_debug_and_display_are_redacted() {
        let token = token(SENTINEL);
        assert!(!format!("{token:?}").contains(SENTINEL));
        assert!(!format!("{token}").contains(SENTINEL));
    }

    #[test]
    fn token_and_connector_ids_are_validated_without_reflection() {
        for invalid in ["", "   ", "line\nbreak"] {
            let error = TelegramBotToken::parse(invalid).unwrap_err();
            assert_sanitized(&error);
        }
        assert_sanitized(&TelegramBotToken::parse(&"x".repeat(257)).unwrap_err());

        let store = InMemoryCredentialStore::default();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        for invalid in [
            "",
            "spaces are unsafe",
            "../unsafe",
            "line\nbreak",
            "telegram-secret-sentinel/unsafe",
        ] {
            let error = runtime.block_on(store.load(invalid)).unwrap_err();
            assert_eq!(error, CredentialStoreError::InvalidConnectorId);
            assert_sanitized(&error);
        }
    }

    #[tokio::test]
    async fn keyring_store_uses_fixed_service_and_bounded_account() {
        let backend = Arc::new(FakeBackend::default());
        let store = OsKeyringCredentialStore::with_backend(backend.clone());
        store.put("agent-1", token("secret-value")).await.unwrap();

        let calls = backend.calls.lock().unwrap().clone();
        assert_eq!(calls[0].0, "animaos.connector.telegram");
        assert_eq!(calls[0].1, "connector:agent-1");
        let write = calls
            .iter()
            .find_map(|(_, _, payload)| payload.as_deref())
            .expect("keyring write was captured");
        assert!(write.contains("\"version\":1"));
    }

    #[tokio::test]
    async fn keyring_store_round_trips_replaces_and_deletes_versioned_credentials() {
        let backend = Arc::new(FakeBackend::default());
        let store = OsKeyringCredentialStore::with_backend(backend);

        assert_eq!(store.load("primary").await.unwrap(), None);
        store.put("primary", token("first-token")).await.unwrap();
        assert_eq!(
            store.load("primary").await.unwrap().unwrap().expose(),
            "first-token"
        );
        store
            .put("primary", token("replacement-token"))
            .await
            .unwrap();
        assert_eq!(
            store.load("primary").await.unwrap().unwrap().expose(),
            "replacement-token"
        );
        store.delete("primary").await.unwrap();
        assert_eq!(store.load("primary").await.unwrap(), None);
    }

    #[tokio::test]
    async fn delete_accepts_an_after_mutation_backend_error_when_absence_is_confirmed() {
        let backend = Arc::new(FakeBackend::default());
        let store = OsKeyringCredentialStore::with_backend(backend.clone());
        store.put("primary", token("prior-token")).await.unwrap();
        backend
            .delete_behaviors
            .lock()
            .unwrap()
            .push_back(DeleteBehavior::FailAfterMutation);

        store.delete("primary").await.unwrap();

        assert_eq!(store.load("primary").await.unwrap(), None);
    }

    #[tokio::test]
    async fn delete_reports_known_failure_when_prior_credential_is_confirmed_unchanged() {
        let backend = Arc::new(FakeBackend::default());
        let store = OsKeyringCredentialStore::with_backend(backend.clone());
        store.put("primary", token("prior-token")).await.unwrap();
        backend
            .delete_behaviors
            .lock()
            .unwrap()
            .push_back(DeleteBehavior::FailBeforeMutation);

        let error = store.delete("primary").await.unwrap_err();

        assert_eq!(error, CredentialStoreError::BackendUnavailable);
        assert_eq!(
            store.load("primary").await.unwrap().unwrap().expose(),
            "prior-token"
        );
        assert_sanitized(&error);
    }

    #[tokio::test]
    async fn delete_success_without_mutation_reports_uncertain_state() {
        let backend = Arc::new(FakeBackend::default());
        let store = OsKeyringCredentialStore::with_backend(backend.clone());
        store.put("primary", token("prior-token")).await.unwrap();
        backend
            .delete_behaviors
            .lock()
            .unwrap()
            .push_back(DeleteBehavior::SucceedWithoutMutation);

        let error = store.delete("primary").await.unwrap_err();

        assert_eq!(error, CredentialStoreError::CredentialStateUncertain);
        assert_sanitized(&error);
    }

    #[tokio::test]
    async fn delete_verification_read_failure_reports_uncertain_state() {
        let backend = Arc::new(FakeBackend::default());
        let store = OsKeyringCredentialStore::with_backend(backend.clone());
        store.put("primary", token("prior-token")).await.unwrap();
        backend
            .read_behaviors
            .lock()
            .unwrap()
            .extend([ReadBehavior::Actual, ReadBehavior::Fail]);

        let error = store.delete("primary").await.unwrap_err();

        assert_eq!(error, CredentialStoreError::CredentialStateUncertain);
        assert_sanitized(&error);
    }

    #[tokio::test]
    async fn delete_verification_mismatch_reports_uncertain_state() {
        let backend = Arc::new(FakeBackend::default());
        let store = OsKeyringCredentialStore::with_backend(backend.clone());
        store.put("primary", token("prior-token")).await.unwrap();
        backend
            .delete_behaviors
            .lock()
            .unwrap()
            .push_back(DeleteBehavior::FailBeforeMutation);
        backend.read_behaviors.lock().unwrap().extend([
            ReadBehavior::Actual,
            ReadBehavior::Override(Some(format!(r#"{{"version":1,"token":"{SENTINEL}"}}"#))),
        ]);

        let error = store.delete("primary").await.unwrap_err();

        assert_eq!(error, CredentialStoreError::CredentialStateUncertain);
        assert_sanitized(&error);
    }

    #[tokio::test]
    async fn versioned_payload_rejects_unsupported_versions_safely() {
        let backend = Arc::new(FakeBackend::default());
        backend.values.lock().unwrap().insert(
            "connector:primary".into(),
            Zeroizing::new(format!(r#"{{"version":99,"token":"{SENTINEL}"}}"#)),
        );
        let store = OsKeyringCredentialStore::with_backend(backend);

        let error = store.load("primary").await.unwrap_err();
        assert_eq!(error, CredentialStoreError::UnsupportedPayloadVersion);
        assert_sanitized(&error);
    }

    #[tokio::test]
    async fn failed_replacement_preserves_prior_credential() {
        let backend = Arc::new(FakeBackend::default());
        let store = OsKeyringCredentialStore::with_backend(backend.clone());
        store.put("primary", token("prior-token")).await.unwrap();
        backend
            .write_behaviors
            .lock()
            .unwrap()
            .push_back(WriteBehavior::FailAfterMutation);

        let error = store.put("primary", token(SENTINEL)).await.unwrap_err();
        assert_eq!(error, CredentialStoreError::BackendUnavailable);
        assert_eq!(
            store.load("primary").await.unwrap().unwrap().expose(),
            "prior-token"
        );
        assert_sanitized(&error);
    }

    #[tokio::test]
    async fn failed_initial_write_does_not_leave_an_orphaned_credential() {
        let backend = Arc::new(FakeBackend::default());
        backend
            .write_behaviors
            .lock()
            .unwrap()
            .push_back(WriteBehavior::FailAfterMutation);
        let store = OsKeyringCredentialStore::with_backend(backend);

        let error = store.put("primary", token(SENTINEL)).await.unwrap_err();
        assert_eq!(error, CredentialStoreError::BackendUnavailable);
        assert_eq!(store.load("primary").await.unwrap(), None);
        assert_sanitized(&error);
    }

    #[tokio::test]
    async fn rollback_write_failure_reports_uncertain_credential_state() {
        let backend = Arc::new(FakeBackend::default());
        let store = OsKeyringCredentialStore::with_backend(backend.clone());
        store.put("primary", token("prior-token")).await.unwrap();
        backend.write_behaviors.lock().unwrap().extend([
            WriteBehavior::FailAfterMutation,
            WriteBehavior::FailBeforeMutation,
        ]);

        let error = store.put("primary", token(SENTINEL)).await.unwrap_err();
        assert_eq!(error, CredentialStoreError::CredentialStateUncertain);
        assert_sanitized(&error);
    }

    #[tokio::test]
    async fn rollback_delete_failure_reports_uncertain_credential_state() {
        let backend = Arc::new(FakeBackend::default());
        backend
            .write_behaviors
            .lock()
            .unwrap()
            .push_back(WriteBehavior::FailAfterMutation);
        backend
            .delete_behaviors
            .lock()
            .unwrap()
            .push_back(DeleteBehavior::FailBeforeMutation);
        let store = OsKeyringCredentialStore::with_backend(backend);

        let error = store.put("primary", token(SENTINEL)).await.unwrap_err();
        assert_eq!(error, CredentialStoreError::CredentialStateUncertain);
        assert_sanitized(&error);
    }

    #[tokio::test]
    async fn rollback_verification_read_failure_reports_uncertain_state() {
        let backend = Arc::new(FakeBackend::default());
        let store = OsKeyringCredentialStore::with_backend(backend.clone());
        store.put("primary", token("prior-token")).await.unwrap();
        backend
            .read_behaviors
            .lock()
            .unwrap()
            .extend([ReadBehavior::Actual, ReadBehavior::Fail]);
        backend
            .write_behaviors
            .lock()
            .unwrap()
            .extend([WriteBehavior::FailAfterMutation, WriteBehavior::Succeed]);

        let error = store.put("primary", token(SENTINEL)).await.unwrap_err();
        assert_eq!(error, CredentialStoreError::CredentialStateUncertain);
        assert_sanitized(&error);
    }

    #[tokio::test]
    async fn rollback_verification_mismatch_reports_uncertain_state() {
        let backend = Arc::new(FakeBackend::default());
        let store = OsKeyringCredentialStore::with_backend(backend.clone());
        store.put("primary", token("prior-token")).await.unwrap();
        backend.read_behaviors.lock().unwrap().extend([
            ReadBehavior::Actual,
            ReadBehavior::Override(Some(format!(r#"{{"version":1,"token":"{SENTINEL}"}}"#))),
        ]);
        backend
            .write_behaviors
            .lock()
            .unwrap()
            .extend([WriteBehavior::FailAfterMutation, WriteBehavior::Succeed]);

        let error = store.put("primary", token(SENTINEL)).await.unwrap_err();
        assert_eq!(error, CredentialStoreError::CredentialStateUncertain);
        assert_sanitized(&error);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn aborting_the_caller_does_not_cancel_an_in_flight_vault_mutation() {
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let backend = Arc::new(BlockingBackend {
            value: Mutex::new(None),
            started_tx: Mutex::new(Some(started_tx)),
            finished_tx: Mutex::new(Some(finished_tx)),
            release: Arc::clone(&release),
        });
        let store = Arc::new(OsKeyringCredentialStore::with_backend(backend.clone()));
        let mutation = tokio::spawn({
            let store = Arc::clone(&store);
            async move { store.put("primary", token(SENTINEL)).await }
        });

        tokio::task::spawn_blocking(move || started_rx.recv_timeout(Duration::from_secs(2)))
            .await
            .unwrap()
            .unwrap();
        mutation.abort();
        {
            let (released, condition) = &*release;
            *released.lock().unwrap() = true;
            condition.notify_all();
        }
        tokio::task::spawn_blocking(move || finished_rx.recv_timeout(Duration::from_secs(2)))
            .await
            .unwrap()
            .unwrap();

        let stored = backend.value.lock().unwrap().clone().unwrap();
        assert!(stored.contains(SENTINEL));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn aborting_the_caller_does_not_cancel_an_in_flight_vault_delete() {
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let backend = Arc::new(BlockingDeleteBackend {
            value: Mutex::new(Some(Zeroizing::new(format!(
                r#"{{"version":1,"token":"{SENTINEL}"}}"#
            )))),
            started_tx: Mutex::new(Some(started_tx)),
            finished_tx: Mutex::new(Some(finished_tx)),
            release: Arc::clone(&release),
        });
        let store = Arc::new(OsKeyringCredentialStore::with_backend(backend.clone()));
        let mutation = tokio::spawn({
            let store = Arc::clone(&store);
            async move { store.delete("primary").await }
        });

        tokio::task::spawn_blocking(move || started_rx.recv_timeout(Duration::from_secs(2)))
            .await
            .unwrap()
            .unwrap();
        mutation.abort();
        {
            let (released, condition) = &*release;
            *released.lock().unwrap() = true;
            condition.notify_all();
        }
        tokio::task::spawn_blocking(move || finished_rx.recv_timeout(Duration::from_secs(2)))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(store.load("primary").await.unwrap(), None);
    }

    #[test]
    fn every_public_error_format_is_sanitized() {
        for error in CredentialStoreError::ALL {
            assert_sanitized(error);
        }
    }

    fn assert_sanitized(error: &CredentialStoreError) {
        assert!(!format!("{error:?}").contains(SENTINEL));
        assert!(!format!("{error}").contains(SENTINEL));
        assert!(!serde_json::to_string(error).unwrap().contains(SENTINEL));
    }

    #[derive(Default)]
    struct FakeBackend {
        values: Mutex<HashMap<String, Zeroizing<String>>>,
        calls: Mutex<Vec<(String, String, Option<String>)>>,
        read_behaviors: Mutex<VecDeque<ReadBehavior>>,
        write_behaviors: Mutex<VecDeque<WriteBehavior>>,
        delete_behaviors: Mutex<VecDeque<DeleteBehavior>>,
    }

    #[derive(Clone)]
    enum ReadBehavior {
        Actual,
        Fail,
        Override(Option<String>),
    }

    #[derive(Clone, Copy)]
    enum WriteBehavior {
        Succeed,
        FailBeforeMutation,
        FailAfterMutation,
    }

    #[derive(Clone, Copy)]
    enum DeleteBehavior {
        SucceedWithoutMutation,
        FailBeforeMutation,
        FailAfterMutation,
    }

    impl CredentialBackend for FakeBackend {
        fn load(
            &self,
            service: &str,
            account: &str,
        ) -> Result<Option<Zeroizing<String>>, BackendError> {
            self.calls
                .lock()
                .unwrap()
                .push((service.into(), account.into(), None));
            match self
                .read_behaviors
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(ReadBehavior::Actual)
            {
                ReadBehavior::Actual => {}
                ReadBehavior::Fail => return Err(BackendError::Unavailable),
                ReadBehavior::Override(value) => return Ok(value.map(Zeroizing::new)),
            }
            Ok(self.values.lock().unwrap().get(account).cloned())
        }

        fn put(&self, service: &str, account: &str, payload: &str) -> Result<(), BackendError> {
            self.calls
                .lock()
                .unwrap()
                .push((service.into(), account.into(), Some(payload.into())));
            let behavior = self
                .write_behaviors
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(WriteBehavior::Succeed);
            if matches!(behavior, WriteBehavior::FailBeforeMutation) {
                return Err(BackendError::Unavailable);
            }
            self.values
                .lock()
                .unwrap()
                .insert(account.into(), Zeroizing::new(payload.into()));
            if matches!(behavior, WriteBehavior::FailAfterMutation) {
                return Err(BackendError::Unavailable);
            }
            Ok(())
        }

        fn delete(&self, service: &str, account: &str) -> Result<(), BackendError> {
            self.calls
                .lock()
                .unwrap()
                .push((service.into(), account.into(), None));
            let behavior = self.delete_behaviors.lock().unwrap().pop_front();
            if matches!(behavior, Some(DeleteBehavior::FailBeforeMutation)) {
                return Err(BackendError::Unavailable);
            }
            if matches!(behavior, Some(DeleteBehavior::SucceedWithoutMutation)) {
                return Ok(());
            }
            self.values.lock().unwrap().remove(account);
            if matches!(behavior, Some(DeleteBehavior::FailAfterMutation)) {
                return Err(BackendError::Unavailable);
            }
            Ok(())
        }
    }

    struct BlockingBackend {
        value: Mutex<Option<Zeroizing<String>>>,
        started_tx: Mutex<Option<mpsc::Sender<()>>>,
        finished_tx: Mutex<Option<mpsc::Sender<()>>>,
        release: Arc<(Mutex<bool>, Condvar)>,
    }

    impl CredentialBackend for BlockingBackend {
        fn load(
            &self,
            _service: &str,
            _account: &str,
        ) -> Result<Option<Zeroizing<String>>, BackendError> {
            Ok(self.value.lock().unwrap().clone())
        }

        fn put(&self, _service: &str, _account: &str, payload: &str) -> Result<(), BackendError> {
            if let Some(started_tx) = self.started_tx.lock().unwrap().take() {
                started_tx.send(()).unwrap();
                let (released, condition) = &*self.release;
                let mut released = released.lock().unwrap();
                while !*released {
                    released = condition.wait(released).unwrap();
                }
            }
            *self.value.lock().unwrap() = Some(Zeroizing::new(payload.to_owned()));
            if let Some(finished_tx) = self.finished_tx.lock().unwrap().take() {
                finished_tx.send(()).unwrap();
            }
            Ok(())
        }

        fn delete(&self, _service: &str, _account: &str) -> Result<(), BackendError> {
            *self.value.lock().unwrap() = None;
            Ok(())
        }
    }

    struct BlockingDeleteBackend {
        value: Mutex<Option<Zeroizing<String>>>,
        started_tx: Mutex<Option<mpsc::Sender<()>>>,
        finished_tx: Mutex<Option<mpsc::Sender<()>>>,
        release: Arc<(Mutex<bool>, Condvar)>,
    }

    impl CredentialBackend for BlockingDeleteBackend {
        fn load(
            &self,
            _service: &str,
            _account: &str,
        ) -> Result<Option<Zeroizing<String>>, BackendError> {
            Ok(self.value.lock().unwrap().clone())
        }

        fn put(&self, _service: &str, _account: &str, payload: &str) -> Result<(), BackendError> {
            *self.value.lock().unwrap() = Some(Zeroizing::new(payload.to_owned()));
            Ok(())
        }

        fn delete(&self, _service: &str, _account: &str) -> Result<(), BackendError> {
            if let Some(started_tx) = self.started_tx.lock().unwrap().take() {
                started_tx.send(()).unwrap();
                let (released, condition) = &*self.release;
                let mut released = released.lock().unwrap();
                while !*released {
                    released = condition.wait(released).unwrap();
                }
            }
            *self.value.lock().unwrap() = None;
            if let Some(finished_tx) = self.finished_tx.lock().unwrap().take() {
                finished_tx.send(()).unwrap();
            }
            Ok(())
        }
    }
}
