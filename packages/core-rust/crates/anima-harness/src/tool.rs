use std::collections::BTreeMap;
use std::sync::Arc;

use anima_core::{Content, DataValue, ToolDescriptor};
use async_trait::async_trait;

/// A tool the harness exposes to the model.
///
/// Implement `descriptor` to advertise the tool to the model (name, description,
/// JSON schema for the arguments) and `execute` to run it with the
/// model-supplied arguments.
#[async_trait]
pub trait HarnessTool: Send + Sync {
    /// Descriptor advertised to the model via `AgentConfig::tools`.
    fn descriptor(&self) -> ToolDescriptor;

    /// Execute the tool with the model-supplied arguments.
    async fn execute(&self, args: BTreeMap<String, DataValue>) -> Result<Content, String>;
}

/// Registry of tools available to a [`Harness`](crate::Harness).
#[derive(Clone, Default)]
pub struct ToolSet {
    tools: Vec<Arc<dyn HarnessTool>>,
}

impl ToolSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder-style registration.
    pub fn with_tool(mut self, tool: impl HarnessTool + 'static) -> Self {
        self.register(tool);
        self
    }

    pub fn register(&mut self, tool: impl HarnessTool + 'static) {
        self.tools.push(Arc::new(tool));
    }

    pub fn register_shared(&mut self, tool: Arc<dyn HarnessTool>) {
        self.tools.push(tool);
    }

    /// Append all tools from another set.
    pub fn extend(&mut self, other: ToolSet) {
        self.tools.extend(other.tools);
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Descriptors for `AgentConfig::tools`, in registration order.
    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.tools.iter().map(|tool| tool.descriptor()).collect()
    }

    /// Dispatch a model tool call to the matching tool.
    pub async fn execute(
        &self,
        name: &str,
        args: BTreeMap<String, DataValue>,
    ) -> Result<Content, String> {
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.descriptor().name == name)
            .ok_or_else(|| format!("Unknown tool: {name}"))?;
        tool.execute(args).await
    }
}
