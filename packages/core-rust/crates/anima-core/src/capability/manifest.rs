use std::collections::BTreeSet;

use super::{
    CapabilityManifest, ManifestCatalogError, MAX_CAPABILITY_ID_BYTES,
    MAX_CAPABILITY_SECRET_REFERENCES,
};

pub(super) fn validate_manifest(manifest: &CapabilityManifest) -> Result<(), ManifestCatalogError> {
    if manifest.id.trim().is_empty() || manifest.id.len() > MAX_CAPABILITY_ID_BYTES {
        return Err(ManifestCatalogError::InvalidManifestId);
    }
    if manifest.version == 0 {
        return Err(ManifestCatalogError::InvalidManifestVersion);
    }
    if manifest.compatibility.manifest_schema_version == 0 {
        return Err(ManifestCatalogError::InvalidManifestSchemaVersion);
    }
    if manifest.schema_digest.trim().is_empty() {
        return Err(ManifestCatalogError::InvalidSchemaDigest);
    }
    if manifest.compatibility.minimum_runtime_schema_version == 0
        || manifest.compatibility.maximum_runtime_schema_version == 0
        || manifest.compatibility.minimum_runtime_schema_version
            > manifest.compatibility.maximum_runtime_schema_version
    {
        return Err(ManifestCatalogError::InvalidRuntimeCompatibility);
    }
    if manifest.secret_references.len() > MAX_CAPABILITY_SECRET_REFERENCES {
        return Err(ManifestCatalogError::TooManySecretReferences);
    }
    let mut secret_names = BTreeSet::new();
    for name in &manifest.secret_references {
        if !is_valid_secret_reference_name(name) || !secret_names.insert(name) {
            return Err(ManifestCatalogError::InvalidSecretReferenceName);
        }
    }
    Ok(())
}

fn is_valid_secret_reference_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}
