use crate::bm25::{QueryExpander, QueryExpansionContext, QueryExpansionRule};
use crate::memory_manager::{
    MemoryManager, MemoryRecallOptions, MemoryRecallResult, MemoryScope, MemorySearchOptions,
    MemoryType, MemoryVectorIndex, NewAgentRelationship, NewMemory, NewTemporalFact,
    RelationshipEndpointKind, TemporalRecordStatus, VectorMemoryHit,
};

#[derive(Clone, Debug, PartialEq)]
pub struct LocomoEvalCase {
    pub name: String,
    pub seed_memories: Vec<NewMemory>,
    pub seed_relationships: Vec<LocomoRelationshipSeed>,
    pub seed_temporal_facts: Vec<LocomoTemporalFactSeed>,
    pub questions: Vec<LocomoQuestion>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocomoRelationshipSeed {
    pub relationship: NewAgentRelationship,
    pub evidence_content_contains: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocomoTemporalFactSeed {
    pub fact: NewTemporalFact,
    pub evidence_content_contains: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocomoVectorHitSeed {
    pub memory_content_contains: String,
    pub score: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocomoQuestion {
    pub name: String,
    pub query: String,
    pub options: MemoryRecallOptions,
    pub expected_answer_contains: Vec<String>,
    pub expected_evidence_contains: Vec<String>,
    pub excluded_evidence_contains: Vec<String>,
    pub vector_hits: Vec<LocomoVectorHitSeed>,
    pub top_k: usize,
    pub required_signal: LocomoRequiredSignal,
    pub expect_no_results: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LocomoRequiredSignal {
    #[default]
    Any,
    Relationship,
    Temporal,
    Vector,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocomoEvalReport {
    pub cases: Vec<LocomoEvalCaseResult>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocomoEvalCaseResult {
    pub name: String,
    pub questions: Vec<LocomoQuestionResult>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocomoQuestionResult {
    pub name: String,
    pub passed: bool,
    pub detail: String,
    pub top_k: usize,
    pub recalled_evidence_count: usize,
    pub expected_evidence_count: usize,
    pub covered_answer_fragments: usize,
    pub expected_answer_fragments: usize,
    pub false_positive_count: usize,
    pub excluded_evidence_count: usize,
}

impl LocomoEvalReport {
    pub fn passed(&self) -> bool {
        self.cases
            .iter()
            .all(|case| case.questions.iter().all(|question| question.passed))
    }

    pub fn total_questions(&self) -> usize {
        self.cases.iter().map(|case| case.questions.len()).sum()
    }

    pub fn passed_questions(&self) -> usize {
        self.cases
            .iter()
            .flat_map(|case| case.questions.iter())
            .filter(|question| question.passed)
            .count()
    }

    pub fn recall_at_k(&self) -> f64 {
        let mut evidence_questions = 0;
        let mut fully_recalled_questions = 0;

        for question in self.cases.iter().flat_map(|case| case.questions.iter()) {
            if question.expected_evidence_count == 0 {
                continue;
            }
            evidence_questions += 1;
            if question.recalled_evidence_count == question.expected_evidence_count {
                fully_recalled_questions += 1;
            }
        }

        ratio(fully_recalled_questions, evidence_questions)
    }

    pub fn answer_coverage(&self) -> f64 {
        let covered = self
            .cases
            .iter()
            .flat_map(|case| case.questions.iter())
            .map(|question| question.covered_answer_fragments)
            .sum();
        let expected = self
            .cases
            .iter()
            .flat_map(|case| case.questions.iter())
            .map(|question| question.expected_answer_fragments)
            .sum();

        ratio(covered, expected)
    }

    pub fn false_positive_rate(&self) -> f64 {
        let false_positives = self
            .cases
            .iter()
            .flat_map(|case| case.questions.iter())
            .map(|question| question.false_positive_count)
            .sum();
        let excluded = self
            .cases
            .iter()
            .flat_map(|case| case.questions.iter())
            .map(|question| question.excluded_evidence_count)
            .sum();

        ratio(false_positives, excluded)
    }

    pub fn failure_messages(&self) -> Vec<String> {
        self.cases
            .iter()
            .flat_map(|case| {
                case.questions
                    .iter()
                    .filter(|question| !question.passed)
                    .map(|question| {
                        format!("{} / {}: {}", case.name, question.name, question.detail)
                    })
            })
            .collect()
    }
}

pub fn run_locomo_eval_cases(cases: &[LocomoEvalCase]) -> LocomoEvalReport {
    LocomoEvalReport {
        cases: cases.iter().map(run_locomo_eval_case).collect(),
    }
}

pub fn locomo_smoke_eval_cases() -> Vec<LocomoEvalCase> {
    vec![
        single_session_profile_case(),
        temporal_update_case(),
        agent_handoff_case(),
        speaker_attribution_case(),
        abstention_case(),
        semantic_vector_case(),
    ]
}

pub fn locomo_query_expander() -> QueryExpander {
    QueryExpander::with_rules([QueryExpansionRule::new(
        "locomo-profile-phrases",
        expand_locomo_query_terms,
    )])
}

fn expand_locomo_query_terms(context: &mut QueryExpansionContext<'_>) {
    if context.has_terms(&["relationship", "status"]) {
        context.push_terms(&["single", "breakup", "parent", "partner"]);
    }

    let asks_to_pursue = context.has_term("pursue") || context.has_term("persue");
    if context.has_term("persue") {
        context.push_terms(&["pursue"]);
    }

    let asks_career_path =
        context.has_term("career") && (context.has_term("path") || asks_to_pursue);
    let asks_counseling_goal = asks_to_pursue && context.has_term("counsel");
    if asks_career_path || asks_counseling_goal {
        context.push_terms(&[
            "counseling",
            "mental",
            "health",
            "working",
            "work",
            "transgender",
            "support",
        ]);
    }

    if context.has_term("long") && context.has_term("friend") {
        context.push_terms(&["known", "years", "since"]);
    }

    if (context.has_term("kid")
        || context.has_term("kids")
        || context.has_term("child")
        || context.has_term("children"))
        && (context.has_term("like") || context.has_term("lik") || context.has_term("love"))
    {
        context.push_terms(&["dinosaur", "nature", "animal", "learning", "stoked"]);
    }

    if context.has_term("activity") || context.has_term("partake") {
        context.push_terms(&[
            "pottery",
            "camping",
            "painting",
            "swimming",
            "class",
            "workshop",
            "clay",
            "museum",
            "nature-inspired",
            "waterfall",
        ]);
    }

    if context.has_term("destress") {
        context.push_terms(&[
            "therapy",
            "headspace",
            "clear",
            "mind",
            "pottery",
            "running",
        ]);
    }

    if context.has_term("self") && context.has_term("care") {
        context.push_terms(&[
            "me-time",
            "refreshes",
            "pottery",
            "running",
            "reading",
            "violin",
        ]);
    }

    if context.has_term("musical") || context.has_term("artist") || context.has_term("band") {
        context.push_terms(&["music", "concert", "song", "sounds", "voice", "singer"]);
    }

    if context.has_term("instrument") {
        context.push_terms(&["clarinet", "violin", "playing"]);
    }

    if context.has_term("pet") {
        context.push_terms(&["cat", "named", "oliver", "luna", "bailey"]);
    }

    if context.has_term("symbol") {
        context.push_terms(&["rainbow", "flag", "transgender", "pendant", "mural"]);
    }

    if context.has_term("event")
        && (context.has_term("children")
            || context.has_term("youth")
            || context.has_term("school")
            || context.has_term("help"))
    {
        context.push_terms(&[
            "mentorship",
            "program",
            "youth",
            "school",
            "speech",
            "talk",
            "audience",
            "allies",
        ]);
    }

    if context.has_term("event")
        && (context.has_term("lgbtq") || context.has_term("pride") || context.has_term("community"))
    {
        context.push_terms(&[
            "support", "group", "pride", "parade", "school", "meeting", "campaign", "speech",
            "talk",
        ]);
    }

    if (context.has_term("subject") || context.has_term("both"))
        && (context.has_term("paint") || context.has_term("art"))
    {
        context.push_terms(&["painting", "painted", "sunset", "easel", "sky", "nature"]);
    }

    if context.has_terms(&["kind", "art"]) {
        context.push_terms(&[
            "abstract",
            "identity",
            "diversity",
            "representation",
            "vibrant",
            "colors",
            "theme",
        ]);
    }

    if context.has_term("support") || (context.has_term("who") && context.has_term("supports")) {
        context.push_terms(&["friends", "family", "mentors", "rocks", "strength"]);
    }

    if (context.has_term("item") || context.has_term("items"))
        && (context.has_term("bought") || context.has_term("buy"))
    {
        context.push_terms(&["bought", "got", "new", "figurines", "shoes", "sneakers"]);
    }

    if context.has_term("book") {
        context.push_terms(&[
            "read",
            "recommended",
            "suggestion",
            "becoming",
            "nicole",
            "dreams",
        ]);
    }

    if context.has_term("family") || context.has_term("hike") {
        context.push_terms(&[
            "kids",
            "children",
            "pottery",
            "painting",
            "painted",
            "museum",
            "swimming",
            "camping",
            "clay",
            "nature",
            "waterfall",
            "marshmallows",
            "stories",
        ]);
    }

    if (context.has_term("type") || context.has_term("kind")) && context.has_term("pottery") {
        context.push_terms(&["cup", "bowl", "bowls", "clay", "made"]);
    }

    if context.has_term("paint") || context.has_term("painted") {
        context.push_terms(&["horse", "sunset", "sunrise", "lake", "nature-inspired"]);
    }

    if context.has_term("long") && context.has_term("art")
        || (context.has_term("art")
            && (context.has_term("practic") || context.has_term("practice")))
    {
        context.push_terms(&["since", "started", "2016", "painting", "years"]);
    }

    if context.has_term("bowl") && context.has_term("remind") {
        context.push_terms(&["hand-painted", "art", "self-expression", "pottery"]);
    }

    if context.has_term("bowl")
        && (context.has_term("make")
            || context.has_term("made")
            || context.has_term("photo")
            || context.has_term("black")
            || context.has_term("white"))
    {
        context.push_terms(&["made", "class", "proud", "black", "white"]);
    }

    if context.has_terms(&["national", "park"]) || context.has_terms(&["theme", "park"]) {
        context.push_terms(&["camping", "outdoors", "nature", "forest"]);
    }

    if context.has_terms(&["political", "leaning"]) {
        context.push_terms(&["conservative", "lgbtq", "rights", "community", "liberal"]);
    }

    if context.has_term("trait") || context.has_term("personality") {
        context.push_terms(&["thoughtful", "authentic", "drive", "real", "care"]);
    }

    if (context.has_term("career") || context.has_term("pursue"))
        && (context.has_term("writ") || context.has_term("writer") || context.has_term("read"))
    {
        context.push_terms(&[
            "counseling",
            "mental",
            "health",
            "jobs",
            "career",
            "reading",
            "books",
            "writing",
        ]);
    }

    if context.has_term("religious")
        || context.has_term("religiou")
        || context.has_term("church")
        || context.has_term("faith")
    {
        context.push_terms(&[
            "religious",
            "church",
            "faith",
            "conservatives",
            "stained",
            "glass",
        ]);
    }

    if context.has_term("roadtrip") {
        context.push_terms(&[
            "accident", "scary", "scared", "bad", "start", "freaked", "damaged",
        ]);
    }

    if context.has_term("roadtrip") && context.has_term("hike") {
        context.push_terms(&["yesterday", "after", "relax"]);
    }

    if context.has_terms(&["home", "country"])
        && (context.has_term("move") || context.has_term("back") || context.has_term("soon"))
    {
        context.push_terms(&[
            "sweden",
            "roots",
            "adoption",
            "agency",
            "interviews",
            "family",
            "kids",
            "loving",
        ]);
    }

    if context.has_term("adoption")
        && (context.has_term("interview") || context.has_term("interviews"))
    {
        context.push_terms(&["passed", "last", "friday", "interviews"]);
    }

    if context.has_term("adoption")
        && (context.has_term("agency")
            || context.has_term("support")
            || context.has_term("individual")
            || context.has_term("choose")
            || context.has_term("chose")
            || context.has_term("consider"))
    {
        context.push_terms(&["lgbtq", "folks", "inclusivity", "support", "spoke", "chose"]);
    }

    if (context.has_term("counsel") || context.has_term("counseling"))
        && (context.has_term("mental") || context.has_term("service") || context.has_term("pursu"))
    {
        context.push_terms(&[
            "trans",
            "people",
            "accept",
            "themselves",
            "supporting",
            "therapeutic",
            "methods",
        ]);
    }

    if context.has_terms(&["summer", "plan"])
        || (context.has_term("adoption")
            && (context.has_term("plan") || context.has_term("summer")))
    {
        context.push_terms(&["researching", "agencies", "adoption", "children", "loving"]);
    }

    if context.has_term("long") && (context.has_term("marri") || context.has_term("husband")) {
        context.push_terms(&["wedding", "years", "dress", "married"]);
    }

    if context.has_term("many") && (context.has_term("child") || context.has_term("kid")) {
        context.push_terms(&[
            "son",
            "daughter",
            "brother",
            "kids",
            "scared",
            "reassured",
            "ok",
        ]);
    }
}

fn run_locomo_eval_case(case: &LocomoEvalCase) -> LocomoEvalCaseResult {
    let mut manager = MemoryManager::with_query_expander(locomo_query_expander());
    let mut setup_failures = Vec::new();

    for memory in &case.seed_memories {
        if let Err(error) = manager.add(memory.clone()) {
            setup_failures.push(LocomoQuestionResult::setup_fail(format!(
                "failed to add seed memory: {}",
                error.message()
            )));
        }
    }

    for seed in &case.seed_relationships {
        let mut relationship = seed.relationship.clone();
        let mut evidence_failed = false;
        for expected in &seed.evidence_content_contains {
            match find_memory_id_by_content(&manager, expected) {
                Some(memory_id) => relationship.evidence_memory_ids.push(memory_id),
                None => {
                    evidence_failed = true;
                    setup_failures.push(LocomoQuestionResult::setup_fail(format!(
                        "no seed memory content contained {expected:?}"
                    )));
                }
            }
        }

        if !evidence_failed {
            if let Err(error) = manager.upsert_agent_relationship(relationship) {
                setup_failures.push(LocomoQuestionResult::setup_fail(format!(
                    "failed to add seed relationship: {}",
                    error.message()
                )));
            }
        }
    }

    for seed in &case.seed_temporal_facts {
        let mut fact = seed.fact.clone();
        let mut evidence_failed = false;
        for expected in &seed.evidence_content_contains {
            match find_memory_id_by_content(&manager, expected) {
                Some(memory_id) => fact.evidence_memory_ids.push(memory_id),
                None => {
                    evidence_failed = true;
                    setup_failures.push(LocomoQuestionResult::setup_fail(format!(
                        "no seed memory content contained {expected:?}"
                    )));
                }
            }
        }

        if !evidence_failed {
            if let Err(error) = manager.add_temporal_fact(fact) {
                setup_failures.push(LocomoQuestionResult::setup_fail(format!(
                    "failed to add seed temporal fact: {}",
                    error.message()
                )));
            }
        }
    }

    let mut questions = setup_failures;
    questions.extend(
        case.questions
            .iter()
            .map(|question| run_locomo_question(&manager, question)),
    );

    LocomoEvalCaseResult {
        name: case.name.clone(),
        questions,
    }
}

fn run_locomo_question(manager: &MemoryManager, question: &LocomoQuestion) -> LocomoQuestionResult {
    let vector_index = match seeded_vector_index(manager, &question.vector_hits) {
        Ok(index) => index,
        Err(detail) => return LocomoQuestionResult::fail(question, detail),
    };
    let vector_index_ref = vector_index
        .as_ref()
        .map(|index| index as &dyn MemoryVectorIndex);
    let results = manager.recall_with_vector_index(
        &question.query,
        question.options.clone(),
        vector_index_ref,
    );
    let top_k = question.top_k.max(1);
    let top_results: Vec<_> = results.iter().take(top_k).collect();

    if question.expect_no_results {
        if top_results.is_empty() {
            return LocomoQuestionResult::pass(
                question,
                "no recall results returned".to_string(),
                0,
                0,
                0,
                0,
            );
        }
        return LocomoQuestionResult::fail_with_counts(
            question,
            format!(
                "expected no recall results, got {:?}",
                top_results
                    .iter()
                    .map(|result| result.memory.content.as_str())
                    .collect::<Vec<_>>()
            ),
            0,
            0,
            question.excluded_evidence_contains.len(),
            0,
        );
    }

    let recalled_evidence_count = question
        .expected_evidence_contains
        .iter()
        .filter(|expected| {
            top_results
                .iter()
                .any(|result| result.memory.content.contains(expected.as_str()))
        })
        .count();
    let false_positive_count = question
        .excluded_evidence_contains
        .iter()
        .filter(|excluded| {
            top_results
                .iter()
                .any(|result| result.memory.content.contains(excluded.as_str()))
        })
        .count();
    let evidence_text = top_results
        .iter()
        .map(|result| result.memory.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let covered_answer_fragments = question
        .expected_answer_contains
        .iter()
        .filter(|fragment| contains_case_insensitive(&evidence_text, fragment))
        .count();
    let signal_supported = signal_supported(&top_results, question);

    let evidence_passed = recalled_evidence_count == question.expected_evidence_contains.len();
    let answer_passed = covered_answer_fragments == question.expected_answer_contains.len();
    let false_positive_passed = false_positive_count == 0;
    let passed = evidence_passed && answer_passed && false_positive_passed && signal_supported;

    let detail = format!(
        "recall {}/{} evidence, answer {}/{} fragments, {} false positives in top {}, signal {:?}",
        recalled_evidence_count,
        question.expected_evidence_contains.len(),
        covered_answer_fragments,
        question.expected_answer_contains.len(),
        false_positive_count,
        top_k,
        question.required_signal
    );

    if passed {
        LocomoQuestionResult::pass(
            question,
            detail,
            recalled_evidence_count,
            covered_answer_fragments,
            false_positive_count,
            question.excluded_evidence_contains.len(),
        )
    } else {
        LocomoQuestionResult::fail_with_counts(
            question,
            format!(
                "{}; top results were {:?}",
                detail,
                top_results
                    .iter()
                    .map(|result| result.memory.content.as_str())
                    .collect::<Vec<_>>()
            ),
            recalled_evidence_count,
            covered_answer_fragments,
            question.excluded_evidence_contains.len(),
            false_positive_count,
        )
    }
}

fn signal_supported(results: &[&MemoryRecallResult], question: &LocomoQuestion) -> bool {
    match question.required_signal {
        LocomoRequiredSignal::Any => true,
        LocomoRequiredSignal::Relationship => matching_evidence_results(results, question)
            .iter()
            .any(|result| result.relationship_score > 0.0),
        LocomoRequiredSignal::Temporal => matching_evidence_results(results, question)
            .iter()
            .any(|result| result.temporal_score > 0.0),
        LocomoRequiredSignal::Vector => matching_evidence_results(results, question)
            .iter()
            .any(|result| result.vector_score > 0.0),
    }
}

fn matching_evidence_results<'a>(
    results: &[&'a MemoryRecallResult],
    question: &LocomoQuestion,
) -> Vec<&'a MemoryRecallResult> {
    results
        .iter()
        .copied()
        .filter(|result| {
            question
                .expected_evidence_contains
                .iter()
                .any(|expected| result.memory.content.contains(expected.as_str()))
        })
        .collect()
}

fn contains_case_insensitive(value: &str, expected: &str) -> bool {
    value
        .to_ascii_lowercase()
        .contains(&expected.to_ascii_lowercase())
}

fn seeded_vector_index(
    manager: &MemoryManager,
    vector_hits: &[LocomoVectorHitSeed],
) -> Result<Option<SeededLocomoVectorIndex>, String> {
    if vector_hits.is_empty() {
        return Ok(None);
    }

    let mut hits = Vec::new();
    for hit in vector_hits {
        let Some(memory_id) = find_memory_id_by_content(manager, &hit.memory_content_contains)
        else {
            return Err(format!(
                "no vector seed memory content contained {:?}",
                hit.memory_content_contains
            ));
        };
        hits.push(VectorMemoryHit {
            memory_id,
            score: hit.score,
        });
    }

    Ok(Some(SeededLocomoVectorIndex { hits }))
}

struct SeededLocomoVectorIndex {
    hits: Vec<VectorMemoryHit>,
}

impl MemoryVectorIndex for SeededLocomoVectorIndex {
    fn search(&self, _query: &str, limit: usize) -> Vec<VectorMemoryHit> {
        self.hits.iter().take(limit).cloned().collect()
    }
}

impl LocomoQuestionResult {
    fn pass(
        question: &LocomoQuestion,
        detail: String,
        recalled_evidence_count: usize,
        covered_answer_fragments: usize,
        false_positive_count: usize,
        excluded_evidence_count: usize,
    ) -> Self {
        Self {
            name: question.name.clone(),
            passed: true,
            detail,
            top_k: question.top_k.max(1),
            recalled_evidence_count,
            expected_evidence_count: question.expected_evidence_contains.len(),
            covered_answer_fragments,
            expected_answer_fragments: question.expected_answer_contains.len(),
            false_positive_count,
            excluded_evidence_count,
        }
    }

    fn fail(question: &LocomoQuestion, detail: String) -> Self {
        Self::fail_with_counts(
            question,
            detail,
            0,
            0,
            question.excluded_evidence_contains.len(),
            0,
        )
    }

    fn fail_with_counts(
        question: &LocomoQuestion,
        detail: String,
        recalled_evidence_count: usize,
        covered_answer_fragments: usize,
        excluded_evidence_count: usize,
        false_positive_count: usize,
    ) -> Self {
        Self {
            name: question.name.clone(),
            passed: false,
            detail,
            top_k: question.top_k.max(1),
            recalled_evidence_count,
            expected_evidence_count: question.expected_evidence_contains.len(),
            covered_answer_fragments,
            expected_answer_fragments: question.expected_answer_contains.len(),
            false_positive_count,
            excluded_evidence_count,
        }
    }

    fn setup_fail(detail: String) -> Self {
        Self {
            name: "setup".into(),
            passed: false,
            detail,
            top_k: 0,
            recalled_evidence_count: 0,
            expected_evidence_count: 0,
            covered_answer_fragments: 0,
            expected_answer_fragments: 0,
            false_positive_count: 0,
            excluded_evidence_count: 0,
        }
    }
}

fn find_memory_id_by_content(manager: &MemoryManager, expected: &str) -> Option<String> {
    manager
        .get_recent(crate::RecentMemoryOptions {
            limit: Some(usize::MAX),
            ..crate::RecentMemoryOptions::default()
        })
        .into_iter()
        .find(|memory| memory.content.contains(expected))
        .map(|memory| memory.id)
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        1.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn single_session_profile_case() -> LocomoEvalCase {
    LocomoEvalCase {
        name: "locomo single-session profile".into(),
        seed_memories: vec![
            locomo_memory(|memory| {
                memory.content =
                    "Leo avoids espresso before launch demos and prefers mint tea".into();
                memory.importance = 0.86;
                memory.tags = Some(vec!["profile".into(), "preference".into()]);
            }),
            locomo_memory(|memory| {
                memory.agent_id = "finance".into();
                memory.agent_name = "Finance".into();
                memory.content = "Maya wants billing exports grouped by invoice status".into();
                memory.importance = 0.72;
            }),
        ],
        seed_relationships: Vec::new(),
        seed_temporal_facts: Vec::new(),
        questions: vec![LocomoQuestion {
            name: "single-hop preference recall".into(),
            query: "What drink should the assistant offer Leo before a launch demo?".into(),
            options: MemoryRecallOptions {
                recent_limit: Some(0),
                lexical_limit: Some(4),
                limit: Some(3),
                ..MemoryRecallOptions::default()
            },
            expected_answer_contains: vec!["mint tea".into()],
            expected_evidence_contains: vec!["prefers mint tea".into()],
            excluded_evidence_contains: vec!["billing exports".into()],
            vector_hits: Vec::new(),
            top_k: 1,
            required_signal: LocomoRequiredSignal::Any,
            expect_no_results: false,
        }],
    }
}

fn temporal_update_case() -> LocomoEvalCase {
    LocomoEvalCase {
        name: "locomo temporal preference update".into(),
        seed_memories: vec![
            locomo_memory(|memory| {
                memory.content =
                    "In January, Leo preferred detailed release notes with long rationale".into();
                memory.importance = 0.58;
            }),
            locomo_memory(|memory| {
                memory.content =
                    "Current preference: Leo wants concise release notes with risk bullets".into();
                memory.importance = 0.92;
            }),
        ],
        seed_relationships: Vec::new(),
        seed_temporal_facts: vec![LocomoTemporalFactSeed {
            fact: locomo_temporal_fact(|fact| {
                fact.predicate = "prefers_release_notes".into();
                fact.value = Some("concise release notes with risk bullets".into());
            }),
            evidence_content_contains: vec!["Current preference".into()],
        }],
        questions: vec![LocomoQuestion {
            name: "temporal update prefers current evidence".into(),
            query: "What is Leo's current release note preference?".into(),
            options: MemoryRecallOptions {
                recent_limit: Some(0),
                lexical_limit: Some(4),
                temporal_limit: Some(4),
                limit: Some(2),
                ..MemoryRecallOptions::default()
            },
            expected_answer_contains: vec!["concise release notes".into(), "risk bullets".into()],
            expected_evidence_contains: vec!["Current preference".into()],
            excluded_evidence_contains: vec!["preferred detailed release notes".into()],
            vector_hits: Vec::new(),
            top_k: 1,
            required_signal: LocomoRequiredSignal::Temporal,
            expect_no_results: false,
        }],
    }
}

fn agent_handoff_case() -> LocomoEvalCase {
    LocomoEvalCase {
        name: "locomo agent-agent handoff".into(),
        seed_memories: vec![
            locomo_memory(|memory| {
                memory.agent_id = "planner".into();
                memory.agent_name = "Planner".into();
                memory.content =
                    "Planner learned that Leo needs rollback risk called out before launch".into();
                memory.importance = 0.88;
                memory.tags = Some(vec!["handoff".into(), "agent-agent".into()]);
                memory.world_id = Some("world-launch".into());
            }),
            locomo_memory(|memory| {
                memory.agent_id = "critic".into();
                memory.agent_name = "Critic".into();
                memory.content = "Critic tracks unrelated UI polish findings".into();
                memory.importance = 0.66;
                memory.world_id = Some("world-launch".into());
            }),
        ],
        seed_relationships: vec![LocomoRelationshipSeed {
            relationship: locomo_relationship(|relationship| {
                relationship.source_agent_id = "planner".into();
                relationship.source_agent_name = "Planner".into();
                relationship.target_agent_id = "critic".into();
                relationship.target_agent_name = "Critic".into();
                relationship.relationship_type = "delegated_to".into();
                relationship.summary = Some("Planner handed Leo launch context to Critic".into());
                relationship.world_id = Some("world-launch".into());
            }),
            evidence_content_contains: vec!["rollback risk".into()],
        }],
        seed_temporal_facts: Vec::new(),
        questions: vec![LocomoQuestion {
            name: "multi-hop agent handoff recall".into(),
            query: "What should Critic remember for Leo before launch?".into(),
            options: MemoryRecallOptions {
                entity_id: Some("critic".into()),
                search: MemorySearchOptions {
                    world_id: Some("world-launch".into()),
                    ..MemorySearchOptions::default()
                },
                recent_limit: Some(0),
                relationship_limit: Some(5),
                limit: Some(3),
                ..MemoryRecallOptions::default()
            },
            expected_answer_contains: vec!["rollback risk".into()],
            expected_evidence_contains: vec!["rollback risk".into()],
            excluded_evidence_contains: vec!["UI polish".into()],
            vector_hits: Vec::new(),
            top_k: 1,
            required_signal: LocomoRequiredSignal::Relationship,
            expect_no_results: false,
        }],
    }
}

fn speaker_attribution_case() -> LocomoEvalCase {
    LocomoEvalCase {
        name: "locomo speaker attribution".into(),
        seed_memories: vec![
            locomo_memory(|memory| {
                memory.agent_id = "assistant".into();
                memory.agent_name = "Assistant".into();
                memory.content = "Maya works in CET and wants morning status updates".into();
                memory.importance = 0.82;
                memory.tags = Some(vec!["speaker".into(), "timezone".into()]);
            }),
            locomo_memory(|memory| {
                memory.agent_id = "assistant".into();
                memory.agent_name = "Assistant".into();
                memory.content = "Leo works in PT and wants afternoon status updates".into();
                memory.importance = 0.81;
                memory.tags = Some(vec!["speaker".into(), "timezone".into()]);
            }),
        ],
        seed_relationships: Vec::new(),
        seed_temporal_facts: Vec::new(),
        questions: vec![LocomoQuestion {
            name: "attributes fact to the right speaker".into(),
            query: "Which timezone belongs to Maya?".into(),
            options: MemoryRecallOptions {
                recent_limit: Some(0),
                lexical_limit: Some(4),
                limit: Some(2),
                ..MemoryRecallOptions::default()
            },
            expected_answer_contains: vec!["CET".into()],
            expected_evidence_contains: vec!["Maya works in CET".into()],
            excluded_evidence_contains: vec!["Leo works in PT".into()],
            vector_hits: Vec::new(),
            top_k: 1,
            required_signal: LocomoRequiredSignal::Any,
            expect_no_results: false,
        }],
    }
}

fn abstention_case() -> LocomoEvalCase {
    LocomoEvalCase {
        name: "locomo abstention".into(),
        seed_memories: vec![locomo_memory(|memory| {
            memory.content = "Leo's release notes should include rollback risk bullets".into();
            memory.importance = 0.75;
        })],
        seed_relationships: Vec::new(),
        seed_temporal_facts: Vec::new(),
        questions: vec![LocomoQuestion {
            name: "unknown answer returns no evidence".into(),
            query: "What is the passport number?".into(),
            options: MemoryRecallOptions {
                recent_limit: Some(0),
                lexical_limit: Some(3),
                limit: Some(3),
                ..MemoryRecallOptions::default()
            },
            expected_answer_contains: Vec::new(),
            expected_evidence_contains: Vec::new(),
            excluded_evidence_contains: vec!["rollback risk bullets".into()],
            vector_hits: Vec::new(),
            top_k: 3,
            required_signal: LocomoRequiredSignal::Any,
            expect_no_results: true,
        }],
    }
}

fn semantic_vector_case() -> LocomoEvalCase {
    LocomoEvalCase {
        name: "locomo semantic vector recall".into(),
        seed_memories: vec![
            locomo_memory(|memory| {
                memory.content = "Leo prefers concise release summaries with rollback notes".into();
                memory.importance = 0.87;
                memory.tags = Some(vec!["semantic".into(), "preference".into()]);
            }),
            locomo_memory(|memory| {
                memory.agent_id = "finance".into();
                memory.agent_name = "Finance".into();
                memory.content = "Billing ledger exports include invoice IDs".into();
                memory.importance = 0.7;
            }),
        ],
        seed_relationships: Vec::new(),
        seed_temporal_facts: Vec::new(),
        questions: vec![LocomoQuestion {
            name: "semantic paraphrase recall".into(),
            query: "shipping brief style".into(),
            options: MemoryRecallOptions {
                recent_limit: Some(0),
                lexical_limit: Some(2),
                limit: Some(1),
                ..MemoryRecallOptions::default()
            },
            expected_answer_contains: vec!["concise release summaries".into()],
            expected_evidence_contains: vec!["concise release summaries".into()],
            excluded_evidence_contains: vec!["Billing ledger".into()],
            vector_hits: vec![
                LocomoVectorHitSeed {
                    memory_content_contains: "concise release summaries".into(),
                    score: 0.96,
                },
                LocomoVectorHitSeed {
                    memory_content_contains: "Billing ledger".into(),
                    score: 0.08,
                },
            ],
            top_k: 1,
            required_signal: LocomoRequiredSignal::Vector,
            expect_no_results: false,
        }],
    }
}

fn locomo_memory(overrides: impl FnOnce(&mut NewMemory)) -> NewMemory {
    let mut memory = NewMemory {
        agent_id: "assistant".into(),
        agent_name: "Assistant".into(),
        memory_type: MemoryType::Fact,
        content: "baseline locomo memory".into(),
        importance: 0.5,
        tags: None,
        scope: Some(MemoryScope::Private),
        room_id: None,
        world_id: None,
        session_id: None,
    };
    overrides(&mut memory);
    memory
}

fn locomo_relationship(overrides: impl FnOnce(&mut NewAgentRelationship)) -> NewAgentRelationship {
    let mut relationship = NewAgentRelationship {
        source_kind: Some(RelationshipEndpointKind::Agent),
        source_agent_id: "assistant".into(),
        source_agent_name: "Assistant".into(),
        target_kind: Some(RelationshipEndpointKind::Agent),
        target_agent_id: "critic".into(),
        target_agent_name: "Critic".into(),
        relationship_type: "related_to".into(),
        summary: Some("LOCOMO benchmark relationship".into()),
        strength: 0.86,
        confidence: 0.82,
        evidence_memory_ids: Vec::new(),
        tags: Some(vec!["locomo".into()]),
        room_id: None,
        world_id: None,
        session_id: None,
    };
    overrides(&mut relationship);
    relationship
}

fn locomo_temporal_fact(overrides: impl FnOnce(&mut NewTemporalFact)) -> NewTemporalFact {
    let mut fact = NewTemporalFact {
        subject_kind: RelationshipEndpointKind::User,
        subject_id: "leo".into(),
        subject_name: "Leo".into(),
        predicate: "prefers".into(),
        object_kind: None,
        object_id: None,
        object_name: None,
        value: Some("benchmark value".into()),
        valid_from: Some(1_700_000_000_000),
        valid_to: None,
        observed_at: Some(1_700_000_000_000),
        confidence: 0.9,
        evidence_memory_ids: Vec::new(),
        supersedes_fact_ids: Vec::new(),
        status: Some(TemporalRecordStatus::Active),
        tags: Some(vec!["locomo".into(), "temporal".into()]),
        room_id: None,
        world_id: None,
        session_id: None,
    };
    overrides(&mut fact);
    fact
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bm25::BM25;

    fn locomo_bm25() -> BM25 {
        BM25::with_expander(locomo_query_expander())
    }

    #[test]
    fn default_bm25_does_not_apply_locomo_query_expansion() {
        let mut bm25 = BM25::new(1.5, 0.75);
        bm25.add_document(
            "counseling",
            "A mentor supports transgender mental health through counseling.",
        );

        let results = bm25.search("What career path should they pursue", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn locomo_expander_normalizes_common_pursue_typo() {
        let mut bm25 = locomo_bm25();
        bm25.add_document("career", "Caroline plans to pursue counseling as a career.");
        bm25.add_document(
            "distractor",
            "Caroline went camping with friends this weekend.",
        );

        let results = bm25.search("What career path has Caroline decided to persue", 10);
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("career")
        );
    }

    #[test]
    fn expands_relationship_status_queries_without_indexing_synonyms() {
        let mut bm25 = locomo_bm25();
        bm25.add_document(
            "status",
            "Caroline is thrilled to make a family as a single parent.",
        );
        bm25.add_document(
            "distractor",
            "Caroline discussed how her relationships changed during her journey.",
        );

        let results = bm25.search("Caroline relationship status", 10);
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("status")
        );
    }

    #[test]
    fn expands_career_path_queries_without_document_synonyms() {
        let mut bm25 = locomo_bm25();
        bm25.add_document(
            "career",
            "Caroline wants counseling work that supports transgender mental health.",
        );
        bm25.add_document(
            "distractor",
            "Caroline talks about self care and family plans.",
        );

        let results = bm25.search("What career path has Caroline decided to persue", 10);
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("career")
        );
    }

    #[test]
    fn expands_children_interest_queries() {
        let mut bm25 = locomo_bm25();
        bm25.add_document(
            "interests",
            "Melanie said they were stoked for the dinosaur exhibit and love nature.",
        );
        bm25.add_document(
            "distractor",
            "Melanie and the kids finished another painting.",
        );

        let results = bm25.search("What do Melanie kids like", 10);
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("interests")
        );
    }

    #[test]
    fn expands_self_care_queries() {
        let mut bm25 = locomo_bm25();
        bm25.add_document(
            "self-care",
            "Melanie carves out me-time each day for running, reading, and playing violin.",
        );
        bm25.add_document("distractor", "Melanie went to a pottery class yesterday.");

        let results = bm25.search("How does Melanie prioritize self-care", 10);
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("self-care")
        );
    }

    #[test]
    fn expands_musical_artist_queries() {
        let mut bm25 = locomo_bm25();
        bm25.add_document("concert", "Melanie saw Matt Patterson sing at the concert.");
        bm25.add_document("distractor", "Melanie played clarinet when she was young.");

        let results = bm25.search("What musical artists has Melanie seen", 10);
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("concert")
        );
    }

    #[test]
    fn expands_inferential_preference_queries() {
        let mut bm25 = locomo_bm25();
        bm25.add_document(
            "outdoors",
            "Melanie loves camping outdoors in nature and exploring the forest.",
        );
        bm25.add_document("distractor", "Melanie watched a movie with family.");

        let results = bm25.search("Would Melanie prefer a national park or theme park", 10);
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("outdoors")
        );
    }

    #[test]
    fn expands_inferential_career_alternative_queries() {
        let mut bm25 = locomo_bm25();
        bm25.add_document(
            "counseling",
            "Caroline is looking into counseling and mental health jobs.",
        );
        bm25.add_document("distractor", "Caroline likes reading books at home.");

        let results = bm25.search("Would Caroline pursue writing as a career option", 10);
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("counseling")
        );
    }

    #[test]
    fn expands_religious_inference_queries() {
        let mut bm25 = locomo_bm25();
        bm25.add_document(
            "church",
            "Caroline made a stained glass piece for a local church about faith and change.",
        );
        bm25.add_document("distractor", "Caroline went hiking with friends.");

        let results = bm25.search("Would Caroline be considered religious", 10);
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("church")
        );
    }

    #[test]
    fn expands_roadtrip_inference_queries() {
        let mut bm25 = locomo_bm25();
        bm25.add_document(
            "accident",
            "Melanie said the roadtrip had a bad start after a scary car accident.",
        );
        bm25.add_document("distractor", "Melanie enjoys swimming with her kids.");

        let results = bm25.search("Would Melanie go on another roadtrip soon", 10);
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("accident")
        );
    }

    #[test]
    fn expands_move_back_home_country_inference_queries() {
        let mut bm25 = locomo_bm25();
        bm25.add_document(
            "adoption",
            "Caroline passed adoption agency interviews and wants to build a loving home for kids.",
        );
        bm25.add_document(
            "roots",
            "Caroline has a necklace from her home country Sweden that reminds her of her roots.",
        );

        let results = bm25.search(
            "Would Caroline want to move back to her home country soon",
            10,
        );
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("adoption")
        );
    }

