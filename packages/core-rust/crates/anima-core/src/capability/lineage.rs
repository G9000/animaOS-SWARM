use std::collections::BTreeMap;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures::lock::Mutex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{CapabilityError, CapabilityResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityLeaseKind {
    Executing,
    RetryExecuting,
    Reconciling,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub enum CapabilityAttemptLineageState {
    Executing {
        fence: Uuid,
        lease_expires_at_ms: u64,
    },
    RetryAuthorized {
        authorization_id: Uuid,
    },
    RetryExecuting {
        fence: Uuid,
        lease_expires_at_ms: u64,
    },
    Completed(CapabilityResult),
    Uncertain,
    Reconciling {
        fence: Uuid,
        lease_expires_at_ms: u64,
    },
    AuthoritativeAbsence {
        fence: Uuid,
    },
    RecoveryRequired,
}

impl fmt::Debug for CapabilityAttemptLineageState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Executing { fence, .. } => formatter
                .debug_struct("Executing")
                .field("fence", fence)
                .finish_non_exhaustive(),
            Self::RetryAuthorized { .. } => formatter.write_str("RetryAuthorized(REDACTED)"),
            Self::RetryExecuting { fence, .. } => formatter
                .debug_struct("RetryExecuting")
                .field("fence", fence)
                .finish_non_exhaustive(),
            Self::Completed(result) => formatter.debug_tuple("Completed").field(result).finish(),
            Self::Uncertain => formatter.write_str("Uncertain"),
            Self::Reconciling { fence, .. } => formatter
                .debug_struct("Reconciling")
                .field("fence", fence)
                .finish_non_exhaustive(),
            Self::AuthoritativeAbsence { fence } => formatter
                .debug_struct("AuthoritativeAbsence")
                .field("fence", fence)
                .finish(),
            Self::RecoveryRequired => formatter.write_str("RecoveryRequired"),
        }
    }
}

/// A host may implement this with durable compare-and-swap storage.
#[async_trait]
pub trait CapabilityLineageStore: Send + Sync {
    async fn load(
        &self,
        logical_invocation_id: Uuid,
        attempt_number: u32,
    ) -> Result<Option<CapabilityAttemptLineageState>, CapabilityError>;

    async fn compare_exchange(
        &self,
        logical_invocation_id: Uuid,
        attempt_number: u32,
        current: Option<CapabilityAttemptLineageState>,
        new: CapabilityAttemptLineageState,
    ) -> Result<bool, CapabilityError>;

    /// Atomically creates a fenced lease using the store's own time authority.
    async fn acquire_lease(
        &self,
        logical_invocation_id: Uuid,
        attempt_number: u32,
        current: Option<CapabilityAttemptLineageState>,
        kind: CapabilityLeaseKind,
        lease_duration_ms: u64,
    ) -> Result<Option<CapabilityAttemptLineageState>, CapabilityError>;

    /// Atomically renews the exact fence only while its prior lease remains unexpired.
    async fn renew_lease(
        &self,
        logical_invocation_id: Uuid,
        attempt_number: u32,
        current: CapabilityAttemptLineageState,
        lease_duration_ms: u64,
    ) -> Result<Option<CapabilityAttemptLineageState>, CapabilityError>;

    /// Atomically replaces the exact lease only when it is expired by store-authoritative time.
    async fn expire_lease(
        &self,
        logical_invocation_id: Uuid,
        attempt_number: u32,
        current: CapabilityAttemptLineageState,
        new: CapabilityAttemptLineageState,
    ) -> Result<bool, CapabilityError>;

    /// Atomically validates an active effect fence against store-authoritative time. It must fail
    /// closed for an expired lease, a superseded or terminal state, or a mismatched fence token.
    /// This check never renews, extends, or recreates a lease.
    async fn validate_effect_fence(
        &self,
        logical_invocation_id: Uuid,
        attempt_number: u32,
        expected_kind: CapabilityLeaseKind,
        fence: Uuid,
    ) -> Result<bool, CapabilityError>;
}

#[derive(Default)]
pub(super) struct InMemoryCapabilityLineageStore {
    states: Mutex<BTreeMap<(Uuid, u32), CapabilityAttemptLineageState>>,
}

#[async_trait]
impl CapabilityLineageStore for InMemoryCapabilityLineageStore {
    async fn load(
        &self,
        logical_invocation_id: Uuid,
        attempt_number: u32,
    ) -> Result<Option<CapabilityAttemptLineageState>, CapabilityError> {
        Ok(self
            .states
            .lock()
            .await
            .get(&(logical_invocation_id, attempt_number))
            .cloned())
    }

