use anima_core::{AgentConfig, Content, Message, MessageRole, ModelGenerateRequest};
use serde_json::Value;

use super::agencies::strip_code_fences;
use super::contracts::{AgentProfileEnvelope, AgentProfileResponse, GenerateProfileRequest};
use super::ApiError;
use crate::app::SharedDaemonState;

pub(crate) struct ProfilePreset {
    pub(crate) id: &'static str,
    pub(crate) style_guidance: &'static str,
}

pub(crate) const PROFILE_PRESETS: &[ProfilePreset] = &[
    ProfilePreset {
        id: "chief-of-staff",
        style_guidance: "Proactive, organized chief of staff. Briefs the owner first, anticipates needs, keeps crisp summaries. Never waits to be asked when something is clearly in scope.",
    },
    ProfilePreset {
        id: "calm-assistant",
        style_guidance: "Patient, thorough assistant. Asks before acting on anything ambiguous, explains reasoning, prefers correctness over speed.",
    },
    ProfilePreset {
        id: "senior-engineer",
        style_guidance: "Direct senior engineer. Code-first, minimal ceremony, flags risks plainly, no filler.",
    },
    ProfilePreset {
        id: "creative-partner",
        style_guidance: "Exploratory creative partner. Offers multiple angles, playful but grounded, generous with ideas.",
    },
];

pub(crate) fn profile_preset(id: &str) -> Option<&'static ProfilePreset> {
    PROFILE_PRESETS.iter().find(|preset| preset.id == id)
}

pub(crate) struct WorkspaceIdentity {
    pub(crate) company_name: String,
    pub(crate) mission: String,
    pub(crate) values: Vec<String>,
}

fn build_profile_prompt(
    preset: &ProfilePreset,
    intent: &str,
    workspace: &WorkspaceIdentity,
) -> String {
    let values = if workspace.values.is_empty() {
        "none specified".to_string()
    } else {
        workspace.values.join(", ")
    };
    let mut lines = Vec::new();
    lines.push("Draft a personality profile for a new agent.".to_string());
    lines.push(String::new());
    lines.push(format!("Company: {}", workspace.company_name));
    lines.push(format!("Mission: {}", workspace.mission));
    lines.push(format!("Values: {values}"));
    lines.push(format!("Personality preset: {}", preset.id));
    lines.push(format!("Preset style guidance: {}", preset.style_guidance));
    lines.push(format!("Owner's intent for this agent: {intent}"));
    lines.push(String::new());
    lines.push(
        "Respond with ONLY a JSON object (no markdown fences, no commentary) with these keys:"
            .to_string(),
    );
    lines.push("  - \"bio\": one sentence describing the agent".to_string());
    lines
        .push("  - \"adjectives\": array of 3 short lowercase personality trait words".to_string());
    lines.push("  - \"style\": one short line describing how the agent communicates".to_string());
    lines.push(
        "  - \"system\": the full system prompt for the agent, written in second person starting with \"You are\", weaving in the mission and values above, 4-8 sentences".to_string(),
    );
    lines.join("\n")
}

fn parse_profile_output(output: &str) -> Result<AgentProfileResponse, String> {
    let cleaned = strip_code_fences(output);
    let parsed: Value = match serde_json::from_str(&cleaned) {
        Ok(value) => value,
        Err(_) => {
            // Models sometimes wrap the object in prose; retry with the first
            // {...} span before giving up.
            let span = cleaned
                .find('{')
                .zip(cleaned.rfind('}'))
                .filter(|(start, end)| start < end)
                .map(|(start, end)| &cleaned[start..=end]);
            match span {
                Some(span) => serde_json::from_str(span)
                    .map_err(|error| format!("model output is not valid JSON: {error}"))?,
                None => return Err("model output is not valid JSON".to_string()),
            }
        }
    };
    let object = parsed
        .as_object()
        .ok_or_else(|| "model output must be a JSON object".to_string())?;
    let get_required_str = |key: &str| -> Result<String, String> {
        object
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("profile is missing a non-empty \"{key}\""))
    };
    let adjectives = object
        .get("adjectives")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .take(5)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let style = object
        .get("style")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("")
        .to_string();

    Ok(AgentProfileResponse {
        bio: get_required_str("bio")?,
        adjectives,
        style,
        system: get_required_str("system")?,
    })
}

