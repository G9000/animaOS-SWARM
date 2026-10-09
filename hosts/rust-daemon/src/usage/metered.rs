//! Metering the secondary model calls (spec §11): titles, compaction, and
//! profile and agency generation have no run record, so their callers wrap
//! the adapter in [`Metered`] and hand what it kept to [`record_secondary`].

use std::sync::{Arc, Mutex as StdMutex, MutexGuard};
use std::time::Instant;

use anima_core::primitives::now_millis;
use anima_core::{
    AgentConfig, ModelAdapter, ModelGenerateRequest, ModelGenerateResponse, TokenUsage,
};
use async_trait::async_trait;
use uuid::Uuid;

use super::{is_zero, usage_record, UsageCall, UsageSource};
use crate::app::SharedDaemonState;

/// The agent id of a call made for no agent (profile or agency generation
/// that names none).
pub(crate) const USAGE_NO_AGENT: &str = "system";

/// A `ModelAdapter` that forwards to `inner` and remembers each completed
/// `generate` (its usage, start time, and duration). A failed or cancelled
/// call remembers nothing. `stream` is not overridden, so the default
/// (`generate`, then `Final`) is metered too.
pub(crate) struct Metered {
    inner: Arc<dyn ModelAdapter>,
    /// Never held across `.await`.
    calls: StdMutex<Vec<MeteredCall>>,
}

/// One completed call.
#[derive(Clone, Debug)]
pub(crate) struct MeteredCall {
    pub(crate) usage: TokenUsage,
    pub(crate) at_ms: u64,
    pub(crate) duration_ms: u64,
}

impl Metered {
    pub(crate) fn new(inner: Arc<dyn ModelAdapter>) -> Self {
        Self {
            inner,
            calls: StdMutex::new(Vec::new()),
        }
    }

    /// The calls completed so far, oldest first; the meter is empty after.
    pub(crate) fn take(&self) -> Vec<MeteredCall> {
        std::mem::take(&mut *self.calls())
    }

    fn calls(&self) -> MutexGuard<'_, Vec<MeteredCall>> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[async_trait]
impl ModelAdapter for Metered {
    fn provider(&self) -> &str {
        self.inner.provider()
    }

    async fn generate(
        &self,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        let at_ms = now_millis();
        let started = Instant::now();
        let response = self.inner.generate(config, request).await?;
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.calls().push(MeteredCall {
            usage: response.usage.clone(),
            at_ms,
            duration_ms,
        });
        Ok(response)
    }
}

/// What a secondary call was for.
#[derive(Clone, Debug)]
pub(crate) struct SecondaryCall {
    pub(crate) agent_id: String,
    pub(crate) session_id: Option<String>,
    pub(crate) source: UsageSource,
    pub(crate) provider: String,
    pub(crate) model: String,
}

impl SecondaryCall {
    /// A call made with `config` (its provider falls back to `"unknown"`).
    pub(crate) fn for_config(
        agent_id: &str,
        session_id: Option<&str>,
        source: UsageSource,
        config: &AgentConfig,
    ) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            session_id: session_id.map(str::to_string),
            source,
            provider: config.provider.clone().unwrap_or_else(|| "unknown".into()),
            model: config.model.clone(),
        }
    }
}

