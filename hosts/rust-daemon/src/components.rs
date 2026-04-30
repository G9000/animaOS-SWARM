mod evaluators;
mod providers;

use std::sync::Arc;

use anima_core::{Evaluator, Provider};

use self::{evaluators::ReflectionMemoryEvaluator, providers::RecentMemoriesProvider};
use crate::state::SharedMemoryStore;

pub(crate) fn default_providers(memory: SharedMemoryStore) -> Vec<Arc<dyn Provider>> {
    vec![Arc::new(RecentMemoriesProvider { memory })]
}

pub(crate) fn default_evaluators(memory: SharedMemoryStore) -> Vec<Arc<dyn Evaluator>> {
    vec![Arc::new(ReflectionMemoryEvaluator { memory })]
}