pub(crate) async fn handle_generate_profile(
    body: Vec<u8>,
    state: &SharedDaemonState,
) -> Result<AgentProfileEnvelope, ApiError> {
    let request: GenerateProfileRequest = super::parse_json_body(body)?;
    let preset = profile_preset(request.preset_id.trim())
        .ok_or_else(|| ApiError::bad_request_static("unknown presetId"))?;
    let intent = request.intent.trim();
    if intent.is_empty() {
        return Err(ApiError::bad_request_static("intent is required"));
    }
    let provider = request.provider.as_deref().unwrap_or("").trim();

    let adapter = {
        let guard = state.read().await;
        // The deterministic adapter cannot generate real profiles; treat it
        // (and an explicitly unconfigured provider) as unavailable so the web
        // app falls back to preset templates.
        if provider.is_empty() || provider == "deterministic" {
            return Err(ApiError::bad_request_static(
                "PROFILE_GENERATION_UNAVAILABLE: no generative provider configured",
            ));
        }
        std::sync::Arc::clone(&guard.model_adapter)
    };

    let identity = WorkspaceIdentity {
        company_name: request.workspace.company_name.trim().to_string(),
        mission: request.workspace.mission.trim().to_string(),
        values: request
            .workspace
            .values
            .into_iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect(),
    };
    let prompt = build_profile_prompt(preset, intent, &identity);
    let generator_config = AgentConfig {
        name: "profile-generator".into(),
        model: request
            .model
            .filter(|model| !model.trim().is_empty())
            .unwrap_or_else(|| "gpt-4o-mini".into()),
        bio: None,
        lore: None,
        knowledge: None,
        topics: None,
        adjectives: None,
        style: None,
        provider: Some(provider.to_string()),
        system: Some("You output only valid JSON objects.".into()),
        tools: None,
        plugins: None,
        settings: None,
    };
    let model_request = ModelGenerateRequest {
        system: "You output only valid JSON objects.".into(),
        messages: vec![Message {
            id: String::new(),
            agent_id: String::new(),
            room_id: String::new(),
            content: Content {
                text: prompt,
                attachments: None,
                metadata: None,
            },
            role: MessageRole::User,
            created_at_ms: 0,
        }],
        temperature: Some(0.4),
        max_tokens: Some(1200),
    };

    let response = adapter
        .generate(&generator_config, &model_request)
        .await
        .map_err(|message| ApiError::bad_request(format!("profile model error: {message}")))?;
    let profile = parse_profile_output(&response.content.text).map_err(|error| {
        ApiError::service_unavailable(format!(
            "profile generation produced unusable output: {error}"
        ))
    })?;
    Ok(AgentProfileEnvelope { profile })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::DaemonState;
    use anima_core::{Content, ModelAdapter, ModelGenerateResponse, ModelStopReason, TokenUsage};
    use async_trait::async_trait;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    const SCRIPTED_PROFILE_JSON: &str = r#"{"bio":"A calm operator.","adjectives":["calm","precise"],"style":"brief, numbered","system":"You are Anima, chief of staff for Northwind."}"#;

    struct ScriptedModelAdapter {
        output: &'static str,
    }

    #[async_trait]
    impl ModelAdapter for ScriptedModelAdapter {
        fn provider(&self) -> &str {
            "scripted"
        }

        async fn generate(
            &self,
            _config: &AgentConfig,
            _request: &ModelGenerateRequest,
        ) -> Result<ModelGenerateResponse, String> {
            Ok(ModelGenerateResponse {
                content: Content {
                    text: self.output.to_string(),
                    attachments: None,
                    metadata: None,
                },
                tool_calls: None,
                usage: TokenUsage::default(),
                stop_reason: ModelStopReason::End,
            })
        }
    }

    fn scripted_state(output: &'static str) -> SharedDaemonState {
        Arc::new(RwLock::new(DaemonState::with_model_adapter(Arc::new(
            ScriptedModelAdapter { output },
        ))))
    }

    fn request_body(preset_id: &str, provider: Option<&str>) -> Vec<u8> {
        let provider_field = provider
            .map(|value| format!(r#","provider":"{value}""#))
            .unwrap_or_default();
        format!(
            r#"{{"presetId":"{preset_id}","intent":"watch my portfolio"{provider_field},"workspace":{{"companyName":"Northwind","mission":"equity research","values":["cite sources"]}}}}"#
        )
        .into_bytes()
    }

    #[test]
    fn known_presets_resolve() {
        for id in [
            "chief-of-staff",
            "calm-assistant",
            "senior-engineer",
            "creative-partner",
        ] {
            assert!(profile_preset(id).is_some(), "preset {id} should exist");
        }
        assert!(profile_preset("nope").is_none());
    }

    #[test]
    fn prompt_embeds_workspace_identity_and_intent() {
        let prompt = build_profile_prompt(
            profile_preset("chief-of-staff").unwrap(),
            "watch my portfolio",
            &WorkspaceIdentity {
                company_name: "Northwind".into(),
                mission: "equity research".into(),
                values: vec!["cite sources".into()],
            },
        );
        assert!(prompt.contains("Northwind"));
        assert!(prompt.contains("equity research"));
        assert!(prompt.contains("cite sources"));
        assert!(prompt.contains("watch my portfolio"));
    }

    #[test]
    fn parses_structured_profile_from_model_output() {
        let output = r#"{"bio":"A calm operator.","adjectives":["calm","precise"],"style":"brief, numbered","system":"You are Anima..."}"#;
        let profile = parse_profile_output(output).expect("valid profile parses");
        assert_eq!(profile.bio, "A calm operator.");
        assert_eq!(profile.adjectives, vec!["calm", "precise"]);
        assert!(profile.system.contains("You are Anima"));
    }

    #[test]
    fn parse_rejects_non_json_output() {
        assert!(parse_profile_output("sorry, I cannot").is_err());
    }

    #[tokio::test]
    async fn happy_path_returns_parsed_profile() {
        let state = scripted_state(SCRIPTED_PROFILE_JSON);
        let envelope =
            handle_generate_profile(request_body("chief-of-staff", Some("openai")), &state)
                .await
                .expect("happy path should succeed");
        assert_eq!(envelope.profile.bio, "A calm operator.");
        assert_eq!(envelope.profile.adjectives, vec!["calm", "precise"]);
        assert!(envelope.profile.system.contains("You are Anima"));
    }

    #[tokio::test]
    async fn deterministic_provider_is_unavailable() {
        let state = scripted_state(SCRIPTED_PROFILE_JSON);
        let error = handle_generate_profile(
            request_body("chief-of-staff", Some("deterministic")),
            &state,
        )
        .await
        .expect_err("deterministic provider should be rejected");
        assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
        assert!(
            error.message.starts_with("PROFILE_GENERATION_UNAVAILABLE"),
            "unexpected message: {}",
            error.message
        );
    }

    #[tokio::test]
    async fn missing_provider_is_unavailable() {
        let state = scripted_state(SCRIPTED_PROFILE_JSON);
        let error = handle_generate_profile(request_body("chief-of-staff", None), &state)
            .await
            .expect_err("missing provider should be rejected");
        assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
        assert!(
            error.message.starts_with("PROFILE_GENERATION_UNAVAILABLE"),
            "unexpected message: {}",
            error.message
        );
    }

    #[tokio::test]
    async fn unknown_preset_is_rejected() {
        let state = scripted_state(SCRIPTED_PROFILE_JSON);
        let error = handle_generate_profile(request_body("nope", Some("openai")), &state)
            .await
            .expect_err("unknown preset should be rejected");
        assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
    }
}
