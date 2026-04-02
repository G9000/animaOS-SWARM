use std::sync::Arc;

use anima_core::TaskResult;

use crate::coordinator::CoordinatorStrategyFn;
use crate::SwarmStrategy;

pub mod round_robin;
pub mod supervisor;

pub fn resolve_strategy(strategy: SwarmStrategy) -> Arc<CoordinatorStrategyFn> {
    match strategy {
        SwarmStrategy::Supervisor => Arc::new(supervisor::supervisor_strategy),
        SwarmStrategy::RoundRobin => Arc::new(round_robin::round_robin_strategy),
        _ => Arc::new(|_| {
            Box::pin(async { TaskResult::error("No coordinator strategy configured", 0) })
        }),
    }
}
