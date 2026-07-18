use anima_core::{
    ApprovalDecision, ApprovalValidity, AutonomyGrant, CapabilityKind, CapabilityManifest,
    CapabilityReferenceId, GrantConsumption, GrantScope, GrantStatus, LogicalInvocation,
    PolicyContext, PolicyDecision, PolicyEngine, PolicyEvaluation, PolicyReason, PolicyReasonCode,
    PolicyRestrictions, PolicyValidationError, RiskLevel, RuntimeCompatibility,
};
use serde_json::json;
use uuid::Uuid;

fn manifest(risk_level: RiskLevel, version: u32) -> CapabilityManifest {
    CapabilityManifest {
        id: "workspace.write".into(),
        version,
        kind: CapabilityKind::Workspace,
        label: "Write workspace file".into(),
        description: "Writes a selected workspace resource.".into(),
        input_schema: json!({ "type": "object" }),
        output_schema: json!({ "type": "object" }),
        side_effects: true,
        risk_level,
        host_permissions: vec![],
        secret_references: vec![],
        environment_requirements: vec![],
        timeout_ms: 1_000,
        cancellation_supported: true,
        max_retries: 0,
        idempotent: false,
        recovery_mode: anima_core::RecoveryMode::NonRetryable,
        supports_streaming: false,
        supports_artifacts: false,
        supports_citations: false,
        schema_digest: format!("sha256:workspace.write:{version}"),
        compatibility: RuntimeCompatibility {
            minimum_runtime_schema_version: 1,
            maximum_runtime_schema_version: 1,
            manifest_schema_version: 1,
        },
    }
}

fn invocation(arguments: serde_json::Value) -> LogicalInvocation {
    LogicalInvocation::new(
        Uuid::from_u128(1),
        "write-report",
        "workspace.write",
        1,
        arguments,
    )
    .unwrap()
}

fn context(
    risk: RiskLevel,
    invocation: LogicalInvocation,
    restrictions: PolicyRestrictions,
) -> PolicyContext {
    PolicyContext::new(
        "owner-1",
        "actor-1",
        "writer-agent",
        1,
        "workspace-1",
        CapabilityReferenceId::new(Uuid::from_u128(10)),
        &manifest(risk, invocation.manifest_version()),
        &invocation,
        1,
        restrictions,
        1_000,
    )
    .unwrap()
}

fn scope(context: &PolicyContext) -> GrantScope {
    GrantScope::new(
        context.owner_id.clone(),
        context.actor_id.clone(),
        context.agent_definition_id.clone(),
        context.agent_definition_version,
        context.workspace_id.clone(),
        context.resource_boundary.clone(),
        context.capability_id.clone(),
        context.manifest_version,
        Some(context.canonical_argument_digest),
    )
    .unwrap()
}

fn grant(context: &PolicyContext, maximum_risk: RiskLevel) -> AutonomyGrant {
    AutonomyGrant::new(
        "grant-1",
        1,
        GrantStatus::Active,
        scope(context),
        maximum_risk,
        500,
        Some(2_000),
        Some(1),
    )
    .unwrap()
}

