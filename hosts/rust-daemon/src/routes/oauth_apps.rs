use axum::extract::{Path, Request as AxumRequest, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response as AxumResponse};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use zeroize::Zeroizing;

use crate::connectors::oauth_apps::{
    OAuthAppCredentials, OAuthAppError, OAuthAppSource, OAuthAppStatus, OAuthProvider,
};

use super::contracts::ConnectorErrorBody;
use super::http::{json_response, read_limited_body, LocalOwnerRejection};
use super::AppState;

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OAuthAppCredentialsRequest {
    client_id: String,
    client_secret: String,
    tenant: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OAuthAppStatusResponse {
    provider: String,
    configured: bool,
    source: Option<String>,
    client_id_hint: Option<String>,
    redirect_uris: Vec<String>,
    tenant: Option<String>,
}

impl From<OAuthAppStatus> for OAuthAppStatusResponse {
    fn from(status: OAuthAppStatus) -> Self {
        Self {
            provider: provider_name(status.provider).to_string(),
            configured: status.configured,
            source: status.source.map(|source| match source {
                OAuthAppSource::Vault => "vault".to_string(),
                OAuthAppSource::Environment => "environment".to_string(),
            }),
            client_id_hint: status.client_id_hint,
            redirect_uris: status.redirect_uris,
            tenant: status.tenant,
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/connectors/oauth-apps/{provider}",
    tag = "connectors",
    params(("provider" = String, Path, description = "OAuth provider: google or microsoft")),
    responses(
        (status = 200, description = "Redacted OAuth application status", body = OAuthAppStatusResponse),
        (status = 400, description = "Unsupported OAuth provider", body = ConnectorErrorBody),
        (status = 403, description = "Local owner authorization required", body = ConnectorErrorBody),
        (status = 503, description = "OAuth application credential vault unavailable", body = ConnectorErrorBody)
    )
)]
pub(super) async fn get_oauth_app(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    request: AxumRequest,
) -> AxumResponse {
    if let Err(rejection) = state.local_owner.authorize(request.headers()) {
        return local_owner_error(rejection);
    }
    let provider = match parse_provider(&provider) {
        Ok(provider) => provider,
        Err(response) => return response,
    };
    match state.oauth_apps.status(provider).await {
        Ok(status) => status_response(status),
        Err(error) => oauth_app_error(error),
    }
}

#[utoipa::path(
    put,
    path = "/api/connectors/oauth-apps/{provider}",
    tag = "connectors",
    params(("provider" = String, Path, description = "OAuth provider: google or microsoft")),
    request_body = OAuthAppCredentialsRequest,
    responses(
        (status = 200, description = "OAuth application credentials saved", body = OAuthAppStatusResponse),
        (status = 400, description = "Invalid provider or configuration", body = ConnectorErrorBody),
        (status = 403, description = "Local owner authorization required", body = ConnectorErrorBody),
        (status = 409, description = "Configuration is in use or environment-managed", body = ConnectorErrorBody),
        (status = 503, description = "OAuth application credential vault unavailable", body = ConnectorErrorBody)
    )
)]
pub(super) async fn put_oauth_app(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    request: AxumRequest,
) -> AxumResponse {
    if let Err(rejection) = state.local_owner.authorize(request.headers()) {
        return local_owner_error(rejection);
    }
    let provider = match parse_provider(&provider) {
        Ok(provider) => provider,
        Err(response) => return response,
    };
    let body = match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(body) => Zeroizing::new(body),
        Err(_) => return invalid_configuration(),
    };
    let request = match parse_oauth_app_request(body) {
        Ok(request) => request,
        Err(_) => return invalid_configuration(),
    };
    let credentials = match OAuthAppCredentials::new(
        provider,
        request.client_id,
        request.client_secret,
        request.tenant,
    ) {
        Ok(credentials) => credentials,
        Err(_) => return invalid_configuration(),
    };
    match state.oauth_apps.status(provider).await {
        Ok(status) if status.source == Some(OAuthAppSource::Environment) => {
            return environment_managed()
        }
        Ok(_) => {}
        Err(error) => return oauth_app_error(error),
    }
    if let Err(error) = state.put_oauth_app_if_unused(provider, credentials).await {
        return oauth_app_error(error);
    }
    match state.oauth_apps.status(provider).await {
        Ok(status) => status_response(status),
        Err(error) => oauth_app_error(error),
    }
}

fn parse_oauth_app_request(body: Zeroizing<Vec<u8>>) -> Result<OAuthAppCredentialsRequest, ()> {
    serde_json::from_slice(body.as_slice()).map_err(|_| ())
}

