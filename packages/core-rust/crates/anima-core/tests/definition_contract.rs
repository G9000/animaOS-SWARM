use anima_core::{
    AgentDefinitionDraft, CapabilityKind, CapabilityManifest, CapabilityOverride,
    CapabilityProfile, CapabilityProfileEntry, DefinitionPublisher, DefinitionValidationError,
    HostRequirement, LifecyclePolicy, ManifestCatalog, MemoryPolicy, ModelPolicy, ProfileRef,
    RecoveryMode, RiskLevel, RuntimeCompatibility, RuntimeLimits,
};
use serde_json::json;

fn manifest(id: &str, version: u32, kind: CapabilityKind) -> CapabilityManifest {
    CapabilityManifest {
        id: id.into(),
        version,
        kind,
        label: format!("{id} capability"),
        description: "A portable capability contract.".into(),
        input_schema: json!({ "type": "object" }),
        output_schema: json!({ "type": "object" }),
        side_effects: false,
        risk_level: RiskLevel::Low,
        host_permissions: vec![],
        secret_references: vec![],
        environment_requirements: vec![],
        timeout_ms: 5_000,
        cancellation_supported: true,
        max_retries: 2,
        idempotent: true,
        recovery_mode: RecoveryMode::Retry,
        supports_streaming: false,
        supports_artifacts: true,
        supports_citations: true,
        schema_digest: format!("sha256:{id}:{version}"),
        compatibility: RuntimeCompatibility {
            minimum_runtime_schema_version: 1,
            maximum_runtime_schema_version: 1,
            manifest_schema_version: 1,
        },
    }
}

fn draft(profile_version: u32) -> AgentDefinitionDraft {
    AgentDefinitionDraft {
        schema_version: 1,
        id: "research-agent".into(),
        version: 1,
        name: "Research Agent".into(),
        display_name: "Research".into(),
        description: "Finds grounded answers.".into(),
        persona: "Careful and concise.".into(),
        system: "Cite the available knowledge.".into(),
        model: ModelPolicy {
            provider: "openai".into(),
            model: "gpt-5".into(),
            credential_reference: Some("openai-production".into()),
            temperature: Some(0.2),
        },
        source_profile: ProfileRef {
            profile_id: "knowledge-workspace".into(),
            profile_version,
        },
        capability_overrides: vec![],
        memory: MemoryPolicy {
            enabled: true,
            namespace: "research".into(),
            retention_days: Some(30),
        },
        approval_policy_id: "standard-approval".into(),
        approval_policy_revision: 3,
        approval_restrictions: vec!["external-write".into()],
        limits: RuntimeLimits {
            max_turns: 8,
            timeout_ms: 30_000,
            max_concurrent_tasks: 2,
        },
        lifecycle: LifecyclePolicy {
            auto_start: false,
            restart_on_failure: true,
            max_restarts: 3,
        },
        host_requirements: vec![HostRequirement {
            id: "workspace-host".into(),
            revision: 1,
        }],
    }
}

fn catalog() -> ManifestCatalog {
    let mut catalog = ManifestCatalog::default();
    catalog
        .register_manifest(manifest("workspace.files", 1, CapabilityKind::Workspace))
        .unwrap();
    catalog
        .register_manifest(manifest("knowledge.search", 1, CapabilityKind::Knowledge))
        .unwrap();
    catalog
        .register_profile(CapabilityProfile {
            id: "knowledge-workspace".into(),
            version: 1,
            label: "Knowledge and Workspace".into(),
            description: "Grounded research with workspace access.".into(),
            entries: vec![
                CapabilityProfileEntry {
                    capability_id: "workspace.files".into(),
                    manifest_version: 1,
                },
                CapabilityProfileEntry {
                    capability_id: "knowledge.search".into(),
                    manifest_version: 1,
                },
            ],
        })
        .unwrap();
    catalog
}

fn publisher() -> DefinitionPublisher {
    DefinitionPublisher::new(vec![HostRequirement {
        id: "workspace-host".into(),
        revision: 1,
    }])
}