/// Prices the calls the meter holds with the overrides now in force (a
/// short state read lock) and queues the rows on the history service. Ids
/// are `usage_<uuid-v4>`. A no-op when the meter is empty; a call whose
/// usage is all zero makes no row, as a run step's does not.
pub(crate) async fn record_secondary(
    state: &SharedDaemonState,
    meter: &Metered,
    call: SecondaryCall,
) {
    let calls = meter
        .take()
        .into_iter()
        .filter(|metered| !is_zero(&metered.usage))
        .collect::<Vec<_>>();
    if calls.is_empty() {
        return;
    }
    let (overrides, history) = {
        let guard = state.read().await;
        (guard.pricing_overrides.clone(), guard.history.clone())
    };
    let records = calls
        .iter()
        .map(|metered| {
            usage_record(
                &UsageCall {
                    id: format!("usage_{}", Uuid::new_v4()),
                    agent_id: call.agent_id.clone(),
                    session_id: call.session_id.clone(),
                    run_id: None,
                    source: call.source,
                    provider: call.provider.clone(),
                    model: call.model.clone(),
                    duration_ms: metered.duration_ms,
                    created_at_ms: metered.at_ms,
                },
                &metered.usage,
                &overrides,
            )
        })
        .collect();
    history.enqueue_usage(records);
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use anima_core::{Content, ModelStopReason, ModelStreamFrame, ModelStreamSink};
    use tokio::sync::{RwLock, Semaphore};

    use super::*;
    use crate::state::DaemonState;
    use crate::usage::{price_call, PricingOverride, PricingSource};

    enum Reply {
        Usage(TokenUsage),
        Fail,
        Hold(Arc<Semaphore>),
    }

    struct Fixed(Reply);

    #[async_trait]
    impl ModelAdapter for Fixed {
        fn provider(&self) -> &str {
            "fixed"
        }

        async fn generate(
            &self,
            _config: &AgentConfig,
            _request: &ModelGenerateRequest,
        ) -> Result<ModelGenerateResponse, String> {
            match &self.0 {
                Reply::Usage(usage) => Ok(ModelGenerateResponse {
                    content: Content {
                        text: "ok".into(),
                        ..Content::default()
                    },
                    tool_calls: None,
                    usage: usage.clone(),
                    stop_reason: ModelStopReason::End,
                }),
                Reply::Fail => Err("provider down".into()),
                Reply::Hold(entered) => {
                    entered.add_permits(1);
                    std::future::pending().await
                }
            }
        }
    }

    struct Frames(Mutex<Vec<ModelStreamFrame>>);

    #[async_trait]
    impl ModelStreamSink for Frames {
        async fn emit(&self, frame: ModelStreamFrame) -> Result<(), String> {
            self.0.lock().unwrap().push(frame);
            Ok(())
        }
    }

    fn tokens(prompt: u64, completion: u64) -> TokenUsage {
        TokenUsage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            ..TokenUsage::default()
        }
    }

    fn config() -> AgentConfig {
        AgentConfig {
            name: "meter".into(),
            model: "gpt-x-mini".into(),
            bio: None,
            lore: None,
            knowledge: None,
            topics: None,
            adjectives: None,
            style: None,
            provider: Some("openai".into()),
            system: None,
            tools: None,
            plugins: None,
            settings: None,
        }
    }

    fn request() -> ModelGenerateRequest {
        ModelGenerateRequest {
            system: String::new(),
            messages: Vec::new(),
            temperature: None,
            max_tokens: None,
        }
    }

    fn meter(reply: Reply) -> Metered {
        Metered::new(Arc::new(Fixed(reply)))
    }

    fn shared_state() -> SharedDaemonState {
        Arc::new(RwLock::new(DaemonState::new()))
    }

    #[tokio::test]
    async fn metered_records_each_completed_generate_with_timing() {
        let meter = meter(Reply::Usage(tokens(10, 2)));
        assert_eq!(meter.provider(), "fixed");
        let before = now_millis();
        meter.generate(&config(), &request()).await.unwrap();
        meter.generate(&config(), &request()).await.unwrap();
        let after = now_millis();

        let calls = meter.take();
        assert_eq!(calls.len(), 2);
        for call in &calls {
            assert_eq!(call.usage, tokens(10, 2));
            assert!((before..=after).contains(&call.at_ms));
        }
        assert!(meter.take().is_empty(), "take empties the meter");
    }

    #[tokio::test]
    async fn a_failed_generate_records_nothing() {
        let meter = meter(Reply::Fail);
        assert!(meter.generate(&config(), &request()).await.is_err());
        assert!(meter.take().is_empty());
    }

    #[tokio::test]
    async fn a_cancelled_generate_records_nothing() {
        let entered = Arc::new(Semaphore::new(0));
        let meter = meter(Reply::Hold(Arc::clone(&entered)));
        {
            let config = config();
            let request = request();
            let call = meter.generate(&config, &request);
            tokio::pin!(call);
            tokio::select! {
                _ = &mut call => panic!("the call is held"),
                permit = entered.acquire() => permit.unwrap().forget(),
            }
            // `call` is dropped here, mid-call.
        }
        assert!(meter.take().is_empty());
    }

    #[tokio::test]
    async fn streaming_through_the_default_path_is_counted_once() {
        let meter = meter(Reply::Usage(tokens(7, 3)));
        let sink = Frames(Mutex::new(Vec::new()));
        meter.stream(&config(), &request(), &sink).await.unwrap();
        let frames = sink.0.into_inner().unwrap();
        assert_eq!(frames.len(), 1);
        assert!(matches!(frames[0], ModelStreamFrame::Final(_)));
        let calls = meter.take();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].usage, tokens(7, 3));
    }

    #[tokio::test]
    async fn record_secondary_is_a_noop_for_an_empty_meter() {
        let state = shared_state();
        let empty = meter(Reply::Fail);
        let _ = empty.generate(&config(), &request()).await;
        let call = SecondaryCall::for_config("agent-a", None, UsageSource::Title, &config());
        record_secondary(&state, &empty, call.clone()).await;

        let zero = meter(Reply::Usage(TokenUsage::default()));
        zero.generate(&config(), &request()).await.unwrap();
        record_secondary(&state, &zero, call).await;

        assert!(state.read().await.history.pending_usage().is_empty());
    }

    #[tokio::test]
    async fn record_secondary_prices_with_the_overrides_in_force() {
        let state = shared_state();
        let overrides = vec![PricingOverride {
            provider: "openai".into(),
            model: "gpt-x".into(),
            input_micros_per_mtok: 3_000_000,
            output_micros_per_mtok: 9_000_000,
            cached_input_micros_per_mtok: None,
        }];
        state.write().await.set_pricing_overrides(overrides.clone());
        let meter = meter(Reply::Usage(tokens(1_000, 500)));
        let before = now_millis();
        meter.generate(&config(), &request()).await.unwrap();
        record_secondary(
            &state,
            &meter,
            SecondaryCall::for_config(
                "agent-a",
                Some("chat:a"),
                UsageSource::Compaction,
                &config(),
            ),
        )
        .await;

        let rows = state.read().await.history.pending_usage();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert!(row.id.starts_with("usage_"), "{}", row.id);
        assert_eq!(row.agent_id, "agent-a");
        assert_eq!(row.session_id.as_deref(), Some("chat:a"));
        assert_eq!(row.run_id, None);
        assert_eq!(row.source, UsageSource::Compaction);
        assert_eq!(
            (row.provider.as_str(), row.model.as_str()),
            ("openai", "gpt-x-mini")
        );
        assert_eq!((row.prompt_tokens, row.completion_tokens), (1_000, 500));
        assert!(row.created_at_ms >= before);
        assert_eq!(row.pricing_source, PricingSource::Override);
        assert_eq!(
            row.cost_micros,
            price_call("openai", "gpt-x-mini", &tokens(1_000, 500), &overrides).0
        );
        assert_eq!(row.cost_micros, Some(3_000 + 4_500));
        assert!(meter.take().is_empty(), "recording takes the calls");
    }

    #[test]
    fn a_call_without_a_provider_is_unknown() {
        let mut config = config();
        config.provider = None;
        let call = SecondaryCall::for_config(USAGE_NO_AGENT, None, UsageSource::Profile, &config);
        assert_eq!(call.provider, "unknown");
        assert_eq!(call.agent_id, "system");
    }
}
