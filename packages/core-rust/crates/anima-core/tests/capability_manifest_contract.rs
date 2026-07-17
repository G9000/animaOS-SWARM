use anima_core::{
    CapabilityKind, CapabilityManifest, CapabilityProfile, CapabilityProfileEntry, ManifestCatalog,
    ManifestCatalogError, RecoveryMode, RiskLevel, RuntimeCompatibility,
};
use serde_json::json;

fn manifest(id: &str, version: u32) -> CapabilityManifest {
    CapabilityManifest {
        id: id.into(),
        version,
        kind: CapabilityKind::Knowledge,
        label: format!("{id} capability"),
        description: "A portable capability contract.".into(),
        input_schema: json!({ "type": "object" }),
        output_schema: json!({ "type": "object" }),
        side_effects: false,
        risk_level: RiskLevel::Low,
        host_permissions: vec!["filesystem.read".into()],
        secret_references: vec!["search-api-key".into()],
        environment_requirements: vec!["SEARCH_ENDPOINT".into()],
        timeout_ms: 5_000,
        cancellation_supported: true,
        max_retries: 2,
        idempotent: true,
        recovery_mode: RecoveryMode::Retry,
        supports_streaming: true,
        supports_artifacts: false,
        supports_citations: true,
        schema_digest: format!("sha256:{id}:{version}"),
        compatibility: RuntimeCompatibility {
            minimum_runtime_schema_version: 1,
            maximum_runtime_schema_version: 1,
            manifest_schema_version: 1,
        },
    }
}

#[test]
fn manifests_and_profiles_round_trip_through_the_public_serde_contract() {
    let manifest = manifest("knowledge.search", 1);
    let profile = CapabilityProfile {
        id: "research".into(),
        version: 1,
        label: "Research".into(),
        description: "Knowledge lookup capabilities.".into(),
        entries: vec![CapabilityProfileEntry {
            capability_id: manifest.id.clone(),
            manifest_version: manifest.version,
        }],
    };

    assert_eq!(
        serde_json::from_str::<CapabilityManifest>(&serde_json::to_string(&manifest).unwrap())
            .unwrap(),
        manifest
    );
    assert_eq!(
        serde_json::from_str::<CapabilityProfile>(&serde_json::to_string(&profile).unwrap())
            .unwrap(),
        profile
    );
}

#[test]
fn catalog_round_trips_through_a_stable_json_snapshot() {
    let mut catalog = ManifestCatalog::default();
    catalog
        .register_manifest(manifest("workspace.write", 1))
        .unwrap();
    catalog
        .register_manifest(manifest("knowledge.search", 1))
        .unwrap();
    catalog
        .register_profile(CapabilityProfile {
            id: "research".into(),
            version: 1,
            label: "Research".into(),
            description: "Knowledge and workspace capabilities.".into(),
            entries: vec![
                CapabilityProfileEntry {
                    capability_id: "workspace.write".into(),
                    manifest_version: 1,
                },
                CapabilityProfileEntry {
                    capability_id: "knowledge.search".into(),
                    manifest_version: 1,
                },
            ],
        })
        .unwrap();

    let json = serde_json::to_string(&catalog).unwrap();
    let restored = serde_json::from_str::<ManifestCatalog>(&json).unwrap();

    assert_eq!(serde_json::to_string(&restored).unwrap(), json);
    assert_eq!(
        restored
            .profile("research", 1)
            .unwrap()
            .entries
            .iter()
            .map(|entry| entry.capability_id.as_str())
            .collect::<Vec<_>>(),
        ["knowledge.search", "workspace.write"]
    );
}

#[test]
fn catalog_deserialization_revalidates_duplicates_and_profile_manifest_references() {
    let missing_manifest_snapshot = json!({
        "manifests": [],
        "profiles": [{
            "id": "research",
            "version": 1,
            "label": "Research",
            "description": "Knowledge lookup capabilities.",
            "entries": [{
                "capability_id": "knowledge.search",
                "manifest_version": 1
            }]
        }]
    });
    let duplicate_manifest_snapshot = json!({
        "manifests": [
            manifest("knowledge.search", 1),
            manifest("knowledge.search", 1)
        ],
        "profiles": []
    });

    assert!(serde_json::from_value::<ManifestCatalog>(missing_manifest_snapshot).is_err());
    assert!(serde_json::from_value::<ManifestCatalog>(duplicate_manifest_snapshot).is_err());
}