#[test]
fn default_ladder_is_allow_approval_and_critical_deny_without_an_exact_grant() {
    for risk in [
        RiskLevel::Low,
        RiskLevel::Medium,
        RiskLevel::High,
        RiskLevel::Critical,
    ] {
        let context = context(
            risk,
            invocation(json!({ "path": "reports/a.md" })),
            Default::default(),
        );
        let decision = PolicyEngine::evaluate(&context, &[]).unwrap().decision;
        assert_eq!(
            decision.kind(),
            match risk {
                RiskLevel::Low => PolicyReasonCode::AllowedByDefault,
                RiskLevel::Medium | RiskLevel::High => PolicyReasonCode::ApprovalRequired,
                RiskLevel::Critical => PolicyReasonCode::DeniedByDefault,
                RiskLevel::None => unreachable!(),
            }
        );
    }

    let context = context(
        RiskLevel::Critical,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    assert!(matches!(
        PolicyEngine::evaluate(&context, &[grant(&context, RiskLevel::Critical)])
            .unwrap()
            .decision,
        PolicyDecision::Allow(_)
    ));
}

#[test]
fn restrictions_only_make_policy_stricter_without_a_matching_grant() {
    let invocation = invocation(json!({ "path": "reports/a.md" }));
    let context = context(
        RiskLevel::Low,
        invocation,
        PolicyRestrictions {
            minimum_risk: Some(RiskLevel::High),
            deny: false,
        },
    );
    assert!(matches!(
        PolicyEngine::evaluate(&context, &[]).unwrap().decision,
        PolicyDecision::RequireApproval(_)
    ));
    assert!(matches!(
        PolicyEngine::evaluate(&context, &[grant(&context, RiskLevel::High)])
            .unwrap()
            .decision,
        PolicyDecision::Allow(_)
    ));
}

#[test]
fn grants_match_every_scope_boundary_and_lifetime_constraint() {
    let context = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    let exact = grant(&context, RiskLevel::High);
    assert!(PolicyEngine::grant_matches(&exact, &context));

    let mut wrong_actor = exact.clone();
    wrong_actor.scope.actor_id = "other".into();
    let mut wrong_agent = exact.clone();
    wrong_agent.scope.agent_definition_version = 2;
    let mut wrong_resource = exact.clone();
    wrong_resource.scope.resource_boundary = CapabilityReferenceId::new(Uuid::from_u128(11));
    let mut wrong_workspace = exact.clone();
    wrong_workspace.scope.workspace_id = "other".into();
    let mut wrong_capability = exact.clone();
    wrong_capability.scope.capability_id = "other.write".into();
    let mut wrong_manifest_version = exact.clone();
    wrong_manifest_version.scope.manifest_version = 2;
    let mut wrong_args = exact.clone();
    wrong_args.scope.canonical_argument_digest = Some(Uuid::from_u128(99));
    let mut expired = exact.clone();
    expired.valid_until_ms = Some(999);
    let mut not_yet_valid = exact.clone();
    not_yet_valid.valid_from_ms = 1_001;
    let mut revoked = exact.clone();
    revoked.status = GrantStatus::Revoked;
    let exhausted = AutonomyGrant::new(
        "exhausted-grant",
        1,
        GrantStatus::Active,
        scope(&context),
        RiskLevel::High,
        500,
        Some(2_000),
        Some(0),
    )
    .unwrap();
    let mut too_low_risk = exact.clone();
    too_low_risk.maximum_risk = RiskLevel::Medium;

    for non_match in [
        wrong_actor,
        wrong_agent,
        wrong_resource,
        wrong_workspace,
        wrong_capability,
        wrong_manifest_version,
        wrong_args,
        expired,
        not_yet_valid,
        revoked,
        exhausted,
        too_low_risk,
    ] {
        assert!(!PolicyEngine::grant_matches(&non_match, &context));
    }
}

#[test]
fn approvals_bind_every_action_identity_and_their_reason_revision() {
    let original = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    let approval = ApprovalDecision::new_approved(
        PolicyEngine::approval_request(&original, None).unwrap(),
        1_000,
    )
    .unwrap();
    let mut changed_actor = original.clone();
    changed_actor.actor_id = "actor-2".into();
    let mut changed_run = original.clone();
    changed_run.run_id = Uuid::from_u128(2);
    let mut changed_step = original.clone();
    changed_step.logical_step_id = "other-step".into();
    let mut changed_invocation = original.clone();
    changed_invocation.logical_invocation_id = Uuid::from_u128(99);
    let mut changed_manifest = original.clone();
    changed_manifest.manifest_version = 2;
    let mut expired = original.clone();
    expired.now_ms = 301_000;

    assert_eq!(
        PolicyEngine::validate_approval(&approval, &changed_actor),
        ApprovalValidity::InvalidActor
    );
    assert_eq!(
        PolicyEngine::validate_approval(&approval, &changed_run),
        ApprovalValidity::InvalidBinding
    );
    assert_eq!(
        PolicyEngine::validate_approval(&approval, &changed_step),
        ApprovalValidity::InvalidBinding
    );
    assert_eq!(
        PolicyEngine::validate_approval(&approval, &changed_invocation),
        ApprovalValidity::InvalidBinding
    );
    assert_eq!(
        PolicyEngine::validate_approval(&approval, &changed_manifest),
        ApprovalValidity::InvalidBinding
    );
    assert_eq!(
        PolicyEngine::validate_approval(&approval, &expired),
        ApprovalValidity::Expired
    );

    let mut malformed = serde_json::to_value(&approval).unwrap();
    malformed["request"]["reason"]["policy_revision"] = json!(2);
    assert!(serde_json::from_value::<ApprovalDecision>(malformed).is_err());
}

#[test]
fn approvals_cannot_be_replayed_across_definition_workspace_or_resource_scope() {
    let original = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    let approval = ApprovalDecision::new_approved(
        PolicyEngine::approval_request(&original, None).unwrap(),
        1_000,
    )
    .unwrap();

    let mut other_definition = original.clone();
    other_definition.agent_definition_id = "other-agent".into();
    let mut other_definition_version = original.clone();
    other_definition_version.agent_definition_version = 2;
    let mut other_workspace = original.clone();
    other_workspace.workspace_id = "other-workspace".into();
    let mut other_resource = original.clone();
    other_resource.resource_boundary = CapabilityReferenceId::new(Uuid::from_u128(11));
    let mut other_restrictions = original.clone();
    other_restrictions.restrictions.deny = true;

    for replay in [
        other_definition,
        other_definition_version,
        other_workspace,
        other_resource,
        other_restrictions,
    ] {
        assert_ne!(
            PolicyEngine::validate_approval(&approval, &replay),
            ApprovalValidity::Valid
        );
    }

    let mut serialized = serde_json::to_value(&approval).unwrap();
    serialized["request"]["workspace_id"] = json!("other-workspace");
    let restored: ApprovalDecision = serde_json::from_value(serialized).unwrap();
    assert_eq!(
        PolicyEngine::validate_approval(&restored, &original),
        ApprovalValidity::InvalidWorkspace
    );
}

#[test]
fn critical_and_deny_overrides_require_an_argument_bound_grant() {
    let critical = context(
        RiskLevel::Critical,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    let mut broad_critical = grant(&critical, RiskLevel::Critical);
    broad_critical.scope.canonical_argument_digest = None;
    assert!(matches!(
        PolicyEngine::evaluate(&critical, &[broad_critical])
            .unwrap()
            .decision,
        PolicyDecision::Deny(_)
    ));
    assert_eq!(
        PolicyEngine::evaluate(&critical, &[grant(&critical, RiskLevel::Critical)])
            .unwrap()
            .decision
            .kind(),
        PolicyReasonCode::AllowedByExactGrantOverride
    );

    let denied = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/a.md" })),
        PolicyRestrictions {
            minimum_risk: None,
            deny: true,
        },
    );
    let mut broad_deny = grant(&denied, RiskLevel::High);
    broad_deny.scope.canonical_argument_digest = None;
    assert!(matches!(
        PolicyEngine::evaluate(&denied, &[broad_deny])
            .unwrap()
            .decision,
        PolicyDecision::Deny(_)
    ));
}

#[test]
fn grants_prefer_lower_remaining_uses_then_earlier_expiry_independent_of_input_order() {
    let context = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    let mut single_use = grant(&context, RiskLevel::High);
    single_use.id = "z-single-use".into();
    single_use.remaining_uses = Some(1);
    single_use.valid_until_ms = Some(2_000);
    let mut hundred_uses = grant(&context, RiskLevel::High);
    hundred_uses.id = "a-hundred-uses".into();
    hundred_uses.remaining_uses = Some(100);
    hundred_uses.valid_until_ms = Some(1_100);
    assert_eq!(
        PolicyEngine::evaluate(&context, &[hundred_uses, single_use])
            .unwrap()
            .consumption
            .unwrap()
            .grant_id,
        "z-single-use"
    );

    let mut imminent = grant(&context, RiskLevel::High);
    imminent.id = "z-imminent".into();
    imminent.remaining_uses = Some(1);
    imminent.valid_until_ms = Some(1_100);
    let mut long_lived = grant(&context, RiskLevel::High);
    long_lived.id = "a-long-lived".into();
    long_lived.remaining_uses = Some(1);
    long_lived.valid_until_ms = Some(2_000);
    assert_eq!(
        PolicyEngine::evaluate(&context, &[long_lived, imminent])
            .unwrap()
            .consumption
            .unwrap()
            .grant_id,
        "z-imminent"
    );
}

#[test]
fn approval_requests_only_exist_for_current_approval_decisions_and_exact_windows() {
    let low = context(
        RiskLevel::Low,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    assert!(PolicyEngine::approval_request(&low, None).is_err());

    let critical = context(
        RiskLevel::Critical,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    assert!(PolicyEngine::approval_request(&critical, None).is_err());

    let restricted = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/a.md" })),
        PolicyRestrictions {
            minimum_risk: None,
            deny: true,
        },
    );
    assert!(matches!(
        PolicyEngine::approval_request(&restricted, None),
        Err(PolicyValidationError::ApprovalNotRequired)
    ));

    let high = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    assert!(PolicyEngine::approval_request(&high, Some(&grant(&high, RiskLevel::High))).is_err());

    let overflowing = PolicyContext::new(
        "owner-1",
        "actor-1",
        "writer-agent",
        1,
        "workspace-1",
        CapabilityReferenceId::new(Uuid::from_u128(10)),
        &manifest(RiskLevel::High, 1),
        &invocation(json!({ "path": "reports/a.md" })),
        1,
        Default::default(),
        i64::MAX,
    )
    .unwrap();
    assert!(matches!(
        PolicyEngine::approval_request(&overflowing, None),
        Err(PolicyValidationError::ApprovalWindowOverflow)
    ));
}

#[test]
fn policy_identifiers_follow_task2_bounds_without_secret_word_sniffing() {
    let capability_id = "c".repeat(256);
    let logical_invocation = LogicalInvocation::new(
        Uuid::from_u128(1),
        "token.rotate",
        capability_id.clone(),
        1,
        json!({ "path": "reports/a.md" }),
    )
    .unwrap();
    let mut capability = manifest(RiskLevel::High, 1);
    capability.id = capability_id;
    let valid_context = PolicyContext::new(
        "owner-1",
        "actor-1",
        "credentials.audit",
        1,
        "token.rotate",
        CapabilityReferenceId::new(Uuid::from_u128(10)),
        &capability,
        &logical_invocation,
        1,
        Default::default(),
        1_000,
    )
    .unwrap();
    assert_eq!(valid_context.capability_id.len(), 256);

    let original = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    let mut serialized = serde_json::to_value(original).unwrap();
    serialized["owner_id"] = json!("x".repeat(129));
    assert!(serde_json::from_value::<PolicyContext>(serialized).is_err());

    let safe_context = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    let debug = format!("{safe_context:?}");
    assert!(!debug.contains(&Uuid::from_u128(10).to_string()));

    let mut injected = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    injected.owner_id = "sk-live-injected-token".into();
    assert!(!format!("{injected:?}").contains("sk-live-injected-token"));
}

#[test]
fn nil_resource_references_are_rejected_on_construction_validation_and_serde() {
    let original_invocation = invocation(json!({ "path": "reports/a.md" }));
    assert!(PolicyContext::new(
        "owner-1",
        "actor-1",
        "writer-agent",
        1,
        "workspace-1",
        CapabilityReferenceId::new(Uuid::nil()),
        &manifest(RiskLevel::High, 1),
        &original_invocation,
        1,
        Default::default(),
        1_000,
    )
    .is_err());

    let original = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    assert!(GrantScope::new(
        original.owner_id.clone(),
        original.actor_id.clone(),
        original.agent_definition_id.clone(),
        original.agent_definition_version,
        original.workspace_id.clone(),
        CapabilityReferenceId::new(Uuid::nil()),
        original.capability_id.clone(),
        original.manifest_version,
        Some(original.canonical_argument_digest),
    )
    .is_err());

    let mut request = PolicyEngine::approval_request(&original, None).unwrap();
    request.resource_boundary = CapabilityReferenceId::new(Uuid::nil());
    assert!(ApprovalDecision::new_approved(request, 1_000).is_err());

    let mut context_json = serde_json::to_value(&original).unwrap();
    context_json["resource_boundary"] = json!(Uuid::nil());
    assert!(serde_json::from_value::<PolicyContext>(context_json).is_err());

    let mut scope_json = serde_json::to_value(scope(&original)).unwrap();
    scope_json["resource_boundary"] = json!(Uuid::nil());
    assert!(serde_json::from_value::<GrantScope>(scope_json).is_err());

    let approval = ApprovalDecision::new_approved(
        PolicyEngine::approval_request(&original, None).unwrap(),
        1_000,
    )
    .unwrap();
    let mut approval_json = serde_json::to_value(approval).unwrap();
    approval_json["request"]["resource_boundary"] = json!(Uuid::nil());
    assert!(serde_json::from_value::<ApprovalDecision>(approval_json).is_err());
}

#[test]
fn matching_grants_cannot_be_reused_as_approval_bindings() {
    let context = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    let grant = grant(&context, RiskLevel::High);
    assert!(PolicyEngine::approval_request(&context, Some(&grant)).is_err());
}

#[test]
fn approval_is_exact_and_changed_arguments_invalidate_it() {
    let original = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    let approval = ApprovalDecision::new_approved(
        PolicyEngine::approval_request(&original, None).unwrap(),
        1_000,
    )
    .unwrap();
    assert_eq!(
        PolicyEngine::validate_approval(&approval, &original),
        ApprovalValidity::Valid
    );

    let proposed = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/b.md" })),
        Default::default(),
    );
    assert_eq!(
        PolicyEngine::validate_approval(&approval, &proposed),
        ApprovalValidity::InvalidArguments
    );
}

#[test]
fn policy_revision_reuses_exact_approval_only_when_current_policy_still_requires_approval() {
    let original = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    let approval = ApprovalDecision::new_approved(
        PolicyEngine::approval_request(&original, None).unwrap(),
        1_000,
    )
    .unwrap();
    let mut revised = original.clone();
    revised.policy_revision = 2;
    assert!(matches!(
        PolicyEngine::evaluate_with_approval(&revised, &[], Some(&approval))
            .unwrap()
            .decision,
        PolicyDecision::Allow(_)
    ));
    revised.restrictions.deny = true;
    assert!(matches!(
        PolicyEngine::evaluate_with_approval(&revised, &[], Some(&approval))
            .unwrap()
            .decision,
        PolicyDecision::Deny(_)
    ));
}

#[test]
fn grant_consumption_is_a_deterministic_non_mutating_proposal() {
    let context = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    let broad = grant(&context, RiskLevel::Critical);
    let mut narrow = grant(&context, RiskLevel::High);
    narrow.id = "a-narrow-grant".into();
    let first = PolicyEngine::evaluate(&context, &[broad.clone(), narrow.clone()]).unwrap();
    let second = PolicyEngine::evaluate(&context, &[narrow.clone(), broad.clone()]).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.consumption.unwrap().grant_id, "a-narrow-grant");
    assert_eq!(narrow.remaining_uses, Some(1));
}

