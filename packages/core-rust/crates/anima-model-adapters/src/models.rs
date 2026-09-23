//! Built-in model metadata for context budgets and cost estimates.
//!
//! Rows are transcribed from `docs/superpowers/plans/data/2026-09-23-model-table.md`,
//! which cites the official page each value was read from on 2026-09-23.
//! Prices are micro-USD per one million tokens at the standard base tier.

use anima_core::TokenUsage;

use crate::catalog::resolve_provider;

pub const PRICING_TABLE_DATE: &str = "2026-09-23";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelPricing {
    pub input_micros_per_mtok: u64,
    pub output_micros_per_mtok: u64,
    /// Cache-read price; `None` bills cached tokens at the input rate.
    pub cached_input_micros_per_mtok: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    pub provider: &'static str,
    /// Lowercase API model id prefix; the longest matching prefix wins.
    pub model_prefix: &'static str,
    /// `None` when the provider does not publish a context window.
    pub context_window: Option<u32>,
    /// `None` when the provider does not publish a maximum output.
    pub max_output: Option<u32>,
    pub vision: bool,
    pub pricing: Option<ModelPricing>,
    pub source: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CostEstimate {
    Priced {
        micros: u64,
    },
    /// Local providers with no per-token charge.
    Free,
    /// ChatGPT subscription sign-in; no per-token charge.
    Subscription,
    Unknown,
}

const FREE_PROVIDERS: &[&str] = &["ollama", "vllm"];

static MODELS: &[ModelInfo] = &[
    // anthropic (12 rows)
    ModelInfo {
        provider: "anthropic",
        model_prefix: "claude-fable-5-1",
        context_window: Some(1_000_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 10_000_000,
            output_micros_per_mtok: 50_000_000,
            cached_input_micros_per_mtok: Some(250_000),
        }),
        source: "https://platform.claude.com/docs/en/about-claude/pricing",
    },
    ModelInfo {
        provider: "anthropic",
        model_prefix: "claude-opus-5-5",
        context_window: Some(1_000_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 4_000_000,
            output_micros_per_mtok: 20_000_000,
            cached_input_micros_per_mtok: Some(200_000),
        }),
        source: "https://platform.claude.com/docs/en/about-claude/pricing",
    },
    ModelInfo {
        provider: "anthropic",
        model_prefix: "claude-sonnet-5",
        context_window: Some(1_000_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 2_000_000,
            output_micros_per_mtok: 10_000_000,
            cached_input_micros_per_mtok: Some(200_000),
        }),
        source: "https://platform.claude.com/docs/en/about-claude/pricing",
    },
    ModelInfo {
        provider: "anthropic",
        model_prefix: "claude-haiku-4-5",
        context_window: Some(200_000),
        max_output: Some(64_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 1_000_000,
            output_micros_per_mtok: 5_000_000,
            cached_input_micros_per_mtok: Some(100_000),
        }),
        source: "https://platform.claude.com/docs/en/about-claude/pricing",
    },
    ModelInfo {
        provider: "anthropic",
        model_prefix: "claude-fable-5",
        context_window: Some(1_000_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 10_000_000,
            output_micros_per_mtok: 50_000_000,
            cached_input_micros_per_mtok: Some(1_000_000),
        }),
        source: "https://platform.claude.com/docs/en/about-claude/pricing",
    },
    ModelInfo {
        provider: "anthropic",
        model_prefix: "claude-opus-5",
        context_window: Some(1_000_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 5_000_000,
            output_micros_per_mtok: 25_000_000,
            cached_input_micros_per_mtok: Some(500_000),
        }),
        source: "https://platform.claude.com/docs/en/about-claude/pricing",
    },
    ModelInfo {
        provider: "anthropic",
        model_prefix: "claude-opus-4-8",
        context_window: Some(1_000_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 5_000_000,
            output_micros_per_mtok: 25_000_000,
            cached_input_micros_per_mtok: Some(500_000),
        }),
        source: "https://platform.claude.com/docs/en/about-claude/pricing",
    },
    ModelInfo {
        provider: "anthropic",
        model_prefix: "claude-opus-4-7",
        context_window: Some(1_000_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 5_000_000,
            output_micros_per_mtok: 25_000_000,
            cached_input_micros_per_mtok: Some(500_000),
        }),
        source: "https://platform.claude.com/docs/en/about-claude/pricing",
    },
    ModelInfo {
        provider: "anthropic",
        model_prefix: "claude-opus-4-6",
        context_window: Some(1_000_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 5_000_000,
            output_micros_per_mtok: 25_000_000,
            cached_input_micros_per_mtok: Some(500_000),
        }),
        source: "https://platform.claude.com/docs/en/about-claude/pricing",
    },
    ModelInfo {
        provider: "anthropic",
        model_prefix: "claude-opus-4-5",
        context_window: Some(200_000),
        max_output: Some(64_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 5_000_000,
            output_micros_per_mtok: 25_000_000,
            cached_input_micros_per_mtok: Some(500_000),
        }),
        source: "https://platform.claude.com/docs/en/about-claude/pricing",
    },
    ModelInfo {
        provider: "anthropic",
        model_prefix: "claude-sonnet-4-6",
        context_window: Some(1_000_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 3_000_000,
            output_micros_per_mtok: 15_000_000,
            cached_input_micros_per_mtok: Some(300_000),
        }),
        source: "https://platform.claude.com/docs/en/about-claude/pricing",
    },
    ModelInfo {
        provider: "anthropic",
        model_prefix: "claude-sonnet-4-5",
        context_window: Some(200_000),
        max_output: Some(64_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 3_000_000,
            output_micros_per_mtok: 15_000_000,
            cached_input_micros_per_mtok: Some(300_000),
        }),
        source: "https://platform.claude.com/docs/en/about-claude/pricing",
    },
    // openai (31 rows + 5 Notes-only priced variants below)
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-6-astra",
        context_window: Some(1_050_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 10_000_000,
            output_micros_per_mtok: 50_000_000,
            cached_input_micros_per_mtok: Some(1_000_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-6-sol",
        context_window: Some(1_050_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 2_000_000,
            output_micros_per_mtok: 10_000_000,
            cached_input_micros_per_mtok: Some(200_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-6-luna",
        context_window: Some(1_050_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 100_000,
            output_micros_per_mtok: 500_000,
            cached_input_micros_per_mtok: Some(10_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-5.6-sol",
        context_window: Some(1_050_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 4_000_000,
            output_micros_per_mtok: 20_000_000,
            cached_input_micros_per_mtok: Some(400_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-5.6-terra",
        context_window: Some(1_050_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 2_000_000,
            output_micros_per_mtok: 12_000_000,
            cached_input_micros_per_mtok: Some(200_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-5.6-luna",
        context_window: Some(1_050_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 200_000,
            output_micros_per_mtok: 1_200_000,
            cached_input_micros_per_mtok: Some(20_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-5.5",
        context_window: Some(1_050_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 5_000_000,
            output_micros_per_mtok: 30_000_000,
            cached_input_micros_per_mtok: Some(500_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-5.5-pro",
        context_window: Some(1_050_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 30_000_000,
            output_micros_per_mtok: 180_000_000,
            cached_input_micros_per_mtok: None,
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-5.4",
        context_window: Some(1_050_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 2_500_000,
            output_micros_per_mtok: 15_000_000,
            cached_input_micros_per_mtok: Some(250_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-5.4-mini",
        context_window: Some(400_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 750_000,
            output_micros_per_mtok: 4_500_000,
            cached_input_micros_per_mtok: Some(75_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-5.4-nano",
        context_window: Some(400_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 200_000,
            output_micros_per_mtok: 1_250_000,
            cached_input_micros_per_mtok: Some(20_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-5.4-pro",
        context_window: Some(1_050_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 30_000_000,
            output_micros_per_mtok: 180_000_000,
            cached_input_micros_per_mtok: None,
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-5.2",
        context_window: Some(400_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 1_750_000,
            output_micros_per_mtok: 14_000_000,
            cached_input_micros_per_mtok: Some(175_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-5.2-pro",
        context_window: Some(400_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 21_000_000,
            output_micros_per_mtok: 168_000_000,
            cached_input_micros_per_mtok: None,
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-5.1",
        context_window: Some(400_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 1_250_000,
            output_micros_per_mtok: 10_000_000,
            cached_input_micros_per_mtok: Some(125_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-5",
        context_window: Some(400_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 1_250_000,
            output_micros_per_mtok: 10_000_000,
            cached_input_micros_per_mtok: Some(125_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-5-mini",
        context_window: Some(400_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 250_000,
            output_micros_per_mtok: 2_000_000,
            cached_input_micros_per_mtok: Some(25_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-5-nano",
        context_window: Some(400_000),
        max_output: Some(128_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 50_000,
            output_micros_per_mtok: 400_000,
            cached_input_micros_per_mtok: Some(5_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-5-pro",
        context_window: Some(400_000),
        max_output: Some(272_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 15_000_000,
            output_micros_per_mtok: 120_000_000,
            cached_input_micros_per_mtok: None,
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-4.1",
        context_window: Some(1_047_576),
        max_output: Some(32_768),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 2_000_000,
            output_micros_per_mtok: 8_000_000,
            cached_input_micros_per_mtok: Some(500_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-4.1-mini",
        context_window: Some(1_047_576),
        max_output: Some(32_768),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 400_000,
            output_micros_per_mtok: 1_600_000,
            cached_input_micros_per_mtok: Some(100_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-4.1-nano",
        context_window: Some(1_047_576),
        max_output: Some(32_768),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 100_000,
            output_micros_per_mtok: 400_000,
            cached_input_micros_per_mtok: Some(25_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-4o",
        context_window: Some(128_000),
        max_output: Some(16_384),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 2_500_000,
            output_micros_per_mtok: 10_000_000,
            cached_input_micros_per_mtok: Some(1_250_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-4o-2024-05-13",
        context_window: Some(128_000),
        max_output: None,
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 5_000_000,
            output_micros_per_mtok: 15_000_000,
            cached_input_micros_per_mtok: None,
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-4o-mini",
        context_window: Some(128_000),
        max_output: Some(16_384),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 150_000,
            output_micros_per_mtok: 600_000,
            cached_input_micros_per_mtok: Some(75_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "o3",
        context_window: Some(200_000),
        max_output: Some(100_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 2_000_000,
            output_micros_per_mtok: 8_000_000,
            cached_input_micros_per_mtok: Some(500_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "o3-pro",
        context_window: Some(200_000),
        max_output: Some(100_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 20_000_000,
            output_micros_per_mtok: 80_000_000,
            cached_input_micros_per_mtok: None,
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "o4-mini",
        context_window: Some(200_000),
        max_output: Some(100_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 1_100_000,
            output_micros_per_mtok: 4_400_000,
            cached_input_micros_per_mtok: Some(275_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "o3-mini",
        context_window: Some(200_000),
        max_output: Some(100_000),
        vision: false,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 1_100_000,
            output_micros_per_mtok: 4_400_000,
            cached_input_micros_per_mtok: Some(550_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "o1",
        context_window: Some(200_000),
        max_output: Some(100_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 15_000_000,
            output_micros_per_mtok: 60_000_000,
            cached_input_micros_per_mtok: Some(7_500_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "o1-pro",
        context_window: Some(200_000),
        max_output: Some(100_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 150_000_000,
            output_micros_per_mtok: 600_000_000,
            cached_input_micros_per_mtok: None,
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    // From the data file's Notes ("Priced but not tabulated"): priced variants that
    // are not given their own table row upstream, so they fall through prefix
    // matching to a cheaper family row unless listed here. Context window is not
    // published for these; treated as unknown rather than guessed.
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-5.6-cyber",
        context_window: None,
        max_output: None,
        vision: false,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 12_500_000,
            output_micros_per_mtok: 75_000_000,
            cached_input_micros_per_mtok: Some(1_250_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-5.5-cyber",
        context_window: None,
        max_output: None,
        vision: false,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 12_500_000,
            output_micros_per_mtok: 75_000_000,
            cached_input_micros_per_mtok: Some(1_250_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-5.3-codex",
        context_window: None,
        max_output: None,
        vision: false,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 1_750_000,
            output_micros_per_mtok: 14_000_000,
            cached_input_micros_per_mtok: Some(175_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "chat-latest",
        context_window: None,
        max_output: None,
        vision: false,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 5_000_000,
            output_micros_per_mtok: 30_000_000,
            cached_input_micros_per_mtok: Some(500_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    ModelInfo {
        provider: "openai",
        model_prefix: "gpt-5-search-api",
        context_window: None,
        max_output: None,
        vision: false,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 1_250_000,
            output_micros_per_mtok: 10_000_000,
            cached_input_micros_per_mtok: Some(125_000),
        }),
        source: "https://developers.openai.com/api/docs/pricing",
    },
    // google (11 rows)
    ModelInfo {
        provider: "google",
        model_prefix: "gemini-3.8-flash",
        context_window: Some(1_048_576),
        max_output: Some(65_536),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 750_000,
            output_micros_per_mtok: 3_750_000,
            cached_input_micros_per_mtok: Some(75_000),
        }),
        source: "https://ai.google.dev/gemini-api/docs/pricing",
    },
    ModelInfo {
        provider: "google",
        model_prefix: "gemini-3.7-flash",
        context_window: Some(1_048_576),
        max_output: Some(65_536),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 750_000,
            output_micros_per_mtok: 3_750_000,
            cached_input_micros_per_mtok: Some(75_000),
        }),
        source: "https://ai.google.dev/gemini-api/docs/pricing",
    },
    ModelInfo {
        provider: "google",
        model_prefix: "gemini-3.6-flash",
        context_window: Some(1_048_576),
        max_output: Some(65_536),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 750_000,
            output_micros_per_mtok: 3_750_000,
            cached_input_micros_per_mtok: Some(75_000),
        }),
        source: "https://ai.google.dev/gemini-api/docs/pricing",
    },
    ModelInfo {
        provider: "google",
        model_prefix: "gemini-3.5-flash",
        context_window: Some(1_048_576),
        max_output: Some(65_536),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 1_500_000,
            output_micros_per_mtok: 9_000_000,
            cached_input_micros_per_mtok: Some(150_000),
        }),
        source: "https://ai.google.dev/gemini-api/docs/pricing",
    },
    ModelInfo {
        provider: "google",
        model_prefix: "gemini-3.5-flash-lite",
        context_window: Some(1_048_576),
        max_output: Some(65_536),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 300_000,
            output_micros_per_mtok: 2_500_000,
            cached_input_micros_per_mtok: Some(30_000),
        }),
        source: "https://ai.google.dev/gemini-api/docs/pricing",
    },
    ModelInfo {
        provider: "google",
        model_prefix: "gemini-3.1-flash-lite",
        context_window: Some(1_048_576),
        max_output: Some(65_536),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 250_000,
            output_micros_per_mtok: 1_500_000,
            cached_input_micros_per_mtok: Some(25_000),
        }),
        source: "https://ai.google.dev/gemini-api/docs/pricing",
    },
    ModelInfo {
        provider: "google",
        model_prefix: "gemini-3.1-pro-preview",
        context_window: Some(1_048_576),
        max_output: Some(65_536),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 2_000_000,
            output_micros_per_mtok: 12_000_000,
            cached_input_micros_per_mtok: Some(200_000),
        }),
        source: "https://ai.google.dev/gemini-api/docs/pricing",
    },
    ModelInfo {
        provider: "google",
        model_prefix: "gemini-3-flash-preview",
        context_window: Some(1_048_576),
        max_output: Some(65_536),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 500_000,
            output_micros_per_mtok: 3_000_000,
            cached_input_micros_per_mtok: Some(50_000),
        }),
        source: "https://ai.google.dev/gemini-api/docs/pricing",
    },
    ModelInfo {
        provider: "google",
        model_prefix: "gemini-2.5-pro",
        context_window: Some(1_048_576),
        max_output: Some(65_536),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 1_250_000,
            output_micros_per_mtok: 10_000_000,
            cached_input_micros_per_mtok: Some(125_000),
        }),
        source: "https://ai.google.dev/gemini-api/docs/pricing",
    },
    ModelInfo {
        provider: "google",
        model_prefix: "gemini-2.5-flash",
        context_window: Some(1_048_576),
        max_output: Some(65_536),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 300_000,
            output_micros_per_mtok: 2_500_000,
            cached_input_micros_per_mtok: Some(30_000),
        }),
        source: "https://ai.google.dev/gemini-api/docs/pricing",
    },
    ModelInfo {
        provider: "google",
        model_prefix: "gemini-2.5-flash-lite",
        context_window: Some(1_048_576),
        max_output: Some(65_536),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 100_000,
            output_micros_per_mtok: 400_000,
            cached_input_micros_per_mtok: Some(10_000),
        }),
        source: "https://ai.google.dev/gemini-api/docs/pricing",
    },
    // deepseek (2 rows)
    ModelInfo {
        provider: "deepseek",
        model_prefix: "deepseek-flash",
        context_window: Some(1_000_000),
        max_output: Some(384_000),
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 300_000,
            output_micros_per_mtok: 1_200_000,
            cached_input_micros_per_mtok: Some(6_000),
        }),
        source: "https://api-docs.deepseek.com/quick_start/pricing",
    },
    ModelInfo {
        provider: "deepseek",
        model_prefix: "deepseek-v4-pro",
        context_window: Some(1_000_000),
        max_output: Some(384_000),
        vision: false,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 1_320_000,
            output_micros_per_mtok: 3_960_000,
            cached_input_micros_per_mtok: Some(44_000),
        }),
        source: "https://api-docs.deepseek.com/quick_start/pricing",
    },
    // xai (6 rows)
    ModelInfo {
        provider: "xai",
        model_prefix: "grok-4.7",
        context_window: Some(500_000),
        max_output: None,
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 2_000_000,
            output_micros_per_mtok: 6_000_000,
            cached_input_micros_per_mtok: Some(500_000),
        }),
        source: "https://docs.x.ai/developers/pricing",
    },
    ModelInfo {
        provider: "xai",
        model_prefix: "grok-4.6",
        context_window: Some(500_000),
        max_output: None,
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 2_000_000,
            output_micros_per_mtok: 6_000_000,
            cached_input_micros_per_mtok: Some(500_000),
        }),
        source: "https://docs.x.ai/developers/pricing",
    },
    ModelInfo {
        provider: "xai",
        model_prefix: "grok-4.5",
        context_window: Some(500_000),
        max_output: None,
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 2_000_000,
            output_micros_per_mtok: 6_000_000,
            cached_input_micros_per_mtok: Some(300_000),
        }),
        source: "https://docs.x.ai/developers/pricing",
    },
    ModelInfo {
        provider: "xai",
        model_prefix: "grok-4.3",
        context_window: Some(1_000_000),
        max_output: None,
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 1_250_000,
            output_micros_per_mtok: 2_500_000,
            cached_input_micros_per_mtok: Some(200_000),
        }),
        source: "https://docs.x.ai/developers/pricing",
    },
    ModelInfo {
        provider: "xai",
        model_prefix: "grok-4.20",
        context_window: Some(1_000_000),
        max_output: None,
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 1_250_000,
            output_micros_per_mtok: 2_500_000,
            cached_input_micros_per_mtok: Some(200_000),
        }),
        source: "https://docs.x.ai/developers/pricing",
    },
    ModelInfo {
        provider: "xai",
        model_prefix: "grok-build-0.1",
        context_window: Some(256_000),
        max_output: None,
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 1_000_000,
            output_micros_per_mtok: 2_000_000,
            cached_input_micros_per_mtok: Some(200_000),
        }),
        source: "https://docs.x.ai/developers/pricing",
    },
    // mistral (9 rows)
    ModelInfo {
        provider: "mistral",
        model_prefix: "mistral-medium-3-5",
        context_window: Some(256_000),
        max_output: None,
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 1_500_000,
            output_micros_per_mtok: 7_500_000,
            cached_input_micros_per_mtok: Some(150_000),
        }),
        source: "https://docs.mistral.ai/inference/pricing",
    },
    ModelInfo {
        provider: "mistral",
        model_prefix: "mistral-large-2512",
        context_window: Some(256_000),
        max_output: None,
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 500_000,
            output_micros_per_mtok: 1_500_000,
            cached_input_micros_per_mtok: Some(50_000),
        }),
        source: "https://docs.mistral.ai/inference/pricing",
    },
    ModelInfo {
        provider: "mistral",
        model_prefix: "mistral-small-2603",
        context_window: Some(256_000),
        max_output: None,
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 150_000,
            output_micros_per_mtok: 600_000,
            cached_input_micros_per_mtok: Some(15_000),
        }),
        source: "https://docs.mistral.ai/inference/pricing",
    },
    ModelInfo {
        provider: "mistral",
        model_prefix: "ministral-14b-2512",
        context_window: Some(256_000),
        max_output: None,
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 200_000,
            output_micros_per_mtok: 200_000,
            cached_input_micros_per_mtok: Some(20_000),
        }),
        source: "https://docs.mistral.ai/inference/pricing",
    },
    ModelInfo {
        provider: "mistral",
        model_prefix: "ministral-8b-2512",
        context_window: Some(256_000),
        max_output: None,
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 150_000,
            output_micros_per_mtok: 150_000,
            cached_input_micros_per_mtok: Some(15_000),
        }),
        source: "https://docs.mistral.ai/inference/pricing",
    },
    ModelInfo {
        provider: "mistral",
        model_prefix: "ministral-3b-2512",
        context_window: Some(256_000),
        max_output: None,
        vision: true,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 100_000,
            output_micros_per_mtok: 100_000,
            cached_input_micros_per_mtok: Some(10_000),
        }),
        source: "https://docs.mistral.ai/inference/pricing",
    },
    ModelInfo {
        provider: "mistral",
        model_prefix: "codestral-2508",
        context_window: Some(128_000),
        max_output: None,
        vision: false,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 300_000,
            output_micros_per_mtok: 900_000,
            cached_input_micros_per_mtok: Some(30_000),
        }),
        source: "https://docs.mistral.ai/inference/pricing",
    },
    ModelInfo {
        provider: "mistral",
        model_prefix: "zai-glm-5-3",
        context_window: Some(1_000_000),
        max_output: None,
        vision: false,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 1_400_000,
            output_micros_per_mtok: 4_400_000,
            cached_input_micros_per_mtok: Some(140_000),
        }),
        source: "https://docs.mistral.ai/inference/pricing",
    },
    ModelInfo {
        provider: "mistral",
        model_prefix: "zai-glm-5-2",
        context_window: Some(1_000_000),
        max_output: None,
        vision: false,
        pricing: Some(ModelPricing {
            input_micros_per_mtok: 1_400_000,
            output_micros_per_mtok: 4_400_000,
            cached_input_micros_per_mtok: Some(140_000),
        }),
        source: "https://docs.mistral.ai/inference/pricing",
    },
];