#[test]
fn catalog_rejects_invalid_manifests_transactionally() {
    let mut catalog = ManifestCatalog::default();
    catalog
        .register_manifest(manifest("knowledge.search", 1))
        .unwrap();
    let snapshot = serde_json::to_string(&catalog).unwrap();

    let mut blank_id = manifest("knowledge.write", 1);
    blank_id.id = " ".into();
    assert!(matches!(
        catalog.register_manifest(blank_id),
        Err(ManifestCatalogError::InvalidManifestId)
    ));

    let mut zero_version = manifest("knowledge.write", 1);
    zero_version.version = 0;
    assert!(matches!(
        catalog.register_manifest(zero_version),
        Err(ManifestCatalogError::InvalidManifestVersion)
    ));

    let mut invalid_compatibility = manifest("knowledge.write", 1);
    invalid_compatibility
        .compatibility
        .minimum_runtime_schema_version = 2;
    invalid_compatibility
        .compatibility
        .maximum_runtime_schema_version = 1;
    assert!(matches!(
        catalog.register_manifest(invalid_compatibility),
        Err(ManifestCatalogError::InvalidRuntimeCompatibility)
    ));
    assert_eq!(serde_json::to_string(&catalog).unwrap(), snapshot);
}

#[test]
fn catalog_deserialization_rejects_malformed_manifests() {
    let mut malformed = serde_json::to_value(manifest("knowledge.search", 1)).unwrap();
    malformed["schema_digest"] = json!("");
    malformed["compatibility"]["manifest_schema_version"] = json!(0);

    assert!(serde_json::from_value::<ManifestCatalog>(json!({
        "manifests": [malformed],
        "profiles": []
    }))
    .is_err());
}

#[test]
fn profiles_reject_duplicate_capability_ids_transactionally_and_on_deserialization() {
    let mut catalog = ManifestCatalog::default();
    catalog
        .register_manifest(manifest("knowledge.search", 1))
        .unwrap();
    catalog
        .register_manifest(manifest("knowledge.search", 2))
        .unwrap();
    let duplicate_profile = CapabilityProfile {
        id: "research".into(),
        version: 1,
        label: "Research".into(),
        description: "Duplicate capability IDs.".into(),
        entries: vec![
            CapabilityProfileEntry {
                capability_id: "knowledge.search".into(),
                manifest_version: 1,
            },
            CapabilityProfileEntry {
                capability_id: "knowledge.search".into(),
                manifest_version: 2,
            },
        ],
    };
    let snapshot = serde_json::to_string(&catalog).unwrap();

    assert!(matches!(
        catalog.register_profile(duplicate_profile.clone()),
        Err(ManifestCatalogError::DuplicateProfileCapabilityId { .. })
    ));
    assert_eq!(serde_json::to_string(&catalog).unwrap(), snapshot);
    assert!(serde_json::from_value::<ManifestCatalog>(json!({
        "manifests": [manifest("knowledge.search", 1), manifest("knowledge.search", 2)],
        "profiles": [duplicate_profile]
    }))
    .is_err());
}