    async fn compare_exchange(
        &self,
        logical_invocation_id: Uuid,
        attempt_number: u32,
        current: Option<CapabilityAttemptLineageState>,
        new: CapabilityAttemptLineageState,
    ) -> Result<bool, CapabilityError> {
        let mut states = self.states.lock().await;
        if states.get(&(logical_invocation_id, attempt_number)) != current.as_ref() {
            return Ok(false);
        }
        states.insert((logical_invocation_id, attempt_number), new);
        Ok(true)
    }

    async fn acquire_lease(
        &self,
        logical_invocation_id: Uuid,
        attempt_number: u32,
        current: Option<CapabilityAttemptLineageState>,
        kind: CapabilityLeaseKind,
        lease_duration_ms: u64,
    ) -> Result<Option<CapabilityAttemptLineageState>, CapabilityError> {
        let mut states = self.states.lock().await;
        if states.get(&(logical_invocation_id, attempt_number)) != current.as_ref() {
            return Ok(None);
        }
        let state = lease_state(
            kind,
            Uuid::new_v4(),
            now_ms().saturating_add(lease_duration_ms),
        );
        states.insert((logical_invocation_id, attempt_number), state.clone());
        Ok(Some(state))
    }

    async fn renew_lease(
        &self,
        logical_invocation_id: Uuid,
        attempt_number: u32,
        current: CapabilityAttemptLineageState,
        lease_duration_ms: u64,
    ) -> Result<Option<CapabilityAttemptLineageState>, CapabilityError> {
        let mut states = self.states.lock().await;
        if states.get(&(logical_invocation_id, attempt_number)) != Some(&current) {
            return Ok(None);
        }
        let now_ms = now_ms();
        let Some((kind, fence, lease_expires_at_ms)) = lease_parts(&current) else {
            return Ok(None);
        };
        if lease_expires_at_ms <= now_ms {
            return Ok(None);
        }
        let renewed = lease_state(kind, fence, now_ms.saturating_add(lease_duration_ms));
        states.insert((logical_invocation_id, attempt_number), renewed.clone());
        Ok(Some(renewed))
    }

    async fn expire_lease(
        &self,
        logical_invocation_id: Uuid,
        attempt_number: u32,
        current: CapabilityAttemptLineageState,
        new: CapabilityAttemptLineageState,
    ) -> Result<bool, CapabilityError> {
        let mut states = self.states.lock().await;
        if states.get(&(logical_invocation_id, attempt_number)) != Some(&current) {
            return Ok(false);
        }
        let Some((_, _, lease_expires_at_ms)) = lease_parts(&current) else {
            return Ok(false);
        };
        if lease_expires_at_ms > now_ms() {
            return Ok(false);
        }
        states.insert((logical_invocation_id, attempt_number), new);
        Ok(true)
    }

    async fn validate_effect_fence(
        &self,
        logical_invocation_id: Uuid,
        attempt_number: u32,
        expected_kind: CapabilityLeaseKind,
        fence: Uuid,
    ) -> Result<bool, CapabilityError> {
        let states = self.states.lock().await;
        let Some(state) = states.get(&(logical_invocation_id, attempt_number)) else {
            return Ok(false);
        };
        let Some((active_kind, active_fence, lease_expires_at_ms)) = lease_parts(state) else {
            return Ok(false);
        };
        Ok(active_kind == expected_kind && active_fence == fence && lease_expires_at_ms > now_ms())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn lease_state(
    kind: CapabilityLeaseKind,
    fence: Uuid,
    lease_expires_at_ms: u64,
) -> CapabilityAttemptLineageState {
    match kind {
        CapabilityLeaseKind::Executing => CapabilityAttemptLineageState::Executing {
            fence,
            lease_expires_at_ms,
        },
        CapabilityLeaseKind::RetryExecuting => CapabilityAttemptLineageState::RetryExecuting {
            fence,
            lease_expires_at_ms,
        },
        CapabilityLeaseKind::Reconciling => CapabilityAttemptLineageState::Reconciling {
            fence,
            lease_expires_at_ms,
        },
    }
}

fn lease_parts(state: &CapabilityAttemptLineageState) -> Option<(CapabilityLeaseKind, Uuid, u64)> {
    match state {
        CapabilityAttemptLineageState::Executing {
            fence,
            lease_expires_at_ms,
        } => Some((CapabilityLeaseKind::Executing, *fence, *lease_expires_at_ms)),
        CapabilityAttemptLineageState::RetryExecuting {
            fence,
            lease_expires_at_ms,
        } => Some((
            CapabilityLeaseKind::RetryExecuting,
            *fence,
            *lease_expires_at_ms,
        )),
        CapabilityAttemptLineageState::Reconciling {
            fence,
            lease_expires_at_ms,
        } => Some((
            CapabilityLeaseKind::Reconciling,
            *fence,
            *lease_expires_at_ms,
        )),
        _ => None,
    }
}
