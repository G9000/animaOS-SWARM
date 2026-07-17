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