#[test]
fn policy_records_round_trip_and_do_not_serialize_arguments_or_credentials() {
    let context = context(
        RiskLevel::High,
        invocation(json!({ "password": "super-secret" })),
        Default::default(),
    );
    let grant = grant(&context, RiskLevel::High);
    let encoded = serde_json::to_string(&(context.clone(), grant)).unwrap();
    assert!(!encoded.contains("super-secret"));
    assert!(!format!("{context:?}").contains("super-secret"));
    let restored: (PolicyContext, AutonomyGrant) = serde_json::from_str(&encoded).unwrap();
    assert_eq!(
        serde_json::to_string(&restored.0).unwrap(),
        serde_json::to_string(&context).unwrap()
    );
    assert!(PolicyEngine::evaluate(&restored.0, &[]).is_err());
}

#[test]
fn malformed_policy_records_are_rejected_during_construction_and_deserialization() {
    let context = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    assert!(AutonomyGrant::new(
        " ",
        0,
        GrantStatus::Active,
        scope(&context),
        RiskLevel::None,
        2_000,
        Some(1_000),
        Some(0)
    )
    .is_err());
    let malformed = json!({
        "id": "grant",
        "revision": 0,
        "status": "Active",
        "scope": scope(&context),
        "maximum_risk": "Low",
        "valid_from_ms": 2_000,
        "valid_until_ms": 1_000,
        "remaining_uses": 0
    });
    assert!(serde_json::from_value::<AutonomyGrant>(malformed).is_err());
}

