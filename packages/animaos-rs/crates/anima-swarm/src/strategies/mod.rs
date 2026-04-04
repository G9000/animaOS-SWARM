use std::sync::Arc;

use crate::coordinator::CoordinatorStrategyFn;
use crate::SwarmStrategy;

pub mod dynamic;
pub mod round_robin;
pub mod supervisor;

pub fn resolve_strategy(strategy: SwarmStrategy) -> Arc<CoordinatorStrategyFn> {
    match strategy {
        SwarmStrategy::Dynamic => Arc::new(dynamic::dynamic_strategy),
        SwarmStrategy::Supervisor => Arc::new(supervisor::supervisor_strategy),
        SwarmStrategy::RoundRobin => Arc::new(round_robin::round_robin_strategy),
    }
}
