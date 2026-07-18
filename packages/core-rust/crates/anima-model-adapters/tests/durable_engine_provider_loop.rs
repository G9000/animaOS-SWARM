use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anima_core::{
    AgentDefinition, AuthoritativePolicyChange, AuthoritativePolicyState, CapabilityError,
    CapabilityExecutionContext, CapabilityKind, CapabilityManifest, CapabilityManifestInput,
    CapabilityReferenceId, CapabilityResult, CapabilityRetryAuthorization, CreateRun,
    CurrentPolicyResolution, CurrentPolicyResolver, DefinitionPin, DefinitionResolver,
    DurableAgentEngine, DurableCapabilityResult, DurableCapabilityStatus, DurableEngineConfig,
    EngineBoundaryAction, EngineCapabilityResult, EngineCapabilityRuntime, EngineControlSignal,
    EngineCrashInjector, EngineCrashPoint, EngineError, EngineErrorCode, EngineLiveEvent,
    EngineLiveEventSink, EnginePolicyRequest, EngineRunOutcome, ExecutionStore,
    InMemoryExecutionStore, LifecyclePolicy, MemoryPolicy, ModelPolicy, PolicyContext,
    PolicyRestrictions, ProfileRef, RecoveryAction, RecoveryMode, ResolvedCapability, RiskLevel,
    Run, RuntimeCompatibility, RuntimeLimits, Session, SessionConcurrencyPolicy,
};
use anima_model_adapters::{ProviderAdapterConfig, ProviderCredential, ProviderModelAdapter};
use async_trait::async_trait;
use axum::{routing::post, Router};
use tokio::net::TcpListener;
use uuid::Uuid;

#[derive(Clone)]
struct StaticDefinition(AgentDefinition);

#[async_trait]
impl DefinitionResolver for StaticDefinition {
    async fn resolve(&self, pin: &DefinitionPin) -> Result<AgentDefinition, EngineError> {
        if pin.id() == self.0.id && pin.version() == self.0.version {
            Ok(self.0.clone())
        } else {
            Err(EngineError::new(EngineErrorCode::DefinitionUnavailable))
        }
    }
}

struct AllowCurrentPolicy;

#[async_trait]
impl CurrentPolicyResolver for AllowCurrentPolicy {
    async fn resolve(
        &self,
        request: EnginePolicyRequest,
    ) -> Result<CurrentPolicyResolution, EngineError> {
        let context = PolicyContext::new(
            request.owner_id().to_string(),
            "provider-loop-actor",
            request.definition().id.clone(),
            request.definition().version,
            "provider-loop-workspace",
            CapabilityReferenceId::new(request.invocation().run_id()),
            request.manifest(),
            request.invocation(),
            request.definition().approval_policy_revision,
            PolicyRestrictions::default(),
            1_000_000,
        )
        .map_err(|_| EngineError::new(EngineErrorCode::Policy))?;
        Ok(CurrentPolicyResolution::new(context, vec![], None))
    }
}

struct RecordingRuntime {
    manifest: CapabilityManifest,
    calls: Mutex<Vec<CapabilityExecutionContext>>,
}

#[async_trait]
impl EngineCapabilityRuntime for RecordingRuntime {
    fn manifest(&self, id: &str, version: u32) -> Option<CapabilityManifest> {
        (id == self.manifest.id && version == self.manifest.version).then(|| self.manifest.clone())
    }