#[test]
fn deserialized_policy_context_requires_exact_manifest_and_invocation_provenance() {
    let invocation = invocation(json!({ "path": "reports/a.md" }));
    let manifest = manifest(RiskLevel::High, 1);
    let original = PolicyContext::new(
        "owner-1",
        "actor-1",
        "writer-agent",
        1,
        "workspace-1",
        CapabilityReferenceId::new(Uuid::from_u128(10)),
        &manifest,
        &invocation,
        1,
        Default::default(),
        1_000,
    )
    .unwrap();
    let mut restored: PolicyContext =
        serde_json::from_value(serde_json::to_value(&original).unwrap()).unwrap();
    assert!(PolicyEngine::evaluate(&restored, &[]).is_err());
    restored.verify_against(&manifest, &invocation).unwrap();
    assert!(matches!(
        PolicyEngine::evaluate(&restored, &[]).unwrap().decision,
        PolicyDecision::RequireApproval(_)
    ));

    let mut lower_risk = serde_json::to_value(&original).unwrap();
    lower_risk["manifest_risk"] = json!("Low");
    let mut lower_risk: PolicyContext = serde_json::from_value(lower_risk).unwrap();
    assert!(lower_risk.verify_against(&manifest, &invocation).is_err());

    let mut substituted_digest = serde_json::to_value(&original).unwrap();
    substituted_digest["canonical_argument_digest"] = json!(Uuid::from_u128(99));
    let mut substituted_digest: PolicyContext = serde_json::from_value(substituted_digest).unwrap();
    assert!(substituted_digest
        .verify_against(&manifest, &invocation)
        .is_err());
}

