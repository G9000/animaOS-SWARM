//! Pure, portable capability risk evaluation.
//!
//! Policy records bind only stable identifiers and canonical argument digests. They deliberately
//! never retain normalized arguments, credential values, executor state, or host-specific data.

use std::cmp::Reverse;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use crate::{CapabilityManifest, LogicalInvocation, RiskLevel};

const APPROVAL_WINDOW_MS: i64 = 300_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PolicyRestrictions {
    /// Raises the manifest risk floor. It may not lower a manifest's declared risk.
    pub minimum_risk: Option<RiskLevel>,
    /// Denies the action unless an exact active autonomy grant explicitly permits it.
    pub deny: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyContext {
    pub owner_id: String,
    pub actor_id: String,
    pub agent_definition_id: String,
    pub agent_definition_version: u32,
    pub workspace_id: String,
    pub resource_boundary: String,
    pub capability_id: String,
    pub manifest_version: u32,
    pub manifest_risk: RiskLevel,
    pub run_id: Uuid,
    pub logical_step_id: String,
    pub logical_invocation_id: Uuid,
    pub canonical_argument_digest: Uuid,
    pub policy_revision: u32,
    pub restrictions: PolicyRestrictions,
    /// Unix milliseconds supplied by the host; policy evaluation does not read a clock.
    pub now_ms: i64,
    #[serde(skip)]
    provenance: Option<PolicyInputSnapshot>,
}

/// An in-memory provenance pin made only after exact manifest/invocation validation.
/// It is intentionally omitted from JSON: a deserialized context must be re-verified before use.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PolicyInputSnapshot {
    manifest_id: String,
    manifest_version: u32,
    manifest_risk: RiskLevel,
    run_id: Uuid,
    logical_step_id: String,
    logical_invocation_id: Uuid,
    canonical_argument_digest: Uuid,
}

