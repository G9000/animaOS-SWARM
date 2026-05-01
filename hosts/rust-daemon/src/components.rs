mod evaluators;
mod providers;

use std::sync::Arc;

use anima_core::{Evaluator, Provider};

use self::{evaluators::ReflectionMemoryEvaluator, providers::RecentMemoriesProvider};
use crate::memory_embeddings::SharedMemoryEmbeddings;
use crate::memory_store::MemoryStoreConfig;
use crate::state::SharedMemoryStore;

pub(crate) fn default_providers(memory: SharedMemoryStore) -> Vec<Arc<dyn Provider>> {
    vec![Arc::new(RecentMemoriesProvider { memory })]
}

pub(crate) fn default_evaluators(
    memory: SharedMemoryStore,
    memory_embeddings: SharedMemoryEmbeddings,
    memory_store: Option<MemoryStoreConfig>,
) -> Vec<Arc<dyn Evaluator>> {
    vec![Arc::new(ReflectionMemoryEvaluator {
        memory,
        memory_embeddings,
        memory_store,
    })]
}