/// Exact-match aliases transcribed from `docs/superpowers/plans/data/2026-09-23-model-table.md`,
/// including aliases stated only in that file's Notes (for example `grok-build-latest`, which the
/// Notes state is an alias of `grok-4.5`, not of the similarly-named `grok-build-0.1` row).
/// `(provider, alias id [lowercase, trimmed], canonical model_prefix)`.
static MODEL_ALIASES: &[(&str, &str, &str)] = &[
    // xai
    ("xai", "grok-4.5-latest", "grok-4.5"),
    ("xai", "grok-build-latest", "grok-4.5"),
    ("xai", "grok-4.3-latest", "grok-4.3"),
    ("xai", "grok-code-fast-1", "grok-build-0.1"),
    ("xai", "grok-code-fast", "grok-build-0.1"),
    ("xai", "grok-code-fast-1-0825", "grok-build-0.1"),
    // mistral
    ("mistral", "mistral-medium-3", "mistral-medium-3-5"),
    ("mistral", "mistral-medium-latest", "mistral-medium-3-5"),
    ("mistral", "mistral-large-latest", "mistral-large-2512"),
    ("mistral", "mistral-small-latest", "mistral-small-2603"),
    ("mistral", "ministral-14b-latest", "ministral-14b-2512"),
    ("mistral", "ministral-8b-latest", "ministral-8b-2512"),
    ("mistral", "ministral-3b-latest", "ministral-3b-2512"),
    ("mistral", "codestral-latest", "codestral-2508"),
    ("mistral", "zai-glm-5", "zai-glm-5-3"),
    ("mistral", "zai-glm-latest", "zai-glm-5-3"),
    // deepseek
    ("deepseek", "deepseek-v4-flash", "deepseek-flash"),
    ("deepseek", "deepseek-v4-flash-vision-exp", "deepseek-flash"),
];