#[test]
fn supplied_nonmatching_grant_cannot_be_silently_downgraded_to_unbound_approval() {
    let context = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    let mut revoked = grant(&context, RiskLevel::High);
    revoked.status = GrantStatus::Revoked;
    assert!(PolicyEngine::approval_request(&context, Some(&revoked)).is_err());
}

#[test]
fn same_id_grant_revisions_are_ranked_deterministically_after_id() {
    let context = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    let mut revision_one = grant(&context, RiskLevel::High);
    revision_one.id = "same-id".into();
    let mut revision_two = revision_one.clone();
    revision_two.revision = 2;
    let first =
        PolicyEngine::evaluate(&context, &[revision_one.clone(), revision_two.clone()]).unwrap();
    let second = PolicyEngine::evaluate(&context, &[revision_two, revision_one]).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.consumption.unwrap().grant_revision, 2);
}

#[test]
fn audit_records_are_validated_on_construction_and_serde_round_trip() {
    let context = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    let evaluation = PolicyEngine::evaluate(&context, &[grant(&context, RiskLevel::High)]).unwrap();
    let encoded = serde_json::to_string(&evaluation).unwrap();
    assert_eq!(
        serde_json::from_str::<PolicyEvaluation>(&encoded).unwrap(),
        evaluation
    );

    let mut invalid_consumption = serde_json::to_value(&evaluation).unwrap();
    invalid_consumption["consumption"]["logical_invocation_id"] = json!(Uuid::nil());
    assert!(serde_json::from_value::<PolicyEvaluation>(invalid_consumption).is_err());
    assert!(serde_json::from_value::<GrantConsumption>(json!({
        "grant_id": " ", "grant_revision": 0, "logical_invocation_id": Uuid::nil()
    }))
    .is_err());
    assert!(serde_json::from_value::<PolicyReason>(json!({
        "code": "allowed_by_grant", "effective_risk": "High", "policy_revision": 0,
        "grant_id": " ", "grant_revision": 0
    }))
    .is_err());
    assert!(serde_json::from_value::<PolicyDecision>(json!({
        "RequireApproval": {
            "code": "allowed_by_default", "effective_risk": "High", "policy_revision": 1,
            "grant_id": null, "grant_revision": null
        }
    }))
    .is_err());
}

