//! Minimal interactive chat harness.
//!
//! Usage:
//!
//! ```bash
//! ANIMA_PROVIDER=anthropic ANIMA_MODEL=claude-sonnet-4-5 \
//!   cargo run -p anima-harness --example chat
//! ```
//!
//! The provider API key is resolved from the provider's standard env vars
//! (e.g. `ANTHROPIC_API_KEY`).

use std::collections::BTreeMap;
use std::io::Write;

use anima_harness::anima_core::{Content, DataValue, TaskStatus, ToolDescriptor};
use anima_harness::{Harness, HarnessTool};
use async_trait::async_trait;

struct CurrentTimeTool;

#[async_trait]
impl HarnessTool for CurrentTimeTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "get_current_time".to_owned(),
            description: "Get the current Unix time in milliseconds.".to_owned(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        }
    }

    async fn execute(&self, _args: BTreeMap<String, DataValue>) -> Result<Content, String> {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_millis();
        Ok(Content {
            text: millis.to_string(),
            attachments: None,
            metadata: None,
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let provider = std::env::var("ANIMA_PROVIDER").unwrap_or_else(|_| "openai".to_owned());
    let model = std::env::var("ANIMA_MODEL").map_err(|_| {
        "set ANIMA_MODEL (and the provider API key env var) to use this example".to_owned()
    })?;

    let mut harness = Harness::builder()
        .provider(provider)
        .model(model)
        .system("You are a helpful assistant.")
        .tool(CurrentTimeTool)
        .build()
        .map_err(|error| error.to_string())?;

    println!("anima-harness chat — type 'exit' to quit");
    loop {
        print!("> ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if std::io::stdin()
            .read_line(&mut line)
            .map_err(|error| error.to_string())?
            == 0
        {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "exit" {
            break;
        }

        let result = harness.chat(line).await;
        match result.status {
            TaskStatus::Success => {
                println!(
                    "{}",
                    result.data.map(|content| content.text).unwrap_or_default()
                );
            }
            TaskStatus::Error => {
                eprintln!("error: {}", result.error.unwrap_or_default());
            }
        }
    }
    Ok(())
}