#[test]
fn profiles_require_nonblank_ids_and_nonzero_versions_transactionally_and_on_deserialization() {
    let mut catalog = ManifestCatalog::default();
    let snapshot = serde_json::to_string(&catalog).unwrap();
    let blank_id = CapabilityProfile {
        id: " ".into(),
        version: 1,
        label: "Research".into(),
        description: "Invalid profile ID.".into(),
        entries: vec![],
    };
    let zero_version = CapabilityProfile {
        id: "research".into(),
        version: 0,
        label: "Research".into(),
        description: "Invalid profile version.".into(),
        entries: vec![],
    };

    assert!(matches!(
        catalog.register_profile(blank_id.clone()),
        Err(ManifestCatalogError::InvalidProfileId)
    ));
    assert!(matches!(
        catalog.register_profile(zero_version.clone()),
        Err(ManifestCatalogError::InvalidProfileVersion)
    ));
    assert_eq!(serde_json::to_string(&catalog).unwrap(), snapshot);
    assert!(serde_json::from_value::<ManifestCatalog>(json!({
        "manifests": [],
        "profiles": [blank_id]
    }))
    .is_err());
    assert!(serde_json::from_value::<ManifestCatalog>(json!({
        "manifests": [],
        "profiles": [zero_version]
    }))
    .is_err());
}

#[test]
fn catalog_rejects_duplicate_manifest_versions() {
    let mut catalog = ManifestCatalog::default();
    catalog
        .register_manifest(manifest("knowledge.search", 1))
        .unwrap();

    let error = catalog
        .register_manifest(manifest("knowledge.search", 1))
        .unwrap_err();

    assert!(matches!(
        error,
        ManifestCatalogError::DuplicateManifest { .. }
    ));
}

#[test]
fn catalog_rejects_duplicate_profile_versions() {
    let mut catalog = ManifestCatalog::default();
    catalog
        .register_manifest(manifest("knowledge.search", 1))
        .unwrap();
    let profile = CapabilityProfile {
        id: "research".into(),
        version: 1,
        label: "Research".into(),
        description: "Knowledge lookup capabilities.".into(),
        entries: vec![CapabilityProfileEntry {
            capability_id: "knowledge.search".into(),
            manifest_version: 1,
        }],
    };
    catalog.register_profile(profile.clone()).unwrap();

    let error = catalog.register_profile(profile).unwrap_err();

    assert!(matches!(
        error,
        ManifestCatalogError::DuplicateProfile { .. }
    ));
}

#[test]
fn catalog_uses_exact_versions_without_fallback() {
    let mut catalog = ManifestCatalog::default();
    catalog
        .register_manifest(manifest("knowledge.search", 2))
        .unwrap();

    assert!(catalog.manifest("knowledge.search", 1).is_none());
    assert!(catalog.manifest("knowledge.search", 2).is_some());
    assert!(catalog.profile("research", 1).is_none());
}

#[test]
fn profiles_must_reference_registered_exact_manifest_versions() {
    let mut catalog = ManifestCatalog::default();
    catalog
        .register_manifest(manifest("knowledge.search", 2))
        .unwrap();

    let error = catalog
        .register_profile(CapabilityProfile {
            id: "research".into(),
            version: 1,
            label: "Research".into(),
            description: "Knowledge lookup capabilities.".into(),
            entries: vec![CapabilityProfileEntry {
                capability_id: "knowledge.search".into(),
                manifest_version: 1,
            }],
        })
        .unwrap_err();

    assert!(matches!(
        error,
        ManifestCatalogError::UnknownManifest { .. }
    ));
}

#[test]
fn catalog_normalizes_profile_entries_into_a_deterministic_order() {
    let mut catalog = ManifestCatalog::default();
    catalog
        .register_manifest(manifest("workspace.write", 1))
        .unwrap();
    catalog
        .register_manifest(manifest("knowledge.search", 1))
        .unwrap();
    catalog
        .register_profile(CapabilityProfile {
            id: "research".into(),
            version: 1,
            label: "Research".into(),
            description: "Knowledge and workspace capabilities.".into(),
            entries: vec![
                CapabilityProfileEntry {
                    capability_id: "workspace.write".into(),
                    manifest_version: 1,
                },
                CapabilityProfileEntry {
                    capability_id: "knowledge.search".into(),
                    manifest_version: 1,
                },
            ],
        })
        .unwrap();

    let entries = &catalog.profile("research", 1).unwrap().entries;
    assert_eq!(entries[0].capability_id, "knowledge.search");
    assert_eq!(entries[1].capability_id, "workspace.write");
}