impl PolicyContext {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        owner_id: impl Into<String>,
        actor_id: impl Into<String>,
        agent_definition_id: impl Into<String>,
        agent_definition_version: u32,
        workspace_id: impl Into<String>,
        resource_boundary: impl Into<String>,
        manifest: &CapabilityManifest,
        invocation: &LogicalInvocation,
        policy_revision: u32,
        restrictions: PolicyRestrictions,
        now_ms: i64,
    ) -> Result<Self, PolicyValidationError> {
        let mut context = Self {
            owner_id: owner_id.into(),
            actor_id: actor_id.into(),
            agent_definition_id: agent_definition_id.into(),
            agent_definition_version,
            workspace_id: workspace_id.into(),
            resource_boundary: resource_boundary.into(),
            capability_id: manifest.id.clone(),
            manifest_version: manifest.version,
            manifest_risk: manifest.risk_level,
            run_id: invocation.run_id(),
            logical_step_id: invocation_step_id(invocation),
            logical_invocation_id: invocation.id(),
            canonical_argument_digest: invocation.canonical_argument_digest(),
            policy_revision,
            restrictions,
            now_ms,
            provenance: None,
        };
        context.verify_against(manifest, invocation)?;
        Ok(context)
    }

    pub fn effective_risk(&self) -> RiskLevel {
        self.restrictions
            .minimum_risk
            .map_or(self.manifest_risk, |minimum| {
                max_risk(self.manifest_risk, minimum)
            })
    }

    fn validate(&self) -> Result<(), PolicyValidationError> {
        for (field, value) in [
            ("owner_id", &self.owner_id),
            ("actor_id", &self.actor_id),
            ("agent_definition_id", &self.agent_definition_id),
            ("workspace_id", &self.workspace_id),
            ("resource_boundary", &self.resource_boundary),
            ("capability_id", &self.capability_id),
            ("logical_step_id", &self.logical_step_id),
        ] {
            validate_id(field, value)?;
        }
        if self.agent_definition_version == 0
            || self.manifest_version == 0
            || self.policy_revision == 0
        {
            return Err(PolicyValidationError::InvalidVersion);
        }
        if self.manifest_risk == RiskLevel::None
            || self.restrictions.minimum_risk == Some(RiskLevel::None)
        {
            return Err(PolicyValidationError::InvalidRiskConstraint);
        }
        Ok(())
    }

    /// Re-establishes the nonserialized provenance pin after loading a policy context.
    pub fn verify_against(
        &mut self,
        manifest: &CapabilityManifest,
        invocation: &LogicalInvocation,
    ) -> Result<(), PolicyValidationError> {
        self.validate()?;
        if manifest.id != invocation.capability_id()
            || manifest.version != invocation.manifest_version()
            || self.capability_id != manifest.id
            || self.manifest_version != manifest.version
            || self.manifest_risk != manifest.risk_level
        {
            return Err(PolicyValidationError::ContextManifestMismatch);
        }
        if self.run_id != invocation.run_id()
            || self.logical_step_id != invocation.logical_step_id()
            || self.logical_invocation_id != invocation.id()
            || self.canonical_argument_digest != invocation.canonical_argument_digest()
        {
            return Err(PolicyValidationError::ContextInvocationMismatch);
        }
        self.provenance = Some(PolicyInputSnapshot {
            manifest_id: manifest.id.clone(),
            manifest_version: manifest.version,
            manifest_risk: manifest.risk_level,
            run_id: invocation.run_id(),
            logical_step_id: invocation.logical_step_id().to_owned(),
            logical_invocation_id: invocation.id(),
            canonical_argument_digest: invocation.canonical_argument_digest(),
        });
        Ok(())
    }

    fn validate_evaluation_input(&self) -> Result<(), PolicyValidationError> {
        self.validate()?;
        let Some(snapshot) = &self.provenance else {
            return Err(PolicyValidationError::UnverifiedPolicyContext);
        };
        if snapshot.manifest_id != self.capability_id
            || snapshot.manifest_version != self.manifest_version
            || snapshot.manifest_risk != self.manifest_risk
        {
            return Err(PolicyValidationError::ContextManifestMismatch);
        }
        if snapshot.run_id != self.run_id
            || snapshot.logical_step_id != self.logical_step_id
            || snapshot.logical_invocation_id != self.logical_invocation_id
            || snapshot.canonical_argument_digest != self.canonical_argument_digest
        {
            return Err(PolicyValidationError::ContextInvocationMismatch);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for PolicyContext {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let context = PolicyContextWire::deserialize(deserializer)?.into_context();
        context.validate().map_err(serde::de::Error::custom)?;
        Ok(context)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyContextWire {
    owner_id: String,
    actor_id: String,
    agent_definition_id: String,
    agent_definition_version: u32,
    workspace_id: String,
    resource_boundary: String,
    capability_id: String,
    manifest_version: u32,
    manifest_risk: RiskLevel,
    run_id: Uuid,
    logical_step_id: String,
    logical_invocation_id: Uuid,
    canonical_argument_digest: Uuid,
    policy_revision: u32,
    restrictions: PolicyRestrictions,
    now_ms: i64,
}

impl PolicyContextWire {
    fn into_context(self) -> PolicyContext {
        PolicyContext {
            owner_id: self.owner_id,
            actor_id: self.actor_id,
            agent_definition_id: self.agent_definition_id,
            agent_definition_version: self.agent_definition_version,
            workspace_id: self.workspace_id,
            resource_boundary: self.resource_boundary,
            capability_id: self.capability_id,
            manifest_version: self.manifest_version,
            manifest_risk: self.manifest_risk,
            run_id: self.run_id,
            logical_step_id: self.logical_step_id,
            logical_invocation_id: self.logical_invocation_id,
            canonical_argument_digest: self.canonical_argument_digest,
            policy_revision: self.policy_revision,
            restrictions: self.restrictions,
            now_ms: self.now_ms,
            provenance: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyReasonCode {
    AllowedByDefault,
    ApprovalRequired,
    DeniedByDefault,
    DeniedByRestriction,
    AllowedByGrant,
    AllowedByApproval,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyReason {
    pub code: PolicyReasonCode,
    pub effective_risk: RiskLevel,
    pub policy_revision: u32,
    pub grant_id: Option<String>,
    pub grant_revision: Option<u32>,
}

impl PolicyReason {
    pub fn new(
        code: PolicyReasonCode,
        effective_risk: RiskLevel,
        policy_revision: u32,
        grant_id: Option<String>,
        grant_revision: Option<u32>,
    ) -> Result<Self, PolicyValidationError> {
        let reason = Self {
            code,
            effective_risk,
            policy_revision,
            grant_id,
            grant_revision,
        };
        reason.validate()?;
        Ok(reason)
    }

    fn validate(&self) -> Result<(), PolicyValidationError> {
        if self.effective_risk == RiskLevel::None || self.policy_revision == 0 {
            return Err(PolicyValidationError::InconsistentAuditRecord);
        }
        let has_grant = match (&self.grant_id, self.grant_revision) {
            (Some(id), Some(revision)) if !id.trim().is_empty() && revision > 0 => true,
            (None, None) => false,
            _ => return Err(PolicyValidationError::InconsistentAuditRecord),
        };
        match self.code {
            PolicyReasonCode::AllowedByGrant if !has_grant => {
                Err(PolicyValidationError::InconsistentAuditRecord)
            }
            PolicyReasonCode::AllowedByDefault
            | PolicyReasonCode::DeniedByDefault
            | PolicyReasonCode::DeniedByRestriction
            | PolicyReasonCode::AllowedByApproval
                if has_grant =>
            {
                Err(PolicyValidationError::InconsistentAuditRecord)
            }
            _ => Ok(()),
        }
    }
}

impl<'de> Deserialize<'de> for PolicyReason {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = PolicyReasonWire::deserialize(deserializer)?;
        Self::new(
            wire.code,
            wire.effective_risk,
            wire.policy_revision,
            wire.grant_id,
            wire.grant_revision,
        )
        .map_err(serde::de::Error::custom)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyReasonWire {
    code: PolicyReasonCode,
    effective_risk: RiskLevel,
    policy_revision: u32,
    grant_id: Option<String>,
    grant_revision: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum PolicyDecision {
    Allow(PolicyReason),
    RequireApproval(PolicyReason),
    Deny(PolicyReason),
}

impl PolicyDecision {
    pub fn allow(reason: PolicyReason) -> Result<Self, PolicyValidationError> {
        let decision = Self::Allow(reason);
        decision.validate()?;
        Ok(decision)
    }

    pub fn require_approval(reason: PolicyReason) -> Result<Self, PolicyValidationError> {
        let decision = Self::RequireApproval(reason);
        decision.validate()?;
        Ok(decision)
    }

    pub fn deny(reason: PolicyReason) -> Result<Self, PolicyValidationError> {
        let decision = Self::Deny(reason);
        decision.validate()?;
        Ok(decision)
    }

    pub fn reason(&self) -> &PolicyReason {
        match self {
            Self::Allow(reason) | Self::RequireApproval(reason) | Self::Deny(reason) => reason,
        }
    }

    /// The stable reason code, useful for safe audits and metrics.
    pub fn kind(&self) -> PolicyReasonCode {
        self.reason().code
    }

    fn validate(&self) -> Result<(), PolicyValidationError> {
        self.reason().validate()?;
        match self {
            Self::Allow(reason)
                if matches!(
                    reason.code,
                    PolicyReasonCode::AllowedByDefault
                        | PolicyReasonCode::AllowedByGrant
                        | PolicyReasonCode::AllowedByApproval
                ) =>
            {
                Ok(())
            }
            Self::RequireApproval(reason) if reason.code == PolicyReasonCode::ApprovalRequired => {
                Ok(())
            }
            Self::Deny(reason)
                if matches!(
                    reason.code,
                    PolicyReasonCode::DeniedByDefault | PolicyReasonCode::DeniedByRestriction
                ) =>
            {
                Ok(())
            }
            _ => Err(PolicyValidationError::InconsistentAuditRecord),
        }
    }
}

impl<'de> Deserialize<'de> for PolicyDecision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let decision = PolicyDecisionWire::deserialize(deserializer)?.into_decision();
        decision.validate().map_err(serde::de::Error::custom)?;
        Ok(decision)
    }
}

#[derive(Deserialize)]
enum PolicyDecisionWire {
    Allow(PolicyReason),
    RequireApproval(PolicyReason),
    Deny(PolicyReason),
}

impl PolicyDecisionWire {
    fn into_decision(self) -> PolicyDecision {
        match self {
            Self::Allow(reason) => PolicyDecision::Allow(reason),
            Self::RequireApproval(reason) => PolicyDecision::RequireApproval(reason),
            Self::Deny(reason) => PolicyDecision::Deny(reason),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GrantScope {
    pub owner_id: String,
    pub actor_id: String,
    pub agent_definition_id: String,
    pub agent_definition_version: u32,
    pub workspace_id: String,
    pub resource_boundary: String,
    pub capability_id: String,
    pub manifest_version: u32,
    pub canonical_argument_digest: Option<Uuid>,
}

impl GrantScope {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        owner_id: impl Into<String>,
        actor_id: impl Into<String>,
        agent_definition_id: impl Into<String>,
        agent_definition_version: u32,
        workspace_id: impl Into<String>,
        resource_boundary: impl Into<String>,
        capability_id: impl Into<String>,
        manifest_version: u32,
        canonical_argument_digest: Option<Uuid>,
    ) -> Result<Self, PolicyValidationError> {
        let scope = Self {
            owner_id: owner_id.into(),
            actor_id: actor_id.into(),
            agent_definition_id: agent_definition_id.into(),
            agent_definition_version,
            workspace_id: workspace_id.into(),
            resource_boundary: resource_boundary.into(),
            capability_id: capability_id.into(),
            manifest_version,
            canonical_argument_digest,
        };
        scope.validate()?;
        Ok(scope)
    }

    fn validate(&self) -> Result<(), PolicyValidationError> {
        for (field, value) in [
            ("owner_id", &self.owner_id),
            ("actor_id", &self.actor_id),
            ("agent_definition_id", &self.agent_definition_id),
            ("workspace_id", &self.workspace_id),
            ("resource_boundary", &self.resource_boundary),
            ("capability_id", &self.capability_id),
        ] {
            validate_id(field, value)?;
        }
        if self.agent_definition_version == 0 || self.manifest_version == 0 {
            return Err(PolicyValidationError::InvalidVersion);
        }
        if self
            .canonical_argument_digest
            .is_some_and(|digest| digest.is_nil())
        {
            return Err(PolicyValidationError::InvalidNilIdentifier);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for GrantScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = GrantScopeWire::deserialize(deserializer)?;
        Self::new(
            wire.owner_id,
            wire.actor_id,
            wire.agent_definition_id,
            wire.agent_definition_version,
            wire.workspace_id,
            wire.resource_boundary,
            wire.capability_id,
            wire.manifest_version,
            wire.canonical_argument_digest,
        )
        .map_err(serde::de::Error::custom)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantScopeWire {
    owner_id: String,
    actor_id: String,
    agent_definition_id: String,
    agent_definition_version: u32,
    workspace_id: String,
    resource_boundary: String,
    capability_id: String,
    manifest_version: u32,
    canonical_argument_digest: Option<Uuid>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GrantStatus {
    Active,
    Revoked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AutonomyGrant {
    pub id: String,
    pub revision: u32,
    pub status: GrantStatus,
    pub scope: GrantScope,
    pub maximum_risk: RiskLevel,
    pub valid_from_ms: i64,
    pub valid_until_ms: Option<i64>,
    /// `Some(n)` is a remaining-use count observed by the caller. Evaluation never decrements it.
    pub remaining_uses: Option<u32>,
}

impl AutonomyGrant {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        revision: u32,
        status: GrantStatus,
        scope: GrantScope,
        maximum_risk: RiskLevel,
        valid_from_ms: i64,
        valid_until_ms: Option<i64>,
        remaining_uses: Option<u32>,
    ) -> Result<Self, PolicyValidationError> {
        let grant = Self {
            id: id.into(),
            revision,
            status,
            scope,
            maximum_risk,
            valid_from_ms,
            valid_until_ms,
            remaining_uses,
        };
        grant.validate()?;
        Ok(grant)
    }

    fn validate(&self) -> Result<(), PolicyValidationError> {
        validate_id("grant_id", &self.id)?;
        if self.revision == 0 {
            return Err(PolicyValidationError::InvalidVersion);
        }
        self.scope.validate()?;
        if self.maximum_risk == RiskLevel::None {
            return Err(PolicyValidationError::InvalidRiskConstraint);
        }
        if self
            .valid_until_ms
            .is_some_and(|until| until <= self.valid_from_ms)
        {
            return Err(PolicyValidationError::InvalidValidityWindow);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for AutonomyGrant {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = AutonomyGrantWire::deserialize(deserializer)?;
        Self::new(
            wire.id,
            wire.revision,
            wire.status,
            wire.scope,
            wire.maximum_risk,
            wire.valid_from_ms,
            wire.valid_until_ms,
            wire.remaining_uses,
        )
        .map_err(serde::de::Error::custom)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AutonomyGrantWire {
    id: String,
    revision: u32,
    status: GrantStatus,
    scope: GrantScope,
    maximum_risk: RiskLevel,
    valid_from_ms: i64,
    valid_until_ms: Option<i64>,
    remaining_uses: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GrantConsumption {
    pub grant_id: String,
    pub grant_revision: u32,
    pub logical_invocation_id: Uuid,
}

impl GrantConsumption {
    pub fn new(
        grant_id: impl Into<String>,
        grant_revision: u32,
        logical_invocation_id: Uuid,
    ) -> Result<Self, PolicyValidationError> {
        let consumption = Self {
            grant_id: grant_id.into(),
            grant_revision,
            logical_invocation_id,
        };
        consumption.validate()?;
        Ok(consumption)
    }

    fn validate(&self) -> Result<(), PolicyValidationError> {
        validate_id("grant_id", &self.grant_id)?;
        if self.grant_revision == 0 {
            return Err(PolicyValidationError::InvalidVersion);
        }
        if self.logical_invocation_id.is_nil() {
            return Err(PolicyValidationError::InvalidNilIdentifier);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for GrantConsumption {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = GrantConsumptionWire::deserialize(deserializer)?;
        Self::new(
            wire.grant_id,
            wire.grant_revision,
            wire.logical_invocation_id,
        )
        .map_err(serde::de::Error::custom)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantConsumptionWire {
    grant_id: String,
    grant_revision: u32,
    logical_invocation_id: Uuid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyEvaluation {
    pub decision: PolicyDecision,
    pub consumption: Option<GrantConsumption>,
}

impl PolicyEvaluation {
    pub fn new(
        decision: PolicyDecision,
        consumption: Option<GrantConsumption>,
    ) -> Result<Self, PolicyValidationError> {
        decision.validate()?;
        if let Some(consumption) = &consumption {
            consumption.validate()?;
            let PolicyDecision::Allow(reason) = &decision else {
                return Err(PolicyValidationError::InconsistentAuditRecord);
            };
            if reason.code != PolicyReasonCode::AllowedByGrant
                || reason.grant_id.as_deref() != Some(consumption.grant_id.as_str())
                || reason.grant_revision != Some(consumption.grant_revision)
            {
                return Err(PolicyValidationError::InconsistentAuditRecord);
            }
        }
        Ok(Self {
            decision,
            consumption,
        })
    }
}

impl<'de> Deserialize<'de> for PolicyEvaluation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = PolicyEvaluationWire::deserialize(deserializer)?;
        Self::new(wire.decision, wire.consumption).map_err(serde::de::Error::custom)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyEvaluationWire {
    decision: PolicyDecision,
    consumption: Option<GrantConsumption>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRequest {
    pub owner_id: String,
    pub actor_id: String,
    pub run_id: Uuid,
    pub logical_step_id: String,
    pub logical_invocation_id: Uuid,
    pub capability_id: String,
    pub manifest_version: u32,
    pub canonical_argument_digest: Uuid,
    pub effective_risk: RiskLevel,
    pub reason: PolicyReason,
    pub policy_revision: u32,
    pub grant_id: Option<String>,
    pub grant_revision: Option<u32>,
    pub requested_at_ms: i64,
    pub expires_at_ms: i64,
}

impl ApprovalRequest {
    fn validate(&self) -> Result<(), PolicyValidationError> {
        self.reason.validate()?;
        for (field, value) in [
            ("owner_id", &self.owner_id),
            ("actor_id", &self.actor_id),
            ("logical_step_id", &self.logical_step_id),
            ("capability_id", &self.capability_id),
        ] {
            validate_id(field, value)?;
        }
        if self.manifest_version == 0 || self.policy_revision == 0 {
            return Err(PolicyValidationError::InvalidVersion);
        }
        if self.run_id.is_nil()
            || self.logical_invocation_id.is_nil()
            || self.canonical_argument_digest.is_nil()
        {
            return Err(PolicyValidationError::InvalidNilIdentifier);
        }
        if self.requested_at_ms < 0
            || self.expires_at_ms != self.requested_at_ms.saturating_add(APPROVAL_WINDOW_MS)
        {
            return Err(PolicyValidationError::InvalidApprovalTime);
        }
        if self.effective_risk == RiskLevel::None
            || self.reason.effective_risk != self.effective_risk
            || self.reason.policy_revision != self.policy_revision
            || self.reason.code != PolicyReasonCode::ApprovalRequired
            || self.reason.grant_id != self.grant_id
            || self.reason.grant_revision != self.grant_revision
        {
            return Err(PolicyValidationError::InconsistentApprovalBinding);
        }
        match (&self.grant_id, self.grant_revision) {
            (Some(id), Some(revision)) if !id.trim().is_empty() && revision > 0 => Ok(()),
            (None, None) => Ok(()),
            _ => Err(PolicyValidationError::InconsistentApprovalBinding),
        }
    }
}

impl<'de> Deserialize<'de> for ApprovalRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let request = ApprovalRequestWire::deserialize(deserializer)?.into_request();
        request.validate().map_err(serde::de::Error::custom)?;
        Ok(request)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovalRequestWire {
    owner_id: String,
    actor_id: String,
    run_id: Uuid,
    logical_step_id: String,
    logical_invocation_id: Uuid,
    capability_id: String,
    manifest_version: u32,
    canonical_argument_digest: Uuid,
    effective_risk: RiskLevel,
    reason: PolicyReason,
    policy_revision: u32,
    grant_id: Option<String>,
    grant_revision: Option<u32>,
    requested_at_ms: i64,
    expires_at_ms: i64,
}

impl ApprovalRequestWire {
    fn into_request(self) -> ApprovalRequest {
        ApprovalRequest {
            owner_id: self.owner_id,
            actor_id: self.actor_id,
            run_id: self.run_id,
            logical_step_id: self.logical_step_id,
            logical_invocation_id: self.logical_invocation_id,
            capability_id: self.capability_id,
            manifest_version: self.manifest_version,
            canonical_argument_digest: self.canonical_argument_digest,
            effective_risk: self.effective_risk,
            reason: self.reason,
            policy_revision: self.policy_revision,
            grant_id: self.grant_id,
            grant_revision: self.grant_revision,
            requested_at_ms: self.requested_at_ms,
            expires_at_ms: self.expires_at_ms,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalDecisionKind {
    Approve,
    Deny,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalDecision {
    pub request: ApprovalRequest,
    pub kind: ApprovalDecisionKind,
    pub decided_at_ms: i64,
    #[serde(skip)]
    binding: Option<ApprovalDecisionBinding>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ApprovalDecisionBinding {
    request: ApprovalRequest,
    kind: ApprovalDecisionKind,
    decided_at_ms: i64,
}

impl ApprovalDecision {
    pub fn new_approved(
        request: ApprovalRequest,
        decided_at_ms: i64,
    ) -> Result<Self, PolicyValidationError> {
        Self::new(request, ApprovalDecisionKind::Approve, decided_at_ms)
    }

    pub fn new_denied(
        request: ApprovalRequest,
        decided_at_ms: i64,
    ) -> Result<Self, PolicyValidationError> {
        Self::new(request, ApprovalDecisionKind::Deny, decided_at_ms)
    }

    pub fn new(
        request: ApprovalRequest,
        kind: ApprovalDecisionKind,
        decided_at_ms: i64,
    ) -> Result<Self, PolicyValidationError> {
        let decision = Self {
            binding: Some(ApprovalDecisionBinding {
                request: request.clone(),
                kind,
                decided_at_ms,
            }),
            request,
            kind,
            decided_at_ms,
        };
        decision.validate()?;
        Ok(decision)
    }

    fn validate(&self) -> Result<(), PolicyValidationError> {
        self.request.validate()?;
        let Some(binding) = &self.binding else {
            return Err(PolicyValidationError::InconsistentApprovalBinding);
        };
        if binding.request != self.request
            || binding.kind != self.kind
            || binding.decided_at_ms != self.decided_at_ms
        {
            return Err(PolicyValidationError::InconsistentApprovalBinding);
        }
        if self.decided_at_ms < self.request.requested_at_ms
            || self.decided_at_ms >= self.request.expires_at_ms
        {
            return Err(PolicyValidationError::InvalidApprovalTime);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for ApprovalDecision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ApprovalDecisionWire::deserialize(deserializer)?;
        Self::new(wire.request, wire.kind, wire.decided_at_ms).map_err(serde::de::Error::custom)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovalDecisionWire {
    request: ApprovalRequest,
    kind: ApprovalDecisionKind,
    decided_at_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalValidity {
    Valid,
    InvalidBinding,
    InvalidDecision,
    InvalidOwner,
    InvalidActor,
    InvalidRun,
    InvalidStep,
    InvalidInvocation,
    InvalidManifest,
    InvalidArguments,
    InvalidRisk,
    Expired,
    InvalidGrant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyValidationError {
    BlankIdentifier { field: &'static str },
    InvalidVersion,
    InvalidRiskConstraint,
    UnverifiedPolicyContext,
    ContextManifestMismatch,
    ContextInvocationMismatch,
    InvalidValidityWindow,
    InconsistentApprovalBinding,
    InvalidApprovalTime,
    SuppliedGrantDoesNotMatch,
    InvalidNilIdentifier,
    InconsistentAuditRecord,
}

impl fmt::Display for PolicyValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlankIdentifier { field } => write!(formatter, "{field} must not be blank"),
            Self::InvalidVersion => formatter.write_str("policy versions must be nonzero"),
            Self::InvalidRiskConstraint => {
                formatter.write_str("policy risk constraints are invalid")
            }
            Self::UnverifiedPolicyContext => {
                formatter.write_str("policy context has no verified input provenance")
            }
            Self::ContextManifestMismatch => {
                formatter.write_str("policy context does not match its pinned manifest")
            }
            Self::ContextInvocationMismatch => {
                formatter.write_str("policy context does not match its logical invocation")
            }
            Self::InvalidValidityWindow => formatter.write_str("policy validity window is invalid"),
            Self::InconsistentApprovalBinding => {
                formatter.write_str("approval bindings are inconsistent")
            }
            Self::InvalidApprovalTime => {
                formatter.write_str("approval decision is outside its request window")
            }
            Self::SuppliedGrantDoesNotMatch => {
                formatter.write_str("supplied autonomy grant does not match the policy context")
            }
            Self::InvalidNilIdentifier => formatter.write_str("policy identifier must not be nil"),
            Self::InconsistentAuditRecord => {
                formatter.write_str("policy audit record is inconsistent")
            }
        }
    }
}

impl std::error::Error for PolicyValidationError {}

pub struct PolicyEngine;

impl PolicyEngine {
    pub fn evaluate(
        context: &PolicyContext,
        grants: &[AutonomyGrant],
    ) -> Result<PolicyEvaluation, PolicyValidationError> {
        context.validate_evaluation_input()?;
        let effective_risk = context.effective_risk();
        if let Some(grant) = select_grant(context, grants) {
            let decision = PolicyDecision::Allow(reason(
                PolicyReasonCode::AllowedByGrant,
                effective_risk,
                context.policy_revision,
                Some(grant),
            ));
            let consumption = grant
                .remaining_uses
                .map(|_| {
                    GrantConsumption::new(
                        grant.id.clone(),
                        grant.revision,
                        context.logical_invocation_id,
                    )
                })
                .transpose()?;
            return PolicyEvaluation::new(decision, consumption);
        }
        let decision = if context.restrictions.deny {
            PolicyDecision::Deny(reason(
                PolicyReasonCode::DeniedByRestriction,
                effective_risk,
                context.policy_revision,
                None,
            ))
        } else if effective_risk == RiskLevel::Critical {
            PolicyDecision::Deny(reason(
                PolicyReasonCode::DeniedByDefault,
                effective_risk,
                context.policy_revision,
                None,
            ))
        } else if risk_rank(effective_risk) >= risk_rank(RiskLevel::Medium) {
            PolicyDecision::RequireApproval(reason(
                PolicyReasonCode::ApprovalRequired,
                effective_risk,
                context.policy_revision,
                None,
            ))
        } else {
            PolicyDecision::Allow(reason(
                PolicyReasonCode::AllowedByDefault,
                effective_risk,
                context.policy_revision,
                None,
            ))
        };
        PolicyEvaluation::new(decision, None)
    }

    pub fn evaluate_with_approval(
        context: &PolicyContext,
        grants: &[AutonomyGrant],
        approval: Option<&ApprovalDecision>,
    ) -> Result<PolicyEvaluation, PolicyValidationError> {
        let initial = Self::evaluate(context, grants)?;
        if !matches!(initial.decision, PolicyDecision::RequireApproval(_)) {
            return Ok(initial);
        }
        let Some(approval) = approval else {
            return Ok(initial);
        };
        if Self::validate_approval_with_grants(approval, context, grants) == ApprovalValidity::Valid
        {
            return PolicyEvaluation::new(
                PolicyDecision::Allow(reason(
                    PolicyReasonCode::AllowedByApproval,
                    context.effective_risk(),
                    context.policy_revision,
                    None,
                )),
                None,
            );
        }
        Ok(initial)
    }

    pub fn approval_request(
        context: &PolicyContext,
        grant: Option<&AutonomyGrant>,
    ) -> Result<ApprovalRequest, PolicyValidationError> {
        context.validate_evaluation_input()?;
        if grant.is_some_and(|grant| !Self::grant_matches(grant, context)) {
            return Err(PolicyValidationError::SuppliedGrantDoesNotMatch);
        }
        let request = ApprovalRequest {
            owner_id: context.owner_id.clone(),
            actor_id: context.actor_id.clone(),
            run_id: context.run_id,
            logical_step_id: context.logical_step_id.clone(),
            logical_invocation_id: context.logical_invocation_id,
            capability_id: context.capability_id.clone(),
            manifest_version: context.manifest_version,
            canonical_argument_digest: context.canonical_argument_digest,
            effective_risk: context.effective_risk(),
            reason: reason(
                PolicyReasonCode::ApprovalRequired,
                context.effective_risk(),
                context.policy_revision,
                grant,
            ),
            policy_revision: context.policy_revision,
            grant_id: grant.map(|grant| grant.id.clone()),
            grant_revision: grant.map(|grant| grant.revision),
            requested_at_ms: context.now_ms,
            expires_at_ms: context.now_ms.saturating_add(APPROVAL_WINDOW_MS),
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate_approval(
        approval: &ApprovalDecision,
        context: &PolicyContext,
    ) -> ApprovalValidity {
        Self::validate_approval_with_grants(approval, context, &[])
    }

    pub fn grant_matches(grant: &AutonomyGrant, context: &PolicyContext) -> bool {
        context.validate_evaluation_input().is_ok()
            && grant.validate().is_ok()
            && grant.status == GrantStatus::Active
            && grant.scope.owner_id == context.owner_id
            && grant.scope.actor_id == context.actor_id
            && grant.scope.agent_definition_id == context.agent_definition_id
            && grant.scope.agent_definition_version == context.agent_definition_version
            && grant.scope.workspace_id == context.workspace_id
            && grant.scope.resource_boundary == context.resource_boundary
            && grant.scope.capability_id == context.capability_id
            && grant.scope.manifest_version == context.manifest_version
            && grant
                .scope
                .canonical_argument_digest
                .is_none_or(|digest| digest == context.canonical_argument_digest)
            && risk_rank(grant.maximum_risk) >= risk_rank(context.effective_risk())
            && context.now_ms >= grant.valid_from_ms
            && grant
                .valid_until_ms
                .is_none_or(|until| context.now_ms < until)
            && grant.remaining_uses.is_none_or(|uses| uses > 0)
    }

    fn validate_approval_with_grants(
        approval: &ApprovalDecision,
        context: &PolicyContext,
        grants: &[AutonomyGrant],
    ) -> ApprovalValidity {
        if context.validate_evaluation_input().is_err() || approval.validate().is_err() {
            return ApprovalValidity::InvalidBinding;
        }
        if approval.kind != ApprovalDecisionKind::Approve {
            return ApprovalValidity::InvalidDecision;
        }
        if context.now_ms < approval.decided_at_ms {
            return ApprovalValidity::InvalidBinding;
        }
        if context.now_ms >= approval.request.expires_at_ms {
            return ApprovalValidity::Expired;
        }
        let request = &approval.request;
        if request.owner_id != context.owner_id {
            return ApprovalValidity::InvalidOwner;
        }
        if request.actor_id != context.actor_id {
            return ApprovalValidity::InvalidActor;
        }
        if request.run_id != context.run_id {
            return ApprovalValidity::InvalidRun;
        }
        if request.logical_step_id != context.logical_step_id {
            return ApprovalValidity::InvalidStep;
        }
        if request.canonical_argument_digest != context.canonical_argument_digest {
            return ApprovalValidity::InvalidArguments;
        }
        if request.logical_invocation_id != context.logical_invocation_id {
            return ApprovalValidity::InvalidInvocation;
        }
        if request.capability_id != context.capability_id
            || request.manifest_version != context.manifest_version
        {
            return ApprovalValidity::InvalidManifest;
        }
        if request.effective_risk != context.effective_risk() {
            return ApprovalValidity::InvalidRisk;
        }
        match (&request.grant_id, request.grant_revision) {
            (None, None) => ApprovalValidity::Valid,
            (Some(id), Some(revision))
                if grants.iter().any(|grant| {
                    grant.id == *id
                        && grant.revision == revision
                        && Self::grant_matches(grant, context)
                }) =>
            {
                ApprovalValidity::Valid
            }
            _ => ApprovalValidity::InvalidGrant,
        }
    }
}

/// Matching grants are sorted by narrowness: argument-bound scopes first, then lower maximum
/// risk, then finite-use grants, then a stable lexical grant ID and highest revision tie-break.
/// Input order is ignored.
fn select_grant<'a>(
    context: &PolicyContext,
    grants: &'a [AutonomyGrant],
) -> Option<&'a AutonomyGrant> {
    grants
        .iter()
        .filter(|grant| PolicyEngine::grant_matches(grant, context))
        .min_by_key(|grant| {
            (
                grant.scope.canonical_argument_digest.is_none(),
                risk_rank(grant.maximum_risk),
                grant.remaining_uses.is_none(),
                Reverse(grant.valid_from_ms),
                &grant.id,
                Reverse(grant.revision),
            )
        })
}

fn invocation_step_id(invocation: &LogicalInvocation) -> String {
    // The invocation identity already binds this stable step label. Expose it only as a safe ID.
    // A dedicated accessor preserves policy's no-raw-arguments boundary.
    invocation.logical_step_id().to_owned()
}

fn reason(
    code: PolicyReasonCode,
    effective_risk: RiskLevel,
    policy_revision: u32,
    grant: Option<&AutonomyGrant>,
) -> PolicyReason {
    PolicyReason::new(
        code,
        effective_risk,
        policy_revision,
        grant.map(|grant| grant.id.clone()),
        grant.map(|grant| grant.revision),
    )
    .expect("policy engine only emits validated audit reasons")
}

fn validate_id(field: &'static str, value: &str) -> Result<(), PolicyValidationError> {
    if value.trim().is_empty() {
        return Err(PolicyValidationError::BlankIdentifier { field });
    }
    Ok(())
}

fn risk_rank(risk: RiskLevel) -> u8 {
    match risk {
        RiskLevel::None => 0,
        RiskLevel::Low => 1,
        RiskLevel::Medium => 2,
        RiskLevel::High => 3,
        RiskLevel::Critical => 4,
    }
}

fn max_risk(left: RiskLevel, right: RiskLevel) -> RiskLevel {
    if risk_rank(left) >= risk_rank(right) {
        left
    } else {
        right
    }
}
