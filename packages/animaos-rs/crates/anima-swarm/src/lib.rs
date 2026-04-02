pub mod message_bus;
pub mod types;

pub use message_bus::MessageBus;
#[derive(Default)]
pub struct SwarmCoordinator;
pub use types::{
    AgentMessage, StrategyContext, SwarmConfig, SwarmMessageBus, SwarmState, SwarmStatus,
    SwarmStrategy,
};

impl SwarmCoordinator {
    pub fn new() -> Self {
        Self
    }
}
