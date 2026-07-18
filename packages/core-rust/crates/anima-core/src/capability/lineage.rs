use std::collections::BTreeMap;
use std::fmt;

use async_trait::async_trait;
use futures::lock::Mutex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{CapabilityError, CapabilityResult};

pub const CAPABILITY_EXECUTION_LEASE_MS: u64 = 30_000;

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
}