#[test]
fn publisher_creates_a_pinned_knowledge_workspace_definition_in_deterministic_order() {
    let catalog = catalog();
    let definition = publisher().publish(&catalog, draft(1)).unwrap();

    assert_eq!(definition.source_profile.profile_version, 1);
    assert_eq!(
        definition
            .resolved_capabilities
            .iter()
            .map(|capability| capability.capability_id.as_str())
            .collect::<Vec<_>>(),
        ["knowledge.search", "workspace.files"]
    );
    assert_eq!(definition.resolved_capabilities[0].manifest_version, 1);
    assert_eq!(
        definition.resolved_capabilities[0].schema_digest,
        "sha256:knowledge.search:1"
    );
    assert_eq!(definition.resolved_capabilities[0].override_config, None);
    assert_eq!(
        definition.resolved_capabilities[0].approval_policy_revision,
        3
    );
}

#[test]
fn publisher_applies_profile_capability_overrides_and_pins_the_effective_manifest() {
    let mut catalog = catalog();
    catalog
        .register_manifest(manifest("knowledge.search", 2, CapabilityKind::Knowledge))
        .unwrap();
    let mut overridden = draft(1);
    overridden.capability_overrides = vec![CapabilityOverride {
        capability_id: "knowledge.search".into(),
        manifest_version: 2,
        configuration: json!({ "result_limit": 5 }),
    }];

    let definition = publisher().publish(&catalog, overridden).unwrap();
    let capability = &definition.resolved_capabilities[0];

    assert_eq!(capability.capability_id, "knowledge.search");
    assert_eq!(capability.manifest_version, 2);
    assert_eq!(capability.schema_digest, "sha256:knowledge.search:2");
    assert_eq!(
        capability.override_config.as_ref().unwrap().configuration,
        json!({ "result_limit": 5 })
    );
    assert_eq!(capability.approval_policy_revision, 3);
}

#[test]
fn publisher_rejects_unknown_definition_schema_versions() {
    let catalog = catalog();
    let mut invalid = draft(1);
    invalid.schema_version = 999;

    let error = publisher().publish(&catalog, invalid).unwrap_err();

    assert!(matches!(
        error,
        DefinitionValidationError::UnsupportedSchemaVersion { .. }
    ));
}

#[test]
fn publisher_rejects_duplicate_capability_ids_after_profile_and_override_resolution() {
    let mut catalog = catalog();
    catalog
        .register_manifest(manifest("knowledge.search", 2, CapabilityKind::Knowledge))
        .unwrap();
    let mut invalid = draft(1);
    invalid.capability_overrides = vec![
        CapabilityOverride {
            capability_id: "knowledge.search".into(),
            manifest_version: 1,
            configuration: json!({ "limit": 5 }),
        },
        CapabilityOverride {
            capability_id: "knowledge.search".into(),
            manifest_version: 2,
            configuration: json!({ "limit": 10 }),
        },
    ];

    let error = publisher().publish(&catalog, invalid).unwrap_err();

    assert!(matches!(
        error,
        DefinitionValidationError::DuplicateCapabilityId { .. }
    ));
}

#[test]
fn publisher_rejects_missing_host_requirements() {
    let catalog = catalog();
    let mut unavailable_host = DefinitionPublisher::default();

    let error = unavailable_host.publish(&catalog, draft(1)).unwrap_err();

    assert!(matches!(
        error,
        DefinitionValidationError::MissingHostRequirement { .. }
    ));
}

#[test]
fn published_definitions_do_not_drift_when_later_profiles_are_registered() {
    let mut catalog = catalog();
    let mut publisher = publisher();
    let published = publisher.publish(&catalog, draft(1)).unwrap();
    catalog
        .register_profile(CapabilityProfile {
            id: "knowledge-workspace".into(),
            version: 2,
            label: "Knowledge only".into(),
            description: "A later profile revision.".into(),
            entries: vec![CapabilityProfileEntry {
                capability_id: "knowledge.search".into(),
                manifest_version: 1,
            }],
        })
        .unwrap();

    let stored = publisher.definition("research-agent", 1).unwrap();
    assert_eq!(stored, &published);
    assert_eq!(stored.source_profile.profile_version, 1);
    assert_eq!(stored.resolved_capabilities.len(), 2);
}

#[test]
fn publisher_rejects_non_increasing_definition_versions() {
    let catalog = catalog();
    let mut publisher = publisher();
    publisher.publish(&catalog, draft(1)).unwrap();

    let error = publisher.publish(&catalog, draft(1)).unwrap_err();

    assert!(matches!(
        error,
        DefinitionValidationError::NonIncreasingVersion { .. }
    ));
}
