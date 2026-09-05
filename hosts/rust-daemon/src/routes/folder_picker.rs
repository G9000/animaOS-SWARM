use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;

use super::{json_response, ApiError, AppState, ErrorBody};

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct FolderPickerResponse {
    root_path: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/workspace/pick-folder",
    tag = "workspace",
    responses(
        (status = 200, description = "Selected folder, or null when cancelled", body = FolderPickerResponse),
        (status = 403, description = "Local owner authorization required", body = ErrorBody),
        (status = 409, description = "A folder dialog is already open", body = ErrorBody),
        (status = 503, description = "Native folder picker unavailable", body = ErrorBody)
    )
)]
pub(super) async fn pick_folder(State(state): State<AppState>, request: Request) -> Response {
    if state.local_owner.authorize(request.headers()).is_err() {
        return json_response(
            StatusCode::FORBIDDEN,
            &ErrorBody {
                error: "Folder selection requires an approved local browser".into(),
            },
        );
    }
    match select_folder().await {
        Ok(root_path) => json_response(StatusCode::OK, &FolderPickerResponse { root_path }),
        Err(error) => error.into_response(),
    }
}

#[cfg(windows)]
async fn select_folder() -> Result<Option<String>, ApiError> {
    static DIALOG: std::sync::LazyLock<std::sync::Arc<tokio::sync::Semaphore>> =
        std::sync::LazyLock::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(1)));
    run_dialog(std::sync::Arc::clone(&DIALOG), || {
        rfd::FileDialog::new()
            .set_title("Choose your animaOS workspace folder")
            .pick_folder()
            .map(|path| {
                path.into_os_string().into_string().map_err(|_| {
                    ApiError::service_unavailable("The selected folder path is not valid Unicode")
                })
            })
            .transpose()
    })
    .await
}

#[cfg(any(windows, test))]
async fn run_dialog(
    gate: std::sync::Arc<tokio::sync::Semaphore>,
    pick: impl FnOnce() -> Result<Option<String>, ApiError> + Send + 'static,
) -> Result<Option<String>, ApiError> {
    let permit = gate
        .try_acquire_owned()
        .map_err(|_| ApiError::conflict("A folder dialog is already open"))?;
    tokio::task::spawn_blocking(move || {
        // Keep the permit until the native dialog closes, even if HTTP disconnects.
        let _permit = permit;
        pick()
    })
    .await
    .map_err(|_| ApiError::service_unavailable("Could not open the folder picker"))?
}

#[cfg(not(windows))]
async fn select_folder() -> Result<Option<String>, ApiError> {
    Err(ApiError::service_unavailable(
        "Folder browsing is available on Windows. Enter the folder path instead.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    #[tokio::test]
    async fn selection_cancellation_and_failure_release_the_dialog() {
        let gate = Arc::new(Semaphore::new(1));
        assert_eq!(
            run_dialog(gate.clone(), || Ok(Some("C:\\My workspace".into())))
                .await
                .unwrap(),
            Some("C:\\My workspace".into())
        );
        assert!(run_dialog(gate.clone(), || Ok(None))
            .await
            .unwrap()
            .is_none());
        assert!(
            run_dialog(gate.clone(), || Err(ApiError::service_unavailable(
                "failed"
            )))
            .await
            .is_err()
        );
        assert_eq!(gate.available_permits(), 1);
    }

    #[tokio::test]
    async fn disconnected_request_keeps_dialog_exclusive_until_it_closes() {
        let gate = Arc::new(Semaphore::new(1));
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (close_tx, close_rx) = std::sync::mpsc::channel();
        let task = tokio::spawn(run_dialog(gate.clone(), move || {
            entered_tx.send(()).unwrap();
            close_rx.recv().unwrap();
            Ok(None)
        }));
        entered_rx.await.unwrap();
        task.abort();
        let _ = task.await;
        let result = run_dialog(gate.clone(), || panic!("second dialog must not open")).await;
        assert_eq!(
            result.unwrap_err().into_response().status(),
            StatusCode::CONFLICT
        );
        close_tx.send(()).unwrap();
        let _permit = tokio::time::timeout(std::time::Duration::from_secs(2), gate.acquire())
            .await
            .unwrap()
            .unwrap();
    }
}
