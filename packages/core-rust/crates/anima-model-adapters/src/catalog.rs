use crate::ProviderDefinition;

#[derive(Clone, Copy)]
pub(crate) enum ProviderKind {
    Anthropic,
    Google,
    OpenAiCompatible,
}

pub(crate) struct CatalogEntry {
    pub definition: ProviderDefinition,
    pub kind: ProviderKind,
}

macro_rules! provider {
    ($id:literal, $label:literal, $aliases:expr, $kind:ident, $key:expr, $key_envs:expr, $base_envs:expr, $url:literal) => {
        CatalogEntry {
            definition: ProviderDefinition {
                id: $id,
                label: $label,
                aliases: $aliases,
                requires_key: $key,
                api_key_envs: $key_envs,
                base_url_envs: $base_envs,
                default_base_url: $url,
            },
            kind: ProviderKind::$kind,
        }
    };
}

const PROVIDERS: &[CatalogEntry] = &[
    provider!(
        "openai",
        "OpenAI",
        &[],
        OpenAiCompatible,
        true,
        &["OPENAI_API_KEY", "OPENAI_KEY", "OPENAI_TOKEN"],
        &["OPENAI_BASE_URL"],
        "https://api.openai.com/v1"
    ),
    provider!(
        "anthropic",
        "Anthropic",
        &[],
        Anthropic,
        true,
        &[
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_KEY",
            "ANTHROPIC_TOKEN",
            "CLAUDE_API_KEY"
        ],
        &["ANTHROPIC_BASE_URL"],
        "https://api.anthropic.com"
    ),
    provider!(
        "google",
        "Google (Gemini)",
        &["gemini"],
        Google,
        true,
        &[
            "GOOGLE_API_KEY",
            "GOOGLE_KEY",
            "GOOGLE_AI_KEY",
            "GEMINI_API_KEY",
            "GOOGLE_GENERATIVE_AI_API_KEY"
        ],
        &["GOOGLE_BASE_URL"],
        "https://generativelanguage.googleapis.com"
    ),
    provider!(
        "ollama",
        "Ollama (local)",
        &[],
        OpenAiCompatible,
        false,
        &["OLLAMA_API_KEY"],
        &["OLLAMA_BASE_URL"],
        "http://127.0.0.1:11434/v1"
    ),
    provider!(
        "groq",
        "Groq",
        &[],
        OpenAiCompatible,
        true,
        &["GROQ_API_KEY", "GROQ_KEY", "GROQ_TOKEN"],
        &["GROQ_BASE_URL"],
        "https://api.groq.com/openai/v1"
    ),
    provider!(
        "xai",
        "xAI (Grok)",
        &["grok"],
        OpenAiCompatible,
        true,
        &["XAI_API_KEY", "XAI_KEY", "GROK_API_KEY"],
        &["XAI_BASE_URL"],
        "https://api.x.ai/v1"
    ),
    provider!(
        "openrouter",
        "OpenRouter",
        &[],
        OpenAiCompatible,
        true,
        &["OPENROUTER_API_KEY", "OPENROUTER_KEY", "OPENROUTER_TOKEN"],
        &["OPENROUTER_BASE_URL"],
        "https://openrouter.ai/api/v1"
    ),
    provider!(
        "mistral",
        "Mistral",
        &[],
        OpenAiCompatible,
        true,
        &["MISTRAL_API_KEY", "MISTRAL_KEY", "MISTRAL_TOKEN"],
        &["MISTRAL_BASE_URL"],
        "https://api.mistral.ai/v1"
    ),
    provider!(
        "together",
        "Together AI",
        &[],
        OpenAiCompatible,
        true,
        &["TOGETHER_API_KEY", "TOGETHER_KEY", "TOGETHER_TOKEN"],
        &["TOGETHER_BASE_URL"],
        "https://api.together.xyz/v1"
    ),
    provider!(
        "deepseek",
        "DeepSeek",
        &[],
        OpenAiCompatible,
        true,
        &["DEEPSEEK_API_KEY"],
        &["DEEPSEEK_BASE_URL"],
        "https://api.deepseek.com/v1"
    ),
    provider!(
        "fireworks",
        "Fireworks",
        &[],
        OpenAiCompatible,
        true,
        &["FIREWORKS_API_KEY"],
        &["FIREWORKS_BASE_URL"],
        "https://api.fireworks.ai/inference/v1"
    ),
    provider!(
        "perplexity",
        "Perplexity",
        &[],
        OpenAiCompatible,
        true,
        &["PERPLEXITY_API_KEY"],
        &["PERPLEXITY_BASE_URL"],
        "https://api.perplexity.ai"
    ),
    provider!(
        "moonshot",
        "Moonshot (Kimi)",
        &["kimi"],
        OpenAiCompatible,
        true,
        &[
            "MOONSHOT_API_KEY",
            "MOONSHOT_KEY",
            "MOONSHOT_TOKEN",
            "KIMI_API_KEY"
        ],
        &["MOONSHOT_BASE_URL", "KIMI_BASE_URL"],
        "https://api.moonshot.ai/v1"
    ),
];

