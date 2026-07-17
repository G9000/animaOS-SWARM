use crate::ProviderDefinition;

#[derive(Clone, Copy)]
pub(crate) enum ProviderKind {
    Anthropic,
    Google,
    OpenAiCompatible,
}

#[derive(Clone, Copy)]
pub(crate) struct CatalogEntry {
    pub definition: &'static ProviderDefinition,
    pub kind: ProviderKind,
}

const PROVIDERS: &[ProviderDefinition] = &[
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

pub fn provider_definitions() -> &'static [ProviderDefinition] {
    PROVIDERS
}

pub(crate) fn resolve_provider(id: &str) -> Option<CatalogEntry> {
    PROVIDERS
        .iter()
        .find(|definition| definition.id == id || definition.aliases.contains(&id))
        .map(|definition| CatalogEntry {
            definition,
            kind: provider_kind(definition.id),
        })
}

fn provider_kind(id: &str) -> ProviderKind {
    match id {
        "anthropic" => ProviderKind::Anthropic,
        "google" => ProviderKind::Google,
        _ => ProviderKind::OpenAiCompatible,
    }
}