#[test]
fn mutated_approval_and_unverified_context_fail_closed() {
    let context = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    let mut approval = ApprovalDecision::new_approved(
        PolicyEngine::approval_request(&context, None).unwrap(),
        1_000,
    )
    .unwrap();
    approval.request.expires_at_ms = 0;
    assert_ne!(
        PolicyEngine::validate_approval(&approval, &context),
        ApprovalValidity::Valid
    );
    assert!(matches!(
        PolicyEngine::evaluate_with_approval(&context, &[], Some(&approval))
            .unwrap()
            .decision,
        PolicyDecision::RequireApproval(_)
    ));

    let mut extended = ApprovalDecision::new_approved(
        PolicyEngine::approval_request(&context, None).unwrap(),
        1_000,
    )
    .unwrap();
    extended.request.expires_at_ms += 1;
    assert_ne!(
        PolicyEngine::validate_approval(&extended, &context),
        ApprovalValidity::Valid
    );

    let mut approval = ApprovalDecision::new_approved(
        PolicyEngine::approval_request(&context, None).unwrap(),
        1_000,
    )
    .unwrap();
    approval.request.reason.policy_revision = 2;
    assert_ne!(
        PolicyEngine::validate_approval(&approval, &context),
        ApprovalValidity::Valid
    );
    let unverified: PolicyContext =
        serde_json::from_value(serde_json::to_value(&context).unwrap()).unwrap();
    assert_ne!(
        PolicyEngine::validate_approval(&approval, &unverified),
        ApprovalValidity::Valid
    );

    let mut malformed = serde_json::to_value(&approval).unwrap();
    malformed["request"]["run_id"] = json!(Uuid::nil());
    assert!(serde_json::from_value::<ApprovalDecision>(malformed).is_err());
}