pub fn model_table() -> &'static [ModelInfo] {
    MODELS
}

pub fn model_info(provider: &str, model: &str) -> Option<&'static ModelInfo> {
    let provider = canonical_provider(provider)?;
    // The ChatGPT subscription runs OpenAI models under the hood; look up context/vision
    // metadata under the `openai` rows even though cost estimation (a separate call) still
    // treats `chatgpt` as a no-per-token subscription.
    let lookup_provider = if provider == "chatgpt" { "openai" } else { provider };
    model_info_in(MODELS, lookup_provider, model)
}

fn model_info_in<'a>(table: &'a [ModelInfo], provider: &str, model: &str) -> Option<&'a ModelInfo> {
    let model = model.trim().to_ascii_lowercase();
    if let Some(canonical) = MODEL_ALIASES.iter().find_map(|(alias_provider, alias, canonical)| {
        (*alias_provider == provider && *alias == model).then_some(*canonical)
    }) {
        if let Some(info) = table
            .iter()
            .find(|info| info.provider == provider && info.model_prefix == canonical)
        {
            return Some(info);
        }
    }
    table
        .iter()
        .filter(|info| info.provider == provider && prefix_matches(&model, info.model_prefix))
        .max_by_key(|info| info.model_prefix.len())
}

/// A prefix matches only when `model` equals it exactly or the next character after the
/// prefix is a `-`, `@`, or `:` boundary. Without this, an unlisted id that merely shares
/// a numeric prefix (`gpt-5.7`, no row) would silently price as the shorter `gpt-5` row.
fn prefix_matches(model: &str, prefix: &str) -> bool {
    match model.strip_prefix(prefix) {
        Some(rest) => rest.is_empty() || matches!(rest.as_bytes()[0], b'-' | b'@' | b':'),
        None => false,
    }
}