pub fn provider_definitions() -> &'static [ProviderDefinition] {
    // ProviderDefinition is the prefix of CatalogEntry only conceptually, so retain a public static
    // projection rather than exposing routing implementation details.
    const DEFINITIONS: &[ProviderDefinition] = &[
        ProviderDefinition {
            id: "openai",
            label: "OpenAI",
            aliases: &[],
            requires_key: true,
            api_key_envs: &["OPENAI_API_KEY", "OPENAI_KEY", "OPENAI_TOKEN"],
            base_url_envs: &["OPENAI_BASE_URL"],
            default_base_url: "https://api.openai.com/v1",
        },
        ProviderDefinition {
            id: "anthropic",
            label: "Anthropic",
            aliases: &[],
            requires_key: true,
            api_key_envs: &[
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_KEY",
                "ANTHROPIC_TOKEN",
                "CLAUDE_API_KEY",
            ],
            base_url_envs: &["ANTHROPIC_BASE_URL"],
            default_base_url: "https://api.anthropic.com",
        },
        ProviderDefinition {
            id: "google",
            label: "Google (Gemini)",
            aliases: &["gemini"],
            requires_key: true,
            api_key_envs: &[
                "GOOGLE_API_KEY",
                "GOOGLE_KEY",
                "GOOGLE_AI_KEY",
                "GEMINI_API_KEY",
                "GOOGLE_GENERATIVE_AI_API_KEY",
            ],
            base_url_envs: &["GOOGLE_BASE_URL"],
            default_base_url: "https://generativelanguage.googleapis.com",
        },
        ProviderDefinition {
            id: "ollama",
            label: "Ollama (local)",
            aliases: &[],
            requires_key: false,
            api_key_envs: &["OLLAMA_API_KEY"],
            base_url_envs: &["OLLAMA_BASE_URL"],
            default_base_url: "http://127.0.0.1:11434/v1",
        },
        ProviderDefinition {
            id: "groq",
            label: "Groq",
            aliases: &[],
            requires_key: true,
            api_key_envs: &["GROQ_API_KEY", "GROQ_KEY", "GROQ_TOKEN"],
            base_url_envs: &["GROQ_BASE_URL"],
            default_base_url: "https://api.groq.com/openai/v1",
        },
        ProviderDefinition {
            id: "xai",
            label: "xAI (Grok)",
            aliases: &["grok"],
            requires_key: true,
            api_key_envs: &["XAI_API_KEY", "XAI_KEY", "GROK_API_KEY"],
            base_url_envs: &["XAI_BASE_URL"],
            default_base_url: "https://api.x.ai/v1",
        },
        ProviderDefinition {
            id: "openrouter",
            label: "OpenRouter",
            aliases: &[],
            requires_key: true,
            api_key_envs: &["OPENROUTER_API_KEY", "OPENROUTER_KEY", "OPENROUTER_TOKEN"],
            base_url_envs: &["OPENROUTER_BASE_URL"],
            default_base_url: "https://openrouter.ai/api/v1",
        },
        ProviderDefinition {
            id: "mistral",
            label: "Mistral",
            aliases: &[],
            requires_key: true,
            api_key_envs: &["MISTRAL_API_KEY", "MISTRAL_KEY", "MISTRAL_TOKEN"],
            base_url_envs: &["MISTRAL_BASE_URL"],
            default_base_url: "https://api.mistral.ai/v1",
        },
        ProviderDefinition {
            id: "together",
            label: "Together AI",
            aliases: &[],
            requires_key: true,
            api_key_envs: &["TOGETHER_API_KEY", "TOGETHER_KEY", "TOGETHER_TOKEN"],
            base_url_envs: &["TOGETHER_BASE_URL"],
            default_base_url: "https://api.together.xyz/v1",
        },
        ProviderDefinition {
            id: "deepseek",
            label: "DeepSeek",
            aliases: &[],
            requires_key: true,
            api_key_envs: &["DEEPSEEK_API_KEY"],
            base_url_envs: &["DEEPSEEK_BASE_URL"],
            default_base_url: "https://api.deepseek.com/v1",
        },
        ProviderDefinition {
            id: "fireworks",
            label: "Fireworks",
            aliases: &[],
            requires_key: true,
            api_key_envs: &["FIREWORKS_API_KEY"],
            base_url_envs: &["FIREWORKS_BASE_URL"],
            default_base_url: "https://api.fireworks.ai/inference/v1",
        },
        ProviderDefinition {
            id: "perplexity",
            label: "Perplexity",
            aliases: &[],
            requires_key: true,
            api_key_envs: &["PERPLEXITY_API_KEY"],
            base_url_envs: &["PERPLEXITY_BASE_URL"],
            default_base_url: "https://api.perplexity.ai",
        },
        ProviderDefinition {
            id: "moonshot",
            label: "Moonshot (Kimi)",
            aliases: &["kimi"],
            requires_key: true,
            api_key_envs: &[
                "MOONSHOT_API_KEY",
                "MOONSHOT_KEY",
                "MOONSHOT_TOKEN",
                "KIMI_API_KEY",
            ],
            base_url_envs: &["MOONSHOT_BASE_URL", "KIMI_BASE_URL"],
            default_base_url: "https://api.moonshot.ai/v1",
        },
    ];
    DEFINITIONS
}

pub(crate) fn resolve_provider(id: &str) -> Option<&'static CatalogEntry> {
    PROVIDERS
        .iter()
        .find(|entry| entry.definition.id == id || entry.definition.aliases.contains(&id))
}
