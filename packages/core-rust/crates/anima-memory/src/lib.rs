mod bm25;
mod eval_harness;
mod memory_manager;
mod vector_index;

pub use bm25::{SearchResult, BM25};
pub use eval_harness::{
    baseline_memory_eval_cases, run_memory_eval_cases, run_memory_eval_checks, MemoryEvalCase,
    MemoryEvalCaseResult, MemoryEvalCheck, MemoryEvalCheckResult, MemoryEvalRelationshipSeed,
    MemoryEvalReport, MemoryEvalVectorHitSeed,
};
pub use memory_manager::{
    AgentRelationship, AgentRelationshipOptions, Memory, MemoryEntity, MemoryEntityOptions,
    MemoryError, MemoryEvaluation, MemoryEvaluationDecision, MemoryEvaluationOptions,
    MemoryEvaluationOutcome, MemoryEvidenceTrace, MemoryImportanceAdjustment, MemoryManager,
    MemoryRecallOptions, MemoryRecallResult, MemoryRetentionPolicy, MemoryRetentionReport,
    MemoryScope, MemorySearchOptions, MemorySearchResult, MemoryType, MemoryVectorIndex,
    NewAgentRelationship, NewMemory, NewMemoryEntity, RecentMemoryOptions,
    RelationshipEndpointKind, VectorMemoryHit,
};
pub use vector_index::{InMemoryVectorIndex, MemoryTextEmbedder, MemoryVectorError};
