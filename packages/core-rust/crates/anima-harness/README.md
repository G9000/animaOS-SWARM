# anima-harness

An embeddable agent harness built on the animaOS Rust core. It composes
`anima-core`'s agentic loop (`AgentRuntime`) with `anima-model-adapters`' provider
catalog behind a small builder API: system prompt + tools + agentic loop + events.

```rust
use anima_harness::Harness;

# async fn example() -> Result<(), anima_harness::HarnessError> {
let mut harness = Harness::builder()
    .provider("anthropic") // resolves ANTHROPIC_API_KEY from the environment
    .model("claude-sonnet-4-5")
    .system("You are a helpful assistant.")
    .build()?;

let result = harness.run("Say hello").await;      // single-shot
let reply = harness.chat("And again?").await;     // keeps conversation history
# Ok(())
# }
```

## Tools

Implement `HarnessTool` and register it on the builder:

```rust
use std::collections::BTreeMap;
use anima_harness::anima_core::{Content, DataValue, ToolDescriptor};
use anima_harness::HarnessTool;
use async_trait::async_trait;

struct GetTime;

#[async_trait]
impl HarnessTool for GetTime {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "get_current_time".into(),
            description: "Get the current Unix time in milliseconds.".into(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        }
    }

    async fn execute(&self, _args: BTreeMap<String, DataValue>) -> Result<Content, String> {
        Ok(Content { text: "0".into(), attachments: None, metadata: None })
    }
}
```

## Credentials

Provider credentials resolve in this order: explicit `.api_key(...)` /
`.base_url(...)` builder overrides, then the provider's standard environment
variables (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, ...), then the provider's
default base URL. For tests or bespoke providers, supply any
`anima_core::ModelAdapter` via `.adapter(...)`.

## Example

```bash
ANIMA_PROVIDER=anthropic ANIMA_MODEL=claude-sonnet-4-5 \
  cargo run -p anima-harness --example chat
```