pub fn estimate_cost_micros(provider: &str, model: &str, usage: &TokenUsage) -> CostEstimate {
    let Some(provider) = canonical_provider(provider) else {
        return CostEstimate::Unknown;
    };
    if provider == "chatgpt" {
        return CostEstimate::Subscription;
    }
    if FREE_PROVIDERS.contains(&provider) {
        return CostEstimate::Free;
    }
    match model_info_in(MODELS, provider, model).and_then(|info| info.pricing) {
        Some(pricing) => CostEstimate::Priced {
            micros: price_usage(&pricing, usage),
        },
        None => CostEstimate::Unknown,
    }
}

/// Prices usage in micro-USD, rounding half up.
pub fn price_usage(pricing: &ModelPricing, usage: &TokenUsage) -> u64 {
    let cached = usage.cached_prompt_tokens.min(usage.prompt_tokens);
    let uncached = usage.prompt_tokens - cached;
    let cached_rate = pricing
        .cached_input_micros_per_mtok
        .unwrap_or(pricing.input_micros_per_mtok);
    let total = u128::from(uncached) * u128::from(pricing.input_micros_per_mtok)
        + u128::from(cached) * u128::from(cached_rate)
        + u128::from(usage.completion_tokens) * u128::from(pricing.output_micros_per_mtok);
    u64::try_from((total + 500_000) / 1_000_000).unwrap_or(u64::MAX)
}