#[test]
fn approval_decision_window_is_exact_and_future_decisions_do_not_authorize() {
    let context = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    let request = PolicyEngine::approval_request(&context, None).unwrap();

    assert!(ApprovalDecision::new_approved(request.clone(), 999).is_err());
    assert!(ApprovalDecision::new_approved(request.clone(), request.expires_at_ms).is_err());

    let future = ApprovalDecision::new_approved(request.clone(), 1_001).unwrap();
    assert_eq!(
        PolicyEngine::validate_approval(&future, &context),
        ApprovalValidity::InvalidBinding
    );

    let exact = ApprovalDecision::new_approved(request.clone(), 1_000).unwrap();
    assert_eq!(
        PolicyEngine::validate_approval(&exact, &context),
        ApprovalValidity::Valid
    );

    let mut mutated = exact.clone();
    mutated.decided_at_ms = 999;
    assert_eq!(
        PolicyEngine::validate_approval(&mutated, &context),
        ApprovalValidity::InvalidBinding
    );

    let mut serialized = serde_json::to_value(&exact).unwrap();
    serialized["decided_at_ms"] = json!(request.expires_at_ms);
    assert!(serde_json::from_value::<ApprovalDecision>(serialized).is_err());

    let mut expiry = context.clone();
    expiry.now_ms = request.expires_at_ms;
    assert_eq!(
        PolicyEngine::validate_approval(&exact, &expiry),
        ApprovalValidity::Expired
    );
}

#[test]
fn grant_scope_rejects_invalid_direct_and_serialized_bindings() {
    let context = context(
        RiskLevel::High,
        invocation(json!({ "path": "reports/a.md" })),
        Default::default(),
    );
    assert!(GrantScope::new(
        " ",
        context.actor_id.clone(),
        context.agent_definition_id.clone(),
        0,
        context.workspace_id.clone(),
        context.resource_boundary.clone(),
        context.capability_id.clone(),
        context.manifest_version,
        Some(Uuid::nil()),
    )
    .is_err());
    let mut malformed = serde_json::to_value(scope(&context)).unwrap();
    malformed["canonical_argument_digest"] = json!(Uuid::nil());
    assert!(serde_json::from_value::<GrantScope>(malformed).is_err());
}