    async fn execute(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<EngineCapabilityResult, CapabilityError> {
        self.calls.lock().unwrap().push(context.clone());
        let durable = DurableCapabilityResult::new(
            CapabilityReferenceId::new(Uuid::new_v5(
                &Uuid::NAMESPACE_OID,
                context.invocation().id().as_bytes(),
            )),
            format!("jcs-v1:{}", "a".repeat(64)),
            self.manifest.schema_digest(),
            1,
            DurableCapabilityStatus::Completed,
        )?;
        Ok(EngineCapabilityResult {
            output: CapabilityResult::new(serde_json::json!({"ok": true})),
            durable,
        })
    }

    async fn recover_dispatched(
        &self,
        _context: CapabilityExecutionContext,
    ) -> Result<RecoveryAction, CapabilityError> {
        Err(CapabilityError::unavailable())
    }

    async fn execute_retry(
        &self,
        _context: CapabilityExecutionContext,
        _authorization: CapabilityRetryAuthorization,
    ) -> Result<EngineCapabilityResult, CapabilityError> {
        Err(CapabilityError::unavailable())
    }
}

struct Continue;

impl EngineControlSignal for Continue {
    fn at_boundary(&self) -> EngineBoundaryAction {
        EngineBoundaryAction::Continue
    }
}

struct NeverCrash;

impl EngineCrashInjector for NeverCrash {
    fn should_crash(&self, _point: EngineCrashPoint) -> bool {
        false
    }
}

struct IgnoreLive;

#[async_trait]
impl EngineLiveEventSink for IgnoreLive {
    async fn emit(&self, _event: EngineLiveEvent) -> Result<(), EngineError> {
        Ok(())
    }
}

#[tokio::test]
async fn openai_and_anthropic_real_adapters_drive_the_durable_capability_loop() {
    for (offset, provider) in ["openai", "anthropic"].into_iter().enumerate() {
        run_provider_loop(provider, 0x900 + (offset as u128 * 0x10)).await;
    }
}

async fn run_provider_loop(provider: &str, seed: u128) {
    let requests = Arc::new(AtomicUsize::new(0));
    let app = match provider {
        "openai" => Router::new().route(
            "/v1/chat/completions",
            post({
                let requests = requests.clone();
                move || {
                    let requests = requests.clone();
                    async move {
                        let body = if requests.fetch_add(1, Ordering::SeqCst) == 0 {
                            concat!(
                                "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_\",\"type\":\"function\",\"function\":{\"name\":\"workspace.\",\"arguments\":\"{\\\"path\\\":\\\"provider\"}}]}}]}\n\n",
                                "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"engine\",\"function\":{\"name\":\"write\",\"arguments\":\".txt\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
                                "data: [DONE]\n\n"
                            )
                        } else {
                            concat!(
                                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"provider-complete\"},\"finish_reason\":\"stop\"}]}\n\n",
                                "data: [DONE]\n\n"
                            )
                        };
                        ([("content-type", "text/event-stream")], body)
                    }
                }
            }),
        ),
        "anthropic" => Router::new().route(
            "/v1/messages",
            post({
                let requests = requests.clone();
                move || {
                    let requests = requests.clone();
                    async move {
                        let body = if requests.fetch_add(1, Ordering::SeqCst) == 0 {
                            concat!(
                                "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1}}}\n\n",
                                "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_engine\",\"name\":\"workspace.write\",\"input\":{}}}\n\n",
                                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"provider.txt\\\"}\"}}\n\n",
                                "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                                "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":1}}\n\n",
                                "data: {\"type\":\"message_stop\"}\n\n"
                            )
                        } else {
                            concat!(
                                "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1}}}\n\n",
                                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"provider-complete\"}}\n\n",
                                "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
                                "data: {\"type\":\"message_stop\"}\n\n"
                            )
                        };
                        ([("content-type", "text/event-stream")], body)
                    }
                }
            }),
        ),
        _ => unreachable!(),
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let manifest = capability_manifest();
    let definition = definition(provider, &manifest);
    let owner_id = Uuid::from_u128(seed + 1);
    let run_id = Uuid::from_u128(seed + 3);
    let session = Session::new_for_definition(
        Uuid::from_u128(seed + 2),
        &definition,
        SessionConcurrencyPolicy::Serial,
    )
    .unwrap();
    let queued = Run::queued(run_id, session.id(), &definition.id, definition.version).unwrap();
    let store = Arc::new(InMemoryExecutionStore::default());
    store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                owner_id,
                session,
                queued,
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await
        .unwrap();
    store
        .apply_authoritative_policy(
            owner_id,
            AuthoritativePolicyChange::create(
                AuthoritativePolicyState::active(owner_id, &definition.id, 1, 1, None).unwrap(),
            ),
        )
        .await
        .unwrap();
    let runtime = Arc::new(RecordingRuntime {
        manifest,
        calls: Mutex::new(vec![]),
    });
    let provider_base = if provider == "openai" {
        format!("{base_url}/v1")
    } else {
        base_url
    };
    let adapter = ProviderModelAdapter::new(ProviderAdapterConfig {
        providers: BTreeMap::from([(
            provider.to_owned(),
            ProviderCredential {
                api_key: Some("test-key".into()),
                base_url: provider_base,
            },
        )]),
    });
    let engine = DurableAgentEngine::new(
        store,
        Arc::new(adapter),
        Arc::new(StaticDefinition(definition)),
        Arc::new(AllowCurrentPolicy),
        runtime.clone(),
        Arc::new(anima_core::ManualExecutionClock::default()),
        Arc::new(IgnoreLive),
        Arc::new(NeverCrash),
        DurableEngineConfig::default(),
    )
    .unwrap();

    let outcome = engine.run(owner_id, run_id, &Continue).await.unwrap();
    assert!(matches!(
        outcome,
        EngineRunOutcome::Completed { ref content } if content.text == "provider-complete"
    ));
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    let calls = runtime.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].invocation().capability_id(), "workspace.write");
    assert_eq!(
        calls[0].normalized_arguments(),
        &serde_json::json!({"path": "provider.txt"})
    );
}

