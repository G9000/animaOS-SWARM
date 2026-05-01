use std::collections::HashMap;
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub struct SearchResult {
    pub id: String,
    pub score: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAnalysisProfile {
    Unicode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextAnalyzer {
    profile: TextAnalysisProfile,
}

impl Default for TextAnalyzer {
    fn default() -> Self {
        Self::multilingual()
    }
}

impl TextAnalyzer {
    pub const fn multilingual() -> Self {
        Self {
            profile: TextAnalysisProfile::Unicode,
        }
    }

    pub const fn unicode() -> Self {
        Self::multilingual()
    }

    pub fn profile(&self) -> TextAnalysisProfile {
        self.profile
    }

    fn tokenize(&self, text: &str) -> Vec<String> {
        tokenize(text)
    }
}

pub type QueryExpansionRuleFn = for<'a> fn(&mut QueryExpansionContext<'a>);

#[derive(Clone, Copy)]
pub struct QueryExpansionRule {
    name: &'static str,
    expand: QueryExpansionRuleFn,
}

impl QueryExpansionRule {
    pub const fn new(name: &'static str, expand: QueryExpansionRuleFn) -> Self {
        Self { name, expand }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
}

impl fmt::Debug for QueryExpansionRule {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QueryExpansionRule")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Default)]
pub struct QueryExpander {
    rules: Vec<QueryExpansionRule>,
}

impl QueryExpander {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_rules(rules: impl IntoIterator<Item = QueryExpansionRule>) -> Self {
        Self {
            rules: rules.into_iter().collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    fn expand_terms(&self, terms: &mut Vec<String>, text_analyzer: &TextAnalyzer) {
        let mut context = QueryExpansionContext {
            terms,
            text_analyzer,
        };
        for rule in &self.rules {
            (rule.expand)(&mut context);
        }
    }
}

pub struct QueryExpansionContext<'a> {
    terms: &'a mut Vec<String>,
    text_analyzer: &'a TextAnalyzer,
}

impl QueryExpansionContext<'_> {
    pub fn has_term(&self, expected: &str) -> bool {
        self.terms.iter().any(|term| term == expected)
    }

    pub fn has_terms(&self, expected: &[&str]) -> bool {
        expected.iter().all(|term| self.has_term(term))
    }

    pub fn push_terms(&mut self, expansions: &[&str]) {
        push_unique_terms(self.terms, expansions, self.text_analyzer);
    }
}

#[derive(Clone, Debug)]
struct DocEntry {
    id: String,
    terms: Vec<String>,
    term_freqs: HashMap<String, usize>,
}

#[derive(Clone, Debug)]
pub struct BM25 {
    docs: HashMap<String, DocEntry>,
    doc_freq: HashMap<String, usize>,
    avg_doc_len: f64,
    k1: f64,
    b: f64,
    query_expander: QueryExpander,
    text_analyzer: TextAnalyzer,
}

impl Default for BM25 {
    fn default() -> Self {
        Self::new(1.5, 0.75)
    }
}

impl BM25 {
    pub fn new(k1: f64, b: f64) -> Self {
        Self::with_parameters_and_expander(k1, b, QueryExpander::default())
    }

    pub fn with_analyzer(text_analyzer: TextAnalyzer) -> Self {
        Self::with_parameters_expander_and_analyzer(
            1.5,
            0.75,
            QueryExpander::default(),
            text_analyzer,
        )
    }

    pub fn with_expander(query_expander: QueryExpander) -> Self {
        Self::with_parameters_and_expander(1.5, 0.75, query_expander)
    }

    pub fn with_expander_and_analyzer(
        query_expander: QueryExpander,
        text_analyzer: TextAnalyzer,
    ) -> Self {
        Self::with_parameters_expander_and_analyzer(1.5, 0.75, query_expander, text_analyzer)
    }

    pub fn with_parameters_and_expander(k1: f64, b: f64, query_expander: QueryExpander) -> Self {
        Self::with_parameters_expander_and_analyzer(k1, b, query_expander, TextAnalyzer::default())
    }

    pub fn with_parameters_expander_and_analyzer(
        k1: f64,
        b: f64,
        query_expander: QueryExpander,
        text_analyzer: TextAnalyzer,
    ) -> Self {
        Self {
            docs: HashMap::new(),
            doc_freq: HashMap::new(),
            avg_doc_len: 0.0,
            k1,
            b,
            query_expander,
            text_analyzer,
        }
    }

    pub fn add_document(&mut self, id: impl Into<String>, text: impl Into<String>) {
        let id = id.into();
        let text = text.into();

        if self.docs.contains_key(&id) {
            self.remove_document(&id);
        }

        let terms = self.text_analyzer.tokenize(&text);
        let mut term_freqs = HashMap::new();
        for term in &terms {
            *term_freqs.entry(term.clone()).or_insert(0) += 1;
        }

        for term in term_freqs.keys() {
            *self.doc_freq.entry(term.clone()).or_insert(0) += 1;
        }

        self.docs.insert(
            id.clone(),
            DocEntry {
                id,
                terms,
                term_freqs,
            },
        );
        self.update_avg_len();
    }

    pub fn remove_document(&mut self, id: &str) {
        let Some(doc) = self.docs.remove(id) else {
            return;
        };

        for term in doc.term_freqs.keys() {
            match self.doc_freq.get(term).copied() {
                Some(0) | None => {
                    self.doc_freq.remove(term);
                }
                Some(1) => {
                    self.doc_freq.remove(term);
                }
                Some(count) => {
                    self.doc_freq.insert(term.clone(), count - 1);
                }
            }
        }

        self.update_avg_len();
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        let query_terms = tokenize_query(query, &self.query_expander, &self.text_analyzer);
        if query_terms.is_empty() || self.docs.is_empty() || limit == 0 {
            return Vec::new();
        }

        let total_docs = self.docs.len() as f64;
        let mut scores = Vec::new();

        for doc in self.docs.values() {
            let mut score = 0.0;
            let doc_len = doc.terms.len() as f64;

            for term in &query_terms {
                let tf = doc.term_freqs.get(term).copied().unwrap_or_default() as f64;
                if tf == 0.0 {
                    continue;
                }

                let df = self.doc_freq.get(term).copied().unwrap_or_default() as f64;
                let idf = (1.0 + (total_docs - df + 0.5) / (df + 0.5)).ln();
                let tf_norm = (tf * (self.k1 + 1.0))
                    / (tf + self.k1 * (1.0 - self.b + self.b * (doc_len / self.avg_doc_len)));
                score += idf * tf_norm;
            }

            if score > 0.0 {
                scores.push(SearchResult {
                    id: doc.id.clone(),
                    score,
                });
            }
        }

        scores.sort_by(|left, right| right.score.total_cmp(&left.score));
        scores.truncate(limit);
        scores
    }

    pub fn clear(&mut self) {
        self.docs.clear();
        self.doc_freq.clear();
        self.avg_doc_len = 0.0;
    }

    pub fn size(&self) -> usize {
        self.docs.len()
    }

    fn update_avg_len(&mut self) {
        if self.docs.is_empty() {
            self.avg_doc_len = 0.0;
            return;
        }

        let total_terms: usize = self.docs.values().map(|doc| doc.terms.len()).sum();
        self.avg_doc_len = total_terms as f64 / self.docs.len() as f64;
    }
}

fn tokenize_query(
    text: &str,
    query_expander: &QueryExpander,
    text_analyzer: &TextAnalyzer,
) -> Vec<String> {
    let mut terms = text_analyzer.tokenize(text);
    query_expander.expand_terms(&mut terms, text_analyzer);
    terms
}

fn tokenize(text: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut word = String::new();
    let mut cjk_run = Vec::new();

    for character in text.chars() {
        if is_cjk(character) {
            flush_word(&mut word, &mut terms);
            cjk_run.push(character);
            continue;
        }

        if character.is_alphanumeric() {
            flush_cjk_run(&mut cjk_run, &mut terms);
            for lowercase in character.to_lowercase() {
                word.push(lowercase);
            }
        } else {
            flush_word(&mut word, &mut terms);
            flush_cjk_run(&mut cjk_run, &mut terms);
        }
    }

    flush_word(&mut word, &mut terms);
    flush_cjk_run(&mut cjk_run, &mut terms);

    terms
}

fn flush_word(word: &mut String, terms: &mut Vec<String>) {
    if word.chars().count() <= 1 {
        word.clear();
        return;
    }

    terms.push(word.to_string());
    word.clear();
}

fn flush_cjk_run(cjk_run: &mut Vec<char>, terms: &mut Vec<String>) {
    if cjk_run.is_empty() {
        return;
    }

    terms.extend(cjk_run.iter().map(char::to_string));
    for pair in cjk_run.windows(2) {
        terms.push(pair.iter().collect());
    }
    cjk_run.clear();
}

fn push_unique_terms(terms: &mut Vec<String>, expansions: &[&str], text_analyzer: &TextAnalyzer) {
    for expansion in expansions {
        for term in text_analyzer.tokenize(expansion) {
            if !terms.iter().any(|existing| existing == &term) {
                terms.push(term);
            }
        }
    }
}

fn is_cjk(character: char) -> bool {
    matches!(
        character as u32,
        0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xac00..=0xd7af
    )
}

#[cfg(test)]
mod tests {
    use super::{QueryExpander, QueryExpansionContext, QueryExpansionRule, TextAnalyzer, BM25};

    #[test]
    fn indexes_and_searches_documents() {
        let mut bm25 = BM25::new(1.5, 0.75);
        bm25.add_document("1", "The quick brown fox jumps over the lazy dog");
        bm25.add_document("2", "A fast red car drives on the highway");
        bm25.add_document("3", "The brown bear sleeps in the forest");

        let results = bm25.search("brown fox", 10);
        assert!(!results.is_empty());
        assert_eq!(results[0].id, "1");
    }

    #[test]
    fn ranks_repeated_terms_higher() {
        let mut bm25 = BM25::new(1.5, 0.75);
        bm25.add_document("1", "TypeScript is a language");
        bm25.add_document(
            "2",
            "TypeScript TypeScript TypeScript everywhere, TypeScript is the best",
        );
        bm25.add_document("3", "Python is also a language");

        let results = bm25.search("TypeScript", 10);
        assert!(results.len() >= 2);

        let doc1_score = results
            .iter()
            .find(|result| result.id == "1")
            .map(|result| result.score)
            .unwrap_or_default();
        let doc2_score = results
            .iter()
            .find(|result| result.id == "2")
            .map(|result| result.score)
            .unwrap_or_default();

        assert!(doc2_score >= doc1_score);
    }

    #[test]
    fn returns_empty_for_no_matches() {
        let mut bm25 = BM25::new(1.5, 0.75);
        bm25.add_document("1", "Hello world");

        let results = bm25.search("xyz123 nonexistent", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn removes_documents() {
        let mut bm25 = BM25::new(1.5, 0.75);
        bm25.add_document("1", "agent swarm coordination");
        bm25.add_document("2", "agent task execution");

        bm25.remove_document("1");

        let results = bm25.search("agent", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "2");
    }

    #[test]
    fn returns_empty_for_blank_queries() {
        let mut bm25 = BM25::new(1.5, 0.75);
        bm25.add_document("1", "Hello world");

        let results = bm25.search("", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn clears_all_documents() {
        let mut bm25 = BM25::new(1.5, 0.75);
        bm25.add_document("1", "test document");
        bm25.clear();

        assert_eq!(bm25.size(), 0);
        assert!(bm25.search("test", 10).is_empty());
    }

    #[test]
    fn readding_document_replaces_previous_content() {
        let mut bm25 = BM25::new(1.5, 0.75);
        bm25.add_document("1", "old content about cats");
        bm25.add_document("1", "new content about dogs");

        assert_eq!(bm25.size(), 1);
        assert!(bm25.search("cats", 10).is_empty());
        assert_eq!(bm25.search("dogs", 10).len(), 1);
    }

    #[test]
    fn respects_limit() {
        let mut bm25 = BM25::new(1.5, 0.75);
        for index in 0..20 {
            bm25.add_document(
                index.to_string(),
                format!("document number {index} about testing"),
            );
        }

        let results = bm25.search("testing", 5);
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn keeps_base_words_distinct() {
        let mut bm25 = BM25::new(1.5, 0.75);
        bm25.add_document("ledger", "The ledger contains invoice history.");
        bm25.add_document("ledge", "The ledge is near the window.");
        bm25.add_document("campus", "The campus has a new lab.");
        bm25.add_document("camp", "The camp opens in July.");

        let ledger_results = bm25.search("ledger", 10);
        assert_eq!(
            ledger_results.first().map(|result| result.id.as_str()),
            Some("ledger")
        );

        let campus_results = bm25.search("campus", 10);
        assert_eq!(
            campus_results.first().map(|result| result.id.as_str()),
            Some("campus")
        );
    }

    #[test]
    fn default_analyzer_keeps_question_words_as_search_terms() {
        let mut bm25 = BM25::new(1.5, 0.75);
        bm25.add_document("question", "what when where why how");

        let results = bm25.search("what when where", 10);
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("question")
        );
    }

    #[test]
    fn multilingual_analyzer_keeps_question_words_as_search_terms() {
        let mut bm25 = BM25::with_analyzer(TextAnalyzer::multilingual());
        bm25.add_document("question", "what when where why how");

        let results = bm25.search("what where", 10);
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("question")
        );
    }

    #[test]
    fn multilingual_analyzer_preserves_non_ascii_latin_terms() {
        let mut bm25 = BM25::with_analyzer(TextAnalyzer::multilingual());
        bm25.add_document("accented", "Cafe notes: mañana, café, résumé.");
        bm25.add_document("plain", "Cafe notes without accented words.");

        let results = bm25.search("café résumé", 10);
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("accented")
        );
    }

    #[test]
    fn multilingual_analyzer_matches_cjk_text_without_whitespace() {
        let mut bm25 = BM25::with_analyzer(TextAnalyzer::multilingual());
        bm25.add_document("tokyo", "東京で寿司を食べた");
        bm25.add_document("kyoto", "京都でお茶を飲んだ");

        let tokyo_results = bm25.search("東京", 10);
        assert_eq!(
            tokyo_results.first().map(|result| result.id.as_str()),
            Some("tokyo")
        );

        let sushi_results = bm25.search("寿司", 10);
        assert_eq!(
            sushi_results.first().map(|result| result.id.as_str()),
            Some("tokyo")
        );
    }

    #[test]
    fn applies_supplied_query_expander() {
        fn expand_application_terms(context: &mut QueryExpansionContext<'_>) {
            if context.has_term("app") {
                context.push_terms(&["application"]);
            }
        }

        let mut bm25 = BM25::with_expander(QueryExpander::with_rules([QueryExpansionRule::new(
            "application-alias",
            expand_application_terms,
        )]));
        bm25.add_document("application", "The application stores local settings.");

        let results = bm25.search("app settings", 10);
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some("application")
        );
    }
}
