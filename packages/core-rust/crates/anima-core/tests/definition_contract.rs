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
            allows_concurrent_sessions: false,
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
fn drafts_and_published_definitions_round_trip_through_serde_with_pins_intact() {
    let mut catalog = catalog();
    catalog
        .register_manifest(manifest("knowledge.search", 2, CapabilityKind::Knowledge))
        .unwrap();
    let mut draft = draft(1);
    draft.capability_overrides = vec![CapabilityOverride {
        capability_id: "knowledge.search".into(),
        manifest_version: 2,
        configuration: json!({ "result_limit": 5 }),
    }];

    let restored_draft =
        serde_json::from_str::<AgentDefinitionDraft>(&serde_json::to_string(&draft).unwrap())
            .unwrap();
    assert_eq!(restored_draft, draft);
    let definition = publisher().publish(&catalog, restored_draft).unwrap();
    let restored_definition = serde_json::from_str::<anima_core::AgentDefinition>(
        &serde_json::to_string(&definition).unwrap(),
    )
    .unwrap();

    assert_eq!(restored_definition, definition);
    assert_eq!(restored_definition.source_profile.profile_version, 1);
    assert_eq!(
        restored_definition.resolved_capabilities[0].schema_digest,
        "sha256:knowledge.search:2"
    );
    assert_eq!(
        restored_definition.resolved_capabilities[0]
            .override_config
            .as_ref()
            .unwrap()
            .manifest_version,
        2
    );
    assert_eq!(
        restored_definition.resolved_capabilities[0].approval_policy_revision,
        3
    );
}

#[test]
fn legacy_lifecycle_payloads_default_to_serial_sessions() {
    let mut draft_json = serde_json::to_value(draft(1)).unwrap();
    draft_json["lifecycle"]
        .as_object_mut()
        .unwrap()
        .remove("allows_concurrent_sessions");
    let restored_draft: AgentDefinitionDraft = serde_json::from_value(draft_json).unwrap();
    assert!(!restored_draft.lifecycle.allows_concurrent_sessions);

    let definition = publisher().publish(&catalog(), restored_draft).unwrap();
    let mut definition_json = serde_json::to_value(definition).unwrap();
    definition_json["lifecycle"]
        .as_object_mut()
        .unwrap()
        .remove("allows_concurrent_sessions");
    let restored_definition: anima_core::AgentDefinition =
        serde_json::from_value(definition_json).unwrap();
    assert!(!restored_definition.lifecycle.allows_concurrent_sessions);
}

#[test]
fn definition_validation_errors_round_trip_through_serde() {
    let error = DefinitionValidationError::MissingHostRequirement {
        id: "workspace-host".into(),
        revision: 1,
    };

    assert_eq!(
        serde_json::from_str::<DefinitionValidationError>(&serde_json::to_string(&error).unwrap())
            .unwrap(),
        error
    );
}

#[test]
fn publisher_rejects_non_finite_temperatures_without_mutating_state_or_json_corruption() {
    for temperature in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let catalog = catalog();
        let mut publisher = publisher();
        let mut invalid = draft(1);
        invalid.model.temperature = Some(temperature);

        assert!(matches!(
            publisher.publish(&catalog, invalid.clone()),
            Err(DefinitionValidationError::InvalidTemperature)
        ));
        assert!(publisher.definition("research-agent", 1).is_none());
        assert!(serde_json::to_string(&invalid).is_err());
    }
}

#[test]
fn publisher_canonicalizes_host_requirement_order_and_rejects_duplicates_transactionally() {
    let catalog = catalog();
    let primary = HostRequirement {
        id: "workspace-host".into(),
        revision: 1,
    };
    let secondary = HostRequirement {
        id: "knowledge-host".into(),
        revision: 2,
    };
    let mut first = draft(1);
    first.host_requirements = vec![primary.clone(), secondary.clone()];
    let mut second = draft(1);
    second.host_requirements = vec![secondary.clone(), primary.clone()];
    let first_definition = DefinitionPublisher::new(vec![primary.clone(), secondary.clone()])
        .publish(&catalog, first)
        .unwrap();
    let second_definition = DefinitionPublisher::new(vec![primary.clone(), secondary.clone()])
        .publish(&catalog, second)
        .unwrap();

    assert_eq!(
        serde_json::to_string(&first_definition).unwrap(),
        serde_json::to_string(&second_definition).unwrap()
    );
    assert_eq!(
        first_definition.host_requirements,
        vec![secondary.clone(), primary.clone()]
    );

    let mut publisher = DefinitionPublisher::new(vec![primary.clone()]);
    let mut duplicate = draft(1);
    duplicate.host_requirements = vec![primary.clone(), primary];
    assert!(matches!(
        publisher.publish(&catalog, duplicate),
        Err(DefinitionValidationError::DuplicateHostRequirement { .. })
    ));
    assert!(publisher.definition("research-agent", 1).is_none());
}