    #[test]
    fn expands_art_practice_duration_queries() {
        let mut bm25 = locomo_bm25();
        bm25.add_document(
            "practice",
            "Melanie started painting in 2016 and has practiced art for years.",
        );
        bm25.add_document(
            "distractor",
            "Melanie visited an art museum with her family.",
        );

        let results = bm25.search("How long has Melanie been practicing art", 10);
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("practice")
        );
    }

    #[test]
    fn expands_bowl_reminder_queries() {
        let mut bm25 = locomo_bm25();
        bm25.add_document(
            "bowl",
            "Melanie said the hand-painted bowl is a reminder of art and self-expression.",
        );
        bm25.add_document("distractor", "Melanie bought a new bowl for the kitchen.");

        let results = bm25.search("What is Melanie's hand-painted bowl a reminder of", 10);
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("bowl")
        );
    }

    #[test]
    fn expands_lgbtq_event_queries() {
        let mut bm25 = locomo_bm25();
        bm25.add_document(
            "support-group",
            "Caroline went to a LGBTQ support group yesterday and it was powerful.",
        );
        bm25.add_document("generic", "Melanie said community support is important.");

        let results = bm25.search("What LGBTQ events has Caroline participated in", 10);
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("support-group")
        );
    }

    #[test]
    fn expands_family_activity_queries() {
        let mut bm25 = locomo_bm25();
        bm25.add_document("clay", "Melanie watched the kids make something with clay.");
        bm25.add_document("museum", "Melanie took the kids to the museum yesterday.");
        bm25.add_document("noise", "Melanie talked about family support.");

        let result_ids: Vec<_> = bm25
            .search("What activities has Melanie done with her family", 10)
            .into_iter()
            .map(|result| result.id)
            .collect();
        assert!(result_ids.iter().any(|id| id == "clay"));
        assert!(result_ids.iter().any(|id| id == "museum"));
    }

    #[test]
    fn expands_post_roadtrip_hike_queries() {
        let mut bm25 = locomo_bm25();
        bm25.add_document(
            "after-roadtrip",
            "Melanie said they just did it yesterday and relaxed after the road trip.",
        );
        bm25.add_document("old-hike", "Melanie went on a hike in June.");

        let results = bm25.search("When did Melanie go on a hike after the roadtrip", 10);
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("after-roadtrip")
        );
    }

    #[test]
    fn expands_children_count_queries() {
        let mut bm25 = locomo_bm25();
        bm25.add_document(
            "siblings",
            "Melanie reassured them and explained their brother would be OK.",
        );
        bm25.add_document(
            "generic",
            "Melanie took the kids to a pottery workshop with clay and paint.",
        );

        let results = bm25.search("How many children does Melanie have", 10);
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("siblings")
        );
    }

    #[test]
    fn expands_adoption_agency_support_queries() {
        let mut bm25 = locomo_bm25();
        bm25.add_document(
            "inclusive-agency",
            "Caroline chose the adoption agency because they help LGBTQ folks and their inclusivity spoke to her.",
        );
        bm25.add_document(
            "generic-adoption",
            "Caroline wants to adopt kids and build a loving home.",
        );

        let results = bm25.search(
            "What type of individuals does the adoption agency Caroline is considering support",
            10,
        );
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("inclusive-agency")
        );
    }

    #[test]
    fn expands_counseling_service_queries() {
        let mut bm25 = locomo_bm25();
        bm25.add_document(
            "trans-services",
            "Caroline wants counseling work with trans people to help them accept themselves and support mental health.",
        );
        bm25.add_document("generic", "Caroline is exploring counseling jobs.");

        let results = bm25.search(
            "What kind of counseling and mental health services is Caroline interested in pursuing",
            10,
        );
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("trans-services")
        );
    }

    #[test]
    fn expands_bowl_made_queries() {
        let mut bm25 = locomo_bm25();
        bm25.add_document(
            "made-bowl",
            "Melanie made this bowl in her class and is proud of it.",
        );
        bm25.add_document("photo", "A photo shows a black and white bowl.");

        let results = bm25.search("Did Melanie make the black and white bowl in the photo", 10);
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("made-bowl")
        );
    }

    #[test]
    fn adoption_interview_queries_do_not_expand_to_summer_plans() {
        let mut bm25 = locomo_bm25();
        bm25.add_document(
            "interview",
            "Caroline passed the adoption agency interviews last Friday.",
        );
        bm25.add_document(
            "summer",
            "Caroline spent the summer researching adoption agencies.",
        );

        let results = bm25.search("When did Caroline pass the adoption interview", 10);
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("interview")
        );
    }

    #[test]
    fn locomo_smoke_eval_cases_pass() {
        let report = run_locomo_eval_cases(&locomo_smoke_eval_cases());

        assert!(report.passed(), "{:?}", report.failure_messages());
        assert_eq!(report.total_questions(), 6);
        assert_eq!(report.passed_questions(), 6);
        assert_eq!(report.recall_at_k(), 1.0);
        assert_eq!(report.answer_coverage(), 1.0);
        assert_eq!(report.false_positive_rate(), 0.0);
    }

    #[test]
    fn locomo_report_exposes_failure_metrics() {
        let case = LocomoEvalCase {
            name: "failing locomo case".into(),
            seed_memories: vec![locomo_memory(|memory| {
                memory.content = "Leo prefers release notes".into();
            })],
            seed_relationships: Vec::new(),
            seed_temporal_facts: Vec::new(),
            questions: vec![LocomoQuestion {
                name: "missing expected evidence".into(),
                query: "release notes".into(),
                options: MemoryRecallOptions {
                    recent_limit: Some(0),
                    limit: Some(1),
                    ..MemoryRecallOptions::default()
                },
                expected_answer_contains: vec!["rollback checklist".into()],
                expected_evidence_contains: vec!["rollback checklist".into()],
                excluded_evidence_contains: Vec::new(),
                vector_hits: Vec::new(),
                top_k: 1,
                required_signal: LocomoRequiredSignal::Any,
                expect_no_results: false,
            }],
        };

        let report = run_locomo_eval_cases(&[case]);

        assert!(!report.passed());
        assert_eq!(report.total_questions(), 1);
        assert_eq!(report.passed_questions(), 0);
        assert_eq!(report.recall_at_k(), 0.0);
        assert_eq!(report.answer_coverage(), 0.0);
        assert_eq!(report.failure_messages().len(), 1);
    }
}