fn capability_manifest() -> CapabilityManifest {
    CapabilityManifest::new(CapabilityManifestInput {
        id: "workspace.write".into(),
        version: 1,
        kind: CapabilityKind::Workspace,
        label: "Write".into(),
        description: "writes one fixture".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["path"],
            "properties": {"path": {"type": "string"}},
            "additionalProperties": false
        }),
        output_schema: serde_json::json!({
            "type": "object",
            "required": ["ok"],
            "properties": {"ok": {"type": "boolean"}},
            "additionalProperties": false
        }),
        side_effects: true,
        risk_level: RiskLevel::Low,
        host_permissions: vec![],
        secret_references: vec![],
        environment_requirements: vec![],
        timeout_ms: 1_000,
        cancellation_supported: true,
        max_retries: 1,
        idempotent: true,
        recovery_mode: RecoveryMode::KeyedIdempotent,
        supports_streaming: false,
        supports_artifacts: false,
        supports_citations: false,
        compatibility: RuntimeCompatibility {
            minimum_runtime_schema_version: 1,
            maximum_runtime_schema_version: 1,
            manifest_schema_version: 1,
        },
    })
    .unwrap()
}

fn definition(provider: &str, manifest: &CapabilityManifest) -> AgentDefinition {
    AgentDefinition {
        schema_version: 1,
        id: "provider-loop".into(),
        version: 1,
        name: "provider-loop".into(),
        display_name: "Provider Loop".into(),
        description: "test".into(),
        persona: "test".into(),
        system: "call the tool".into(),
        model: ModelPolicy {
            provider: provider.into(),
            model: "fixture".into(),
            credential_reference: None,
            temperature: None,
        },
        source_profile: ProfileRef {
            profile_id: "empty".into(),
            profile_version: 1,
        },
        resolved_capabilities: vec![ResolvedCapability {
            capability_id: manifest.id.clone(),
            manifest_version: manifest.version,
            schema_digest: manifest.schema_digest().into(),
            override_config: None,
            approval_policy_revision: 1,
        }],
        memory: MemoryPolicy {
            enabled: false,
            namespace: "test".into(),
            retention_days: None,
        },
        approval_policy_id: "default".into(),
        approval_policy_revision: 1,
        approval_restrictions: vec![],
        limits: RuntimeLimits {
            max_turns: 4,
            timeout_ms: 30_000,
            max_concurrent_tasks: 1,
        },
        lifecycle: LifecyclePolicy {
            auto_start: false,
            restart_on_failure: false,
            max_restarts: 0,
            allows_concurrent_sessions: false,
        },
        host_requirements: vec![],
    }
}