#[utoipa::path(
    delete,
    path = "/api/connectors/oauth-apps/{provider}",
    tag = "connectors",
    params(("provider" = String, Path, description = "OAuth provider: google or microsoft")),
    responses(
        (status = 204, description = "OAuth application credentials deleted"),
        (status = 400, description = "Unsupported OAuth provider", body = ConnectorErrorBody),
        (status = 403, description = "Local owner authorization required", body = ConnectorErrorBody),
        (status = 409, description = "Configuration is in use or environment-managed", body = ConnectorErrorBody),
        (status = 503, description = "OAuth application credential vault unavailable", body = ConnectorErrorBody)
    )
)]
pub(super) async fn delete_oauth_app(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    request: AxumRequest,
) -> AxumResponse {
    if let Err(rejection) = state.local_owner.authorize(request.headers()) {
        return local_owner_error(rejection);
    }
    let provider = match parse_provider(&provider) {
        Ok(provider) => provider,
        Err(response) => return response,
    };
    match state.oauth_apps.status(provider).await {
        Ok(status) if status.source == Some(OAuthAppSource::Environment) => {
            return environment_managed()
        }
        Ok(_) => {}
        Err(error) => return oauth_app_error(error),
    }
    match state.delete_oauth_app_if_unused(provider).await {
        Ok(_) => no_store(StatusCode::NO_CONTENT.into_response()),
        Err(error) => oauth_app_error(error),
    }
}

fn parse_provider(provider: &str) -> Result<OAuthProvider, AxumResponse> {
    match provider {
        "google" => Ok(OAuthProvider::Google),
        "microsoft" => Ok(OAuthProvider::Microsoft),
        _ => Err(error_response(
            StatusCode::BAD_REQUEST,
            "oauth_app_invalid_provider",
            "OAuth application provider is invalid",
        )),
    }
}

fn provider_name(provider: OAuthProvider) -> &'static str {
    match provider {
        OAuthProvider::Google => "google",
        OAuthProvider::Microsoft => "microsoft",
    }
}

fn status_response(status: OAuthAppStatus) -> AxumResponse {
    no_store(json_response(
        StatusCode::OK,
        &OAuthAppStatusResponse::from(status),
    ))
}

fn invalid_configuration() -> AxumResponse {
    error_response(
        StatusCode::BAD_REQUEST,
        "oauth_app_invalid_configuration",
        "OAuth application configuration is invalid",
    )
}

fn environment_managed() -> AxumResponse {
    error_response(
        StatusCode::CONFLICT,
        "oauth_app_environment_managed",
        "OAuth application configuration is managed by the environment",
    )
}

fn oauth_app_error(error: OAuthAppError) -> AxumResponse {
    match error {
        OAuthAppError::InvalidClientId
        | OAuthAppError::InvalidClientSecret
        | OAuthAppError::InvalidTenant => invalid_configuration(),
        OAuthAppError::DependenciesExist => error_response(
            StatusCode::CONFLICT,
            "oauth_app_configuration_in_use",
            "OAuth application configuration is in use",
        ),
        OAuthAppError::VaultUnavailable
        | OAuthAppError::InvalidVaultPayload
        | OAuthAppError::UnsupportedVaultPayloadVersion
        | OAuthAppError::VaultStateUncertain
        | OAuthAppError::OperationCancelled => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "oauth_app_credential_vault_unavailable",
            "OAuth application credential vault is unavailable",
        ),
    }
}

fn local_owner_error(rejection: LocalOwnerRejection) -> AxumResponse {
    match rejection {
        LocalOwnerRejection::LocalAdminRequired => error_response(
            StatusCode::FORBIDDEN,
            "connector_local_admin_required",
            "local connector administration authorization is required",
        ),
        LocalOwnerRejection::OriginRejected => error_response(
            StatusCode::FORBIDDEN,
            "connector_origin_rejected",
            "browser origin is not approved for connector administration",
        ),
    }
}

fn error_response(status: StatusCode, code: &str, message: &str) -> AxumResponse {
    no_store(json_response(
        status,
        &ConnectorErrorBody {
            code: code.to_string(),
            error: message.to_string(),
        },
    ))
}

