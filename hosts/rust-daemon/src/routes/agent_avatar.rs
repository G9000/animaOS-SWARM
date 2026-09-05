use std::io::{Read, Write};
use std::path::PathBuf;

use super::{
    workspace::{detect_avatar_media_type, WorkspaceAvatar, MAX_WORKSPACE_AVATAR_BYTES},
    ApiError,
};
use crate::app::SharedDaemonState;
use atomicwrites::{AllowOverwrite, AtomicFile};

async fn path(id: &str, state: &SharedDaemonState) -> Result<PathBuf, ApiError> {
    let guard = state.read().await;
    let runtime_id = guard.agent_runtime_id(id).ok_or_else(ApiError::not_found)?;
    let root = &guard
        .workspace
        .as_ref()
        .ok_or_else(|| ApiError::conflict("workspace is not configured"))?
        .root_path;
    // Encode the stored runtime ID so even restored IDs cannot introduce path components.
    let filename: String = runtime_id
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(root.join("assets").join("agent-avatars").join(filename))
}

pub(super) async fn put(
    id: &str,
    bytes: Vec<u8>,
    content_type: Option<&str>,
    state: &SharedDaemonState,
) -> Result<(), ApiError> {
    if bytes.is_empty() || bytes.len() > MAX_WORKSPACE_AVATAR_BYTES {
        return Err(ApiError::bad_request_static(
            "avatar must be between 1 byte and 5 MiB",
        ));
    }
    let detected = detect_avatar_media_type(&bytes)
        .ok_or_else(|| ApiError::bad_request_static("avatar must be PNG, JPEG, or WebP"))?;
    if content_type != Some(detected) {
        return Err(ApiError::bad_request_static(
            "avatar content type does not match its bytes",
        ));
    }
    let path = path(id, state).await?;
    std::fs::create_dir_all(path.parent().expect("avatar parent"))
        .map_err(|e| ApiError::service_unavailable(e.to_string()))?;
    AtomicFile::new(&path, AllowOverwrite)
        .write(|file| file.write_all(&bytes))
        .map_err(|e| ApiError::service_unavailable(e.to_string()))
}

pub(super) async fn get(id: &str, state: &SharedDaemonState) -> Result<WorkspaceAvatar, ApiError> {
    let file = std::fs::File::open(path(id, state).await?).map_err(|_| ApiError::not_found())?;
    let mut bytes = Vec::new();
    file.take((MAX_WORKSPACE_AVATAR_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| ApiError::not_found())?;
    if bytes.len() > MAX_WORKSPACE_AVATAR_BYTES {
        return Err(ApiError::not_found());
    }
    let content_type = detect_avatar_media_type(&bytes).ok_or_else(ApiError::not_found)?;
    Ok(WorkspaceAvatar {
        bytes,
        content_type,
    })
}

pub(super) async fn remove(id: &str, state: &SharedDaemonState) -> Result<(), ApiError> {
    match std::fs::remove_file(path(id, state).await?) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ApiError::service_unavailable(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{control_plane_store::WorkspaceConfig, state::DaemonState};
    use std::sync::Arc;
    use tokio::sync::RwLock;

    #[tokio::test]
    async fn avatar_is_isolated_durable_and_invalid_replacement_preserves_image() {
        let root =
            std::env::temp_dir().join(format!("anima-agent-avatar-{}", uuid::Uuid::new_v4()));
        let mut daemon = DaemonState::new();
        daemon.workspace = Some(WorkspaceConfig {
            root_path: root.clone(),
            company_name: "Test".into(),
            mission: "Test".into(),
            values: vec![],
        });
        let config: anima_core::AgentConfig =
            serde_json::from_value(serde_json::json!({"name":"One", "model":"test"})).unwrap();
        let first = daemon.create_agent(config.clone()).unwrap().state.id;
        let second = daemon.create_agent(config).unwrap().state.id;
        let state = Arc::new(RwLock::new(daemon));
        let png = b"\x89PNG\r\n\x1a\nfixture".to_vec();
        put(&first, png.clone(), Some("image/png"), &state)
            .await
            .unwrap();
        assert_eq!(get(&first, &state).await.unwrap().bytes, png);
        assert!(get(&second, &state).await.is_err());
        assert!(get("../escape", &state).await.is_err());
        assert!(
            put(&first, b"<svg/>".to_vec(), Some("image/svg+xml"), &state)
                .await
                .is_err()
        );
        assert!(put(&first, png.clone(), Some("image/jpeg"), &state)
            .await
            .is_err());
        assert!(put(
            &first,
            vec![0; MAX_WORKSPACE_AVATAR_BYTES + 1],
            Some("image/png"),
            &state
        )
        .await
        .is_err());
        assert_eq!(get(&first, &state).await.unwrap().bytes, png);
        assert_eq!(
            std::fs::read(path(&first, &state).await.unwrap()).unwrap(),
            png
        );
        remove(&first, &state).await.unwrap();
        remove(&first, &state).await.unwrap();
        assert!(get(&first, &state).await.is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
