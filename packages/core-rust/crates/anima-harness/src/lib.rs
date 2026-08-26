//! Ergonomic agent harness built on the animaOS Rust core.
//!
//! `anima-harness` composes `anima-core`'s [`AgentRuntime`] agentic loop with
//! `anima-model-adapters`' provider catalog behind a small builder API:
//!
//! ```no_run
//! use anima_harness::Harness;
//!
//! # async fn example() -> Result<(), anima_harness::HarnessError> {
//! let mut harness = Harness::builder()
//!     .provider("anthropic") // resolves ANTHROPIC_API_KEY from the environment
//!     .model("claude-sonnet-4-5")
//!     .system("You are a helpful assistant.")
//!     .build()?;
//!
//! let result = harness.run("Say hello").await;
//! println!("{:?}", result.data.map(|content| content.text));
//! # Ok(())
//! # }
//! ```

mod config;
mod harness;
mod tool;

pub use config::{HarnessBuilder, HarnessError};
pub use harness::Harness;
pub use tool::{HarnessTool, ToolSet};

pub use anima_core;
pub use anima_model_adapters;
