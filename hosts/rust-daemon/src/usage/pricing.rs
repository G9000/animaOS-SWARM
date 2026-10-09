//! Prices for model calls: the built-in table plus the owner's overrides
//! (spec §11.1).

use anima_core::TokenUsage;
use anima_model_adapters::{
    canonical_provider_id, estimate_cost_micros, price_usage, CostEstimate, ModelPricing,
};
use serde::{Deserialize, Serialize};

use super::PricingSource;

pub(crate) const MAX_PRICING_OVERRIDES: usize = 100;
pub(crate) const MAX_PRICE_MICROS_PER_MTOK: u64 = 1_000_000_000_000;
pub(crate) const MAX_PRICING_PROVIDER_CHARS: usize = 64;
pub(crate) const MAX_PRICING_MODEL_CHARS: usize = 128;

pub(crate) const PRICING_TOO_MANY: &str = "at most 100 pricing overrides";
pub(crate) const PRICING_ENTRY_INVALID: &str =
    "each override needs a provider, a model, and prices from 0 to 1000000000000";
pub(crate) const PRICING_DUPLICATE: &str = "each provider and model may appear once";

/// One owner price: `model` is a lowercase prefix and the longest match wins.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PricingOverride {
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) input_micros_per_mtok: u64,
    pub(crate) output_micros_per_mtok: u64,
    #[serde(default)]
    pub(crate) cached_input_micros_per_mtok: Option<u64>,
}

impl PricingOverride {
    fn pricing(&self) -> ModelPricing {
        ModelPricing {
            input_micros_per_mtok: self.input_micros_per_mtok,
            output_micros_per_mtok: self.output_micros_per_mtok,
            cached_input_micros_per_mtok: self.cached_input_micros_per_mtok,
        }
    }
}

/// Validates and normalizes a replacement list: provider and model are
/// trimmed and lowercased. The error is one of the `PRICING_*` constants.
pub(crate) fn validate_overrides(
    list: Vec<PricingOverride>,
) -> Result<Vec<PricingOverride>, &'static str> {
    if list.len() > MAX_PRICING_OVERRIDES {
        return Err(PRICING_TOO_MANY);
    }
    let mut cleaned = Vec::with_capacity(list.len());
    for entry in list {
        let provider = entry.provider.trim().to_lowercase();
        let model = entry.model.trim().to_lowercase();
        let valid_text = |text: &str, max_chars: usize| {
            let chars = text.chars().count();
            (1..=max_chars).contains(&chars) && !text.chars().any(char::is_control)
        };
        let valid_rate = |rate: u64| rate <= MAX_PRICE_MICROS_PER_MTOK;
        if !valid_text(&provider, MAX_PRICING_PROVIDER_CHARS)
            || !valid_text(&model, MAX_PRICING_MODEL_CHARS)
            || !valid_rate(entry.input_micros_per_mtok)
            || !valid_rate(entry.output_micros_per_mtok)
            || !entry.cached_input_micros_per_mtok.is_none_or(valid_rate)
        {
            return Err(PRICING_ENTRY_INVALID);
        }
        // Two names of one provider (`google` and `gemini`) are one provider.
        let key = provider_key(&provider);
        if cleaned.iter().any(|known: &PricingOverride| {
            known.model == model && provider_key(&known.provider) == key
        }) {
            return Err(PRICING_DUPLICATE);
        }
        cleaned.push(PricingOverride {
            provider,
            model,
            ..entry
        });
    }
    Ok(cleaned)
}

/// A provider name in the form the table uses: its canonical id, or the
/// trimmed lowercase text for a provider the catalog does not know.
fn provider_key(provider: &str) -> String {
    canonical_provider_id(provider)
        .map(str::to_string)
        .unwrap_or_else(|| provider.trim().to_lowercase())
}