fn no_store(mut response: AxumResponse) -> AxumResponse {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
        Router,
    };
    use tokio::sync::{RwLock, Semaphore};
    use tower::ServiceExt;
    use zeroize::Zeroizing;

    use crate::{
        agent_runs::AgentRunCoordinator,
        app::{DaemonConfig, DeterministicMailTransport},
        connectors::{
            credentials::InMemoryCredentialStore,
            gcalendar::{
                client::UnconfiguredGoogleTransport, store::InMemoryGoogleCredentialStore,
                CalendarManager,
            },
            mail::MailManager,
            oauth_apps::{
                InMemoryOAuthAppVault, OAuthAppError, OAuthAppService, OAuthEnvironment,
                OAuthVaultBackend, OAuthVaultError,
            },
            runtime::ConnectorManager,
            telegram::TelegramClient,
        },
        routes::http::{ApiKeyPolicy, LocalOwnerPolicy},
        schedules::SchedulerService,
        state::DaemonState,
    };

    use super::{oauth_app_error, parse_oauth_app_request};

    struct TestEnvironment(HashMap<String, String>);

    impl OAuthEnvironment for TestEnvironment {
        fn get(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
    }

    struct FailingVault;

    impl OAuthVaultBackend for FailingVault {
        fn load(&self, _: &str, _: &str) -> Result<Option<Zeroizing<String>>, OAuthVaultError> {
            Err(OAuthVaultError::Unavailable)
        }

        fn put(&self, _: &str, _: &str, _: &str) -> Result<(), OAuthVaultError> {
            Err(OAuthVaultError::Unavailable)
        }

        fn delete(&self, _: &str, _: &str) -> Result<(), OAuthVaultError> {
            Err(OAuthVaultError::Unavailable)
        }
    }

    fn app(oauth_apps: OAuthAppService, max_request_bytes: usize) -> Router {
        let state = Arc::new(RwLock::new(DaemonState::new()));
        let limiter = Arc::new(Semaphore::new(4));
        let runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::clone(&limiter));
        let connector_manager = ConnectorManager::new(
            Arc::clone(&state),
            runs.clone(),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(TelegramClient::new().expect("test Telegram client should configure")),
        );
        let scheduler =
            SchedulerService::new(Arc::clone(&state), runs.clone(), connector_manager.clone());
        let calendar = CalendarManager::new(
            &state,
            runs.clone(),
            Arc::new(InMemoryGoogleCredentialStore::default()),
            Arc::new(UnconfiguredGoogleTransport),
            oauth_apps.clone(),
        );
        let mail = MailManager::new(
            &state,
            runs.clone(),
            Arc::new(InMemoryGoogleCredentialStore::default()),
            Arc::new(DeterministicMailTransport),
            oauth_apps.clone(),
        );
        super::super::router_with_services_with_policies(
            state,
            DaemonConfig {
                max_request_bytes,
                ..DaemonConfig::default()
            },
            limiter,
            runs,
            connector_manager,
            calendar,
            mail,
            oauth_apps,
            scheduler,
            LocalOwnerPolicy::for_test(true, None),
            ApiKeyPolicy::for_test(None),
        )
    }

    fn request(method: &str, uri: &str, body: Body, authorized: bool) -> Request<Body> {
        let mut builder = Request::builder().method(method).uri(uri);
        if authorized {
            builder = builder
                .header("host", "127.0.0.1:8080")
                .header("origin", "http://localhost:4200");
        }
        builder.body(body).unwrap()
    }

    async fn json(response: axum::response::Response) -> serde_json::Value {
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    #[test]
    fn secret_request_parser_accepts_valid_json_and_rejects_trailing_malformed_data() {
        let parsed = parse_oauth_app_request(Zeroizing::new(
            br#"{"clientId":"client","clientSecret":"secret","tenant":"common"}"#.to_vec(),
        ))
        .unwrap();
        assert_eq!(parsed.client_id, "client");
        assert_eq!(parsed.client_secret, "secret");
        assert_eq!(parsed.tenant.as_deref(), Some("common"));

        assert!(parse_oauth_app_request(Zeroizing::new(
            br#"{"clientId":"client","clientSecret":"secret"} trailing"#.to_vec(),
        ))
        .is_err());
    }

    #[tokio::test]
    async fn put_get_and_delete_are_exact_owner_only_routes_with_redacted_statuses() {
        let app = app(OAuthAppService::in_memory(), 512);
        let secret = "oauth-secret-sentinel";
        let put = app
            .clone()
            .oneshot(request(
                "PUT",
                "/api/connectors/oauth-apps/google",
                Body::from(format!(
                    r#"{{"clientId":"client-12345678","clientSecret":"{secret}"}}"#
                )),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(put.status(), StatusCode::OK);
        assert_eq!(put.headers()["cache-control"], "no-store");
        let put_body = json(put).await;
        assert_eq!(put_body["provider"], "google");
        assert_eq!(put_body["configured"], true);
        assert_eq!(put_body["source"], "vault");
        assert_eq!(put_body["clientIdHint"], "...5678");
        assert!(!put_body.to_string().contains(secret));
        assert!(put_body.get("clientSecret").is_none());

        let get = app
            .clone()
            .oneshot(request(
                "GET",
                "/api/connectors/oauth-apps/google",
                Body::empty(),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(get.status(), StatusCode::OK);
        assert_eq!(get.headers()["cache-control"], "no-store");
        assert_eq!(json(get).await, put_body);

        for path in [
            "/api/connectors/oauth-app/google",
            "/api/connectors/oauth-apps/google/extra",
        ] {
            let response = app
                .clone()
                .oneshot(request("GET", path, Body::empty(), true))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }

        let delete = app
            .clone()
            .oneshot(request(
                "DELETE",
                "/api/connectors/oauth-apps/google",
                Body::empty(),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(delete.status(), StatusCode::NO_CONTENT);
        assert_eq!(delete.headers()["cache-control"], "no-store");
        assert!(to_bytes(delete.into_body(), usize::MAX)
            .await
            .unwrap()
            .is_empty());

        let get = app
            .clone()
            .oneshot(request(
                "GET",
                "/api/connectors/oauth-apps/google",
                Body::empty(),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(json(get).await["configured"], false);
    }

    #[tokio::test]
    async fn invalid_provider_and_configuration_have_stable_400_codes() {
        let app = app(OAuthAppService::in_memory(), 512);
        let invalid_provider = app
            .clone()
            .oneshot(request(
                "GET",
                "/api/connectors/oauth-apps/github",
                Body::empty(),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(invalid_provider.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            json(invalid_provider).await["code"],
            "oauth_app_invalid_provider"
        );

        let invalid_configuration = app
            .clone()
            .oneshot(request(
                "PUT",
                "/api/connectors/oauth-apps/google",
                Body::from(r#"{"clientId":"client","clientSecret":"","tenant":"tenant"}"#),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(invalid_configuration.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            json(invalid_configuration).await["code"],
            "oauth_app_invalid_configuration"
        );
    }

    #[tokio::test]
    async fn put_authorizes_before_malformed_or_oversized_body_processing() {
        let app = app(OAuthAppService::in_memory(), 32);
        for body in [Body::from("{"), Body::from(vec![b'x'; 1_024])] {
            let response = app
                .clone()
                .oneshot(request(
                    "PUT",
                    "/api/connectors/oauth-apps/google",
                    body,
                    false,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(
                json(response).await["code"],
                "connector_local_admin_required"
            );
        }
    }

    #[tokio::test]
    async fn environment_managed_configuration_cannot_be_put_or_deleted() {
        let service = OAuthAppService::with_backends(
            Arc::new(InMemoryOAuthAppVault::default()),
            Arc::new(TestEnvironment(HashMap::from([
                ("ANIMA_GOOGLE_CLIENT_ID".into(), "environment-client".into()),
                (
                    "ANIMA_GOOGLE_CLIENT_SECRET".into(),
                    "environment-secret".into(),
                ),
            ]))),
        );
        let app = app(service, 512);
        for (method, body) in [
            (
                "PUT",
                Body::from(r#"{"clientId":"new-client","clientSecret":"new-secret"}"#),
            ),
            ("DELETE", Body::empty()),
        ] {
            let response = app
                .clone()
                .oneshot(request(
                    method,
                    "/api/connectors/oauth-apps/google",
                    body,
                    true,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CONFLICT);
            assert_eq!(
                json(response).await["code"],
                "oauth_app_environment_managed"
            );
        }
    }

    #[tokio::test]
    async fn vault_failures_are_redacted_with_one_stable_503_code() {
        let app = app(
            OAuthAppService::with_backends(
                Arc::new(FailingVault),
                Arc::new(TestEnvironment(HashMap::new())),
            ),
            512,
        );
        let response = app
            .oneshot(request(
                "GET",
                "/api/connectors/oauth-apps/microsoft",
                Body::empty(),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = json(response).await;
        assert_eq!(body["code"], "oauth_app_credential_vault_unavailable");
        assert_eq!(body.as_object().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn service_conflicts_and_internal_vault_failures_have_stable_mappings() {
        let response = oauth_app_error(OAuthAppError::DependenciesExist);
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            json(response).await["code"],
            "oauth_app_configuration_in_use"
        );
        for error in [
            OAuthAppError::VaultUnavailable,
            OAuthAppError::InvalidVaultPayload,
            OAuthAppError::UnsupportedVaultPayloadVersion,
            OAuthAppError::VaultStateUncertain,
            OAuthAppError::OperationCancelled,
        ] {
            let response = oauth_app_error(error);
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            let body = json(response).await;
            assert_eq!(body["code"], "oauth_app_credential_vault_unavailable");
            assert!(!body.to_string().contains(&error.to_string()));
        }
    }
}