fn canonical_provider(provider: &str) -> Option<&'static str> {
    let requested = provider.trim().to_ascii_lowercase();
    if requested == "chatgpt" {
        return Some("chatgpt");
    }
    resolve_provider(&requested).map(|entry| entry.definition.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_definitions;
    use std::collections::BTreeSet;

    const PRICED: ModelPricing = ModelPricing {
        input_micros_per_mtok: 2_000_000,
        output_micros_per_mtok: 10_000_000,
        cached_input_micros_per_mtok: Some(200_000),
    };

    fn row(prefix: &'static str) -> ModelInfo {
        ModelInfo {
            provider: "openai",
            model_prefix: prefix,
            context_window: Some(100_000),
            max_output: Some(10_000),
            vision: true,
            pricing: Some(PRICED),
            source: "https://example.com",
        }
    }

    #[test]
    fn longest_matching_prefix_wins() {
        let table = [row("o3"), row("o3-pro")];

        assert_eq!(
            model_info_in(&table, "openai", "o3-pro-2025-06-10")
                .unwrap()
                .model_prefix,
            "o3-pro"
        );
        assert_eq!(
            model_info_in(&table, "openai", "O3-2025-04-16")
                .unwrap()
                .model_prefix,
            "o3"
        );
        assert!(model_info_in(&table, "openai", "gpt-4o").is_none());
        assert!(model_info_in(&table, "anthropic", "o3").is_none());
    }

    #[test]
    fn cost_uses_the_cached_rate_and_rounds_half_up() {
        let usage = TokenUsage {
            prompt_tokens: 1_000,
            completion_tokens: 500,
            total_tokens: 1_500,
            cached_prompt_tokens: 400,
            reasoning_tokens: 100,
        };

        // 600 × 2 + 400 × 0.2 + 500 × 10 micro-USD
        assert_eq!(price_usage(&PRICED, &usage), 6_280);

        let without_cache_price = ModelPricing {
            cached_input_micros_per_mtok: None,
            ..PRICED
        };
        assert_eq!(price_usage(&without_cache_price, &usage), 7_000);

        let tiny = TokenUsage {
            prompt_tokens: 1,
            total_tokens: 1,
            ..TokenUsage::default()
        };
        assert_eq!(price_usage(&PRICED, &tiny), 2);
    }

    #[test]
    fn local_providers_are_free_chatgpt_is_subscription_and_unknowns_are_unknown() {
        let usage = TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 10,
            total_tokens: 20,
            ..TokenUsage::default()
        };

        assert_eq!(
            estimate_cost_micros("ollama", "llama3.2", &usage),
            CostEstimate::Free
        );
        assert_eq!(
            estimate_cost_micros("vllm", "anything", &usage),
            CostEstimate::Free
        );
        assert_eq!(
            estimate_cost_micros("chatgpt", "gpt-5.5", &usage),
            CostEstimate::Subscription
        );
        assert_eq!(
            estimate_cost_micros("openai", "not-a-real-model", &usage),
            CostEstimate::Unknown
        );
        assert_eq!(
            estimate_cost_micros("no-such-provider", "x", &usage),
            CostEstimate::Unknown
        );
    }

    #[test]
    fn every_row_is_well_formed_unique_and_sourced() {
        let known: BTreeSet<&str> = provider_definitions()
            .iter()
            .map(|definition| definition.id)
            .collect();
        let mut seen = BTreeSet::new();

        assert!(
            !model_table().is_empty(),
            "the built-in table must not be empty"
        );
        for info in model_table() {
            assert!(
                known.contains(info.provider),
                "{} is not a catalog provider",
                info.provider
            );
            assert_eq!(info.model_prefix, info.model_prefix.to_ascii_lowercase());
            if let Some(context_window) = info.context_window {
                assert!(context_window > 0, "{}", info.model_prefix);
            }
            if let (Some(max_output), Some(context_window)) = (info.max_output, info.context_window)
            {
                assert!(max_output <= context_window, "{}", info.model_prefix);
            }
            assert!(info.source.starts_with("https://"), "{}", info.model_prefix);
            if let Some(pricing) = info.pricing {
                assert!(
                    pricing.input_micros_per_mtok > 0 && pricing.output_micros_per_mtok > 0,
                    "{}",
                    info.model_prefix
                );
            }
            assert!(
                seen.insert((info.provider, info.model_prefix)),
                "duplicate {}",
                info.model_prefix
            );
        }
    }

    #[test]
    fn table_resolves_representative_models_through_aliases() {
        let sonnet = model_info("anthropic", "claude-sonnet-5").expect("sonnet 5 row");
        assert_eq!(sonnet.context_window, Some(1_000_000));
        assert_eq!(
            sonnet.pricing,
            Some(ModelPricing {
                input_micros_per_mtok: 2_000_000,
                output_micros_per_mtok: 10_000_000,
                cached_input_micros_per_mtok: Some(200_000),
            })
        );
        assert_eq!(
            model_info("anthropic", "claude-opus-5-5")
                .unwrap()
                .model_prefix,
            "claude-opus-5-5"
        );
        assert_eq!(
            model_info("anthropic", "claude-opus-5")
                .unwrap()
                .model_prefix,
            "claude-opus-5"
        );
        assert_eq!(
            model_info("openai", "gpt-5.5-pro").unwrap().model_prefix,
            "gpt-5.5-pro"
        );
        assert_eq!(
            model_info("openai", "gpt-5.5-2026-04-23")
                .unwrap()
                .model_prefix,
            "gpt-5.5"
        );
        // `gemini` is a catalog alias of `google`.
        assert_eq!(
            model_info("gemini", "gemini-2.5-flash").map(|info| info.provider),
            Some("google")
        );
    }

    #[test]
    fn priced_variants_do_not_fall_back_to_a_cheaper_family_row() {
        assert_eq!(
            model_info("openai", "gpt-5.6-cyber-2026-09-01")
                .unwrap()
                .model_prefix,
            "gpt-5.6-cyber"
        );
        assert_eq!(
            model_info("openai", "gpt-5.3-codex").unwrap().model_prefix,
            "gpt-5.3-codex"
        );

        let usage = TokenUsage {
            prompt_tokens: 1_000_000,
            completion_tokens: 0,
            total_tokens: 1_000_000,
            ..TokenUsage::default()
        };
        assert_eq!(
            estimate_cost_micros("openai", "gpt-5.5-cyber", &usage),
            CostEstimate::Priced { micros: 12_500_000 }
        );
    }

    #[test]
    fn every_listed_alias_resolves_to_its_canonical_row() {
        for (provider, alias, canonical) in MODEL_ALIASES {
            let row = model_table()
                .iter()
                .find(|info| info.provider == *provider && info.model_prefix == *canonical)
                .unwrap_or_else(|| {
                    panic!("alias {alias} for {provider} names missing canonical row {canonical}")
                });
            let resolved = model_info(provider, alias)
                .unwrap_or_else(|| panic!("alias {alias} for {provider} did not resolve"));
            assert_eq!(resolved.model_prefix, row.model_prefix, "alias {alias}");
            assert_eq!(resolved.provider, row.provider, "alias {alias}");
        }
    }

    #[test]
    fn grok_build_latest_resolves_to_grok_4_5_not_grok_build_0_1() {
        assert_eq!(
            model_info("xai", "grok-build-latest").unwrap().model_prefix,
            "grok-4.5"
        );
        assert_eq!(
            model_info("xai", "grok-build-0.1").unwrap().model_prefix,
            "grok-build-0.1"
        );
    }

    #[test]
    fn chatgpt_model_info_looks_up_the_openai_row_but_still_prices_as_subscription() {
        let info = model_info("chatgpt", "gpt-5.5").expect("chatgpt should resolve openai rows");
        assert_eq!(info.provider, "openai");
        assert_eq!(info.model_prefix, "gpt-5.5");

        let usage = TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 10,
            total_tokens: 20,
            ..TokenUsage::default()
        };
        assert_eq!(
            estimate_cost_micros("chatgpt", "gpt-5.5", &usage),
            CostEstimate::Subscription
        );
    }

    #[test]
    fn prefix_match_requires_a_boundary_character() {
        assert!(model_info("openai", "gpt-5.7").is_none());
        assert!(model_info("openai", "gpt-5.6").is_none());
        assert_eq!(
            model_info("openai", "gpt-5.5-2026-04-23")
                .unwrap()
                .model_prefix,
            "gpt-5.5"
        );
        assert_eq!(
            model_info("anthropic", "claude-haiku-4-5-20251001")
                .unwrap()
                .model_prefix,
            "claude-haiku-4-5"
        );
        assert_eq!(
            model_info("openai", "o3-2025-04-16").unwrap().model_prefix,
            "o3"
        );
    }
}