/// Resolution order: an owner override (same provider, longest model prefix),
/// then `chatgpt` as a subscription (no cost), then local providers as free,
/// then the table, otherwise unknown (no cost).
pub(crate) fn price_call(
    provider: &str,
    model: &str,
    usage: &TokenUsage,
    overrides: &[PricingOverride],
) -> (Option<u64>, PricingSource) {
    let wanted_provider = provider_key(provider);
    let model_lower = model.trim().to_lowercase();
    let matched = overrides
        .iter()
        .filter(|entry| {
            provider_key(&entry.provider) == wanted_provider
                && model_lower.starts_with(&entry.model.trim().to_lowercase())
        })
        .max_by_key(|entry| entry.model.trim().len());
    if let Some(entry) = matched {
        return (
            Some(price_usage(&entry.pricing(), usage)),
            PricingSource::Override,
        );
    }
    match estimate_cost_micros(provider, model, usage) {
        CostEstimate::Priced { micros } => (Some(micros), PricingSource::Table),
        CostEstimate::Free => (Some(0), PricingSource::Free),
        CostEstimate::Subscription => (None, PricingSource::Subscription),
        CostEstimate::Unknown => (None, PricingSource::Unknown),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(prompt: u64, completion: u64, cached: u64) -> TokenUsage {
        TokenUsage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            cached_prompt_tokens: cached,
            reasoning_tokens: 0,
        }
    }

    fn entry(provider: &str, model: &str, input: u64, output: u64) -> PricingOverride {
        PricingOverride {
            provider: provider.into(),
            model: model.into(),
            input_micros_per_mtok: input,
            output_micros_per_mtok: output,
            cached_input_micros_per_mtok: None,
        }
    }

    #[test]
    fn pricing_resolution_order() {
        let call = usage(1_000_000, 1_000_000, 0);

        let table = estimate_cost_micros("anthropic", "claude-fable-5-1", &call);
        let CostEstimate::Priced { micros: table_cost } = table else {
            panic!("the table prices claude-fable-5-1");
        };
        assert_eq!(
            price_call("anthropic", "claude-fable-5-1", &call, &[]),
            (Some(table_cost), PricingSource::Table),
            "a known table model prices equal to estimate_cost_micros"
        );

        let overrides = [
            entry("anthropic", "claude-fable", 1_000_000, 2_000_000),
            entry("anthropic", "claude-fable-5", 3_000_000, 4_000_000),
        ];
        assert_eq!(
            price_call("anthropic", "claude-fable-5-1", &call, &overrides),
            (Some(7_000_000), PricingSource::Override),
            "an override beats the table and the longest prefix wins"
        );
        assert_eq!(
            price_call("Anthropic", "CLAUDE-FABLE-5-1", &call, &overrides).1,
            PricingSource::Override,
            "case is ignored"
        );
        assert_eq!(
            price_call("openai", "claude-fable-5-1", &call, &overrides).1,
            PricingSource::Unknown,
            "another provider's override does not apply"
        );

        assert_eq!(
            price_call("chatgpt", "gpt-5", &call, &[]),
            (None, PricingSource::Subscription)
        );
        assert_eq!(
            price_call("ollama", "llama3", &call, &[]),
            (Some(0), PricingSource::Free)
        );
        assert_eq!(
            price_call("vllm", "anything", &call, &[]),
            (Some(0), PricingSource::Free)
        );
        assert_eq!(
            price_call("anthropic", "no-such-model", &call, &[]),
            (None, PricingSource::Unknown)
        );
        assert_eq!(
            price_call("mystery", "m", &call, &[]),
            (None, PricingSource::Unknown)
        );
    }

    #[test]
    fn an_override_can_price_a_model_the_table_lacks() {
        let call = usage(500_000, 100_000, 0);
        let overrides = [entry("mystery", "m-", 2_000_000, 10_000_000)];
        assert_eq!(
            price_call("mystery", "m-large", &call, &overrides),
            (Some(2_000_000), PricingSource::Override)
        );
        assert_eq!(
            price_call("mystery", "other", &call, &overrides).1,
            PricingSource::Unknown
        );
    }

    #[test]
    fn cached_rate_defaults_to_the_input_rate() {
        let call = usage(1_000_000, 0, 400_000);
        let mut priced = entry("mystery", "m", 1_000_000, 0);
        assert_eq!(
            price_call("mystery", "m", &call, &[priced.clone()]).0,
            Some(1_000_000),
            "cached tokens bill at the input rate"
        );
        priced.cached_input_micros_per_mtok = Some(100_000);
        assert_eq!(
            price_call("mystery", "m", &call, &[priced]).0,
            Some(640_000),
            "600k at the input rate plus 400k at the cached rate"
        );
    }

    #[test]
    fn validate_overrides_normalizes_and_rejects() {
        let cleaned = validate_overrides(vec![entry("  OpenAI ", " GPT-5 ", 1, 2)]).expect("valid");
        assert_eq!(
            (cleaned[0].provider.as_str(), cleaned[0].model.as_str()),
            ("openai", "gpt-5")
        );

        let many = (0..=MAX_PRICING_OVERRIDES)
            .map(|n| entry("p", &format!("m{n}"), 1, 1))
            .collect();
        assert_eq!(validate_overrides(many), Err(PRICING_TOO_MANY));
        assert_eq!(PRICING_TOO_MANY, "at most 100 pricing overrides");

        let invalid = |candidate: PricingOverride| {
            assert_eq!(
                validate_overrides(vec![candidate]),
                Err(PRICING_ENTRY_INVALID)
            );
        };
        invalid(entry("  ", "m", 1, 1));
        invalid(entry("p", "", 1, 1));
        invalid(entry("p", &"m".repeat(MAX_PRICING_MODEL_CHARS + 1), 1, 1));
        invalid(entry(
            &"p".repeat(MAX_PRICING_PROVIDER_CHARS + 1),
            "m",
            1,
            1,
        ));
        invalid(entry("p", "m\u{7}", 1, 1));
        invalid(entry("p", "m", MAX_PRICE_MICROS_PER_MTOK + 1, 1));
        invalid(entry("p", "m", 1, MAX_PRICE_MICROS_PER_MTOK + 1));
        let mut cached = entry("p", "m", 1, 1);
        cached.cached_input_micros_per_mtok = Some(MAX_PRICE_MICROS_PER_MTOK + 1);
        invalid(cached);
        assert!(validate_overrides(vec![entry("p", "m", MAX_PRICE_MICROS_PER_MTOK, 0)]).is_ok());
        assert!(
            validate_overrides(vec![entry("p", &"m".repeat(MAX_PRICING_MODEL_CHARS), 0, 0)])
                .is_ok()
        );
        assert_eq!(
            PRICING_ENTRY_INVALID,
            "each override needs a provider, a model, and prices from 0 to 1000000000000"
        );

        assert_eq!(
            validate_overrides(vec![entry("P", "M", 1, 1), entry("p", " m ", 2, 2)]),
            Err(PRICING_DUPLICATE)
        );
        assert_eq!(PRICING_DUPLICATE, "each provider and model may appear once");
    }

    #[test]
    fn aliases_of_one_provider_are_duplicates() {
        assert_eq!(
            validate_overrides(vec![
                entry("google", "gemini-x", 1, 1),
                entry(" Gemini ", "GEMINI-X", 2, 2),
            ]),
            Err(PRICING_DUPLICATE)
        );
        assert!(
            validate_overrides(vec![
                entry("google", "gemini-x", 1, 1),
                entry("gemini", "gemini-y", 2, 2),
            ])
            .is_ok(),
            "another model of the same provider is not a duplicate"
        );
    }

    #[test]
    fn overrides_round_trip_as_camel_case() {
        let json = serde_json::json!({
            "provider": "p", "model": "m",
            "inputMicrosPerMtok": 1, "outputMicrosPerMtok": 2
        });
        let parsed: PricingOverride = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.cached_input_micros_per_mtok, None);
        assert_eq!(
            serde_json::to_value(&parsed).unwrap()["inputMicrosPerMtok"],
            1
        );
    }
}