#[test]
fn definition_deserialization_rejects_tampered_invariants() {
    let catalog = catalog();
    let definition = publisher().publish(&catalog, draft(1)).unwrap();
    let json = serde_json::to_value(definition).unwrap();

    let mut unsupported_schema = json.clone();
    unsupported_schema["schema_version"] = json!(999);
    assert!(serde_json::from_value::<anima_core::AgentDefinition>(unsupported_schema).is_err());

    let mut duplicate_capability = json.clone();
    let first_capability = duplicate_capability["resolved_capabilities"][0].clone();
    duplicate_capability["resolved_capabilities"]
        .as_array_mut()
        .unwrap()
        .push(first_capability);
    assert!(serde_json::from_value::<anima_core::AgentDefinition>(duplicate_capability).is_err());

    let mut reversed_capabilities = json.clone();
    reversed_capabilities["resolved_capabilities"]
        .as_array_mut()
        .unwrap()
        .reverse();
    assert!(serde_json::from_value::<anima_core::AgentDefinition>(reversed_capabilities).is_err());

    let mut blank_digest = json.clone();
    blank_digest["resolved_capabilities"][0]["schema_digest"] = json!("");
    assert!(serde_json::from_value::<anima_core::AgentDefinition>(blank_digest).is_err());

    let mut blank_capability_pin = json.clone();
    blank_capability_pin["resolved_capabilities"][0]["capability_id"] = json!("");
    assert!(serde_json::from_value::<anima_core::AgentDefinition>(blank_capability_pin).is_err());

    let mut zero_manifest_pin = json.clone();
    zero_manifest_pin["resolved_capabilities"][0]["manifest_version"] = json!(0);
    assert!(serde_json::from_value::<anima_core::AgentDefinition>(zero_manifest_pin).is_err());

    let mut inconsistent_override = json.clone();
    inconsistent_override["resolved_capabilities"][0]["override_config"] = json!({
        "capability_id": "different.capability",
        "manifest_version": 2,
        "configuration": {}
    });
    assert!(serde_json::from_value::<anima_core::AgentDefinition>(inconsistent_override).is_err());

    let mut inconsistent_policy_revision = json.clone();
    inconsistent_policy_revision["resolved_capabilities"][0]["approval_policy_revision"] = json!(0);
    assert!(
        serde_json::from_value::<anima_core::AgentDefinition>(inconsistent_policy_revision)
            .is_err()
    );

    let mut invalid_host = json.clone();
    invalid_host["host_requirements"] = json!([{ "id": "", "revision": 0 }]);
    assert!(serde_json::from_value::<anima_core::AgentDefinition>(invalid_host).is_err());

    let mut duplicate_host = json.clone();
    let host = duplicate_host["host_requirements"][0].clone();
    duplicate_host["host_requirements"] = json!([host.clone(), host]);
    assert!(serde_json::from_value::<anima_core::AgentDefinition>(duplicate_host).is_err());

    let non_finite_json = serde_json::to_string(&json)
        .unwrap()
        .replace("\"temperature\":0.2", "\"temperature\":1e999");
    assert!(serde_json::from_str::<anima_core::AgentDefinition>(&non_finite_json).is_err());
}

#[test]
fn overrides_absent_from_the_source_profile_intentionally_add_a_capability() {
    let mut catalog = catalog();
    catalog
        .register_manifest(manifest(
            "communication.notify",
            1,
            CapabilityKind::Communication,
        ))
        .unwrap();
    let mut with_addition = draft(1);
    with_addition.capability_overrides = vec![CapabilityOverride {
        capability_id: "communication.notify".into(),
        manifest_version: 1,
        configuration: json!({ "channel": "updates" }),
    }];

    let definition = publisher().publish(&catalog, with_addition).unwrap();

    assert_eq!(definition.resolved_capabilities.len(), 3);
    assert_eq!(
        definition.resolved_capabilities[0].capability_id,
        "communication.notify"
    );
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
    catalog
        .register_manifest(manifest("knowledge.search", 2, CapabilityKind::Knowledge))
        .unwrap();

    let stored = publisher.definition("research-agent", 1).unwrap();
    assert_eq!(stored, &published);
    assert_eq!(stored.source_profile.profile_version, 1);
    assert_eq!(stored.resolved_capabilities.len(), 2);
    assert_eq!(stored.resolved_capabilities[0].manifest_version, 1);
    assert_eq!(
        stored.resolved_capabilities[0].schema_digest,
        "sha256:knowledge.search:1"
    );
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
