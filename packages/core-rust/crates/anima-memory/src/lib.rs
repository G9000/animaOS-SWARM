mod bm25;
pub mod core_port;
mod eval_harness;
#[cfg(feature = "locomo-eval")]
mod locomo_eval;
mod memory_manager;
mod vector_index;

pub use bm25::{
    QueryExpander, QueryExpansionContext, QueryExpansionRule, SearchResult, TextAnalysisProfile,
    TextAnalyzer, BM25,
};
pub use core_port::{
    core_memory_to_legacy, legacy_memory_to_core, legacy_new_memory_to_core,
    memory_manager_port_unavailable, LegacyBridgeError, LegacyBridgeErrorCode, LegacyMemoryContext,
};
pub use eval_harness::{
    baseline_memory_eval_cases, run_memory_eval_cases, run_memory_eval_checks, MemoryEvalCase,
    MemoryEvalCaseResult, MemoryEvalCheck, MemoryEvalCheckResult, MemoryEvalRelationshipSeed,
    MemoryEvalReport, MemoryEvalVectorHitSeed,
};
#[cfg(feature = "locomo-eval")]
pub use locomo_eval::{
    locomo_query_expander, locomo_smoke_eval_cases, run_locomo_eval_cases, LocomoEvalCase,
    LocomoEvalCaseResult, LocomoEvalReport, LocomoQuestion, LocomoQuestionResult,
    LocomoRelationshipSeed, LocomoRequiredSignal, LocomoVectorHitSeed,
};
pub use memory_manager::{
    AgentRelationship, AgentRelationshipOptions, Memory, MemoryEntity, MemoryEntityOptions,
    MemoryError, MemoryEvaluation, MemoryEvaluationDecision, MemoryEvaluationOptions,
    MemoryEvaluationOutcome, MemoryEvidenceTrace, MemoryImportanceAdjustment, MemoryManager,
    MemoryManagerSnapshot, MemoryRecallOptions, MemoryRecallResult, MemoryRecallWeights,
    MemoryRetentionPolicy, MemoryRetentionReport, MemoryScope, MemorySearchOptions,
    MemorySearchResult, MemoryType, MemoryVectorIndex, NewAgentRelationship, NewMemory,
    NewMemoryEntity, NewTemporalFact, NewTemporalRelationship, RecentMemoryOptions,
    RelationshipEndpointKind, TemporalFact, TemporalFactOptions, TemporalRecordStatus,
    TemporalRelationship, TemporalRelationshipOptions, VectorMemoryHit,
};
pub use vector_index::{InMemoryVectorIndex, MemoryTextEmbedder, MemoryVectorError};
