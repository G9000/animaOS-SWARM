use std::collections::HashMap;

const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "in", "on", "at", "to", "for", "of", "with", "by", "is",
    "are", "was", "were", "be", "been", "being", "have", "has", "had", "do", "does", "did", "will",
    "would", "could", "should", "may", "might", "can", "this", "that", "these", "those", "it",
    "its", "i", "you", "he", "she", "we", "they", "me", "him", "her", "us", "them", "my", "your",
    "his", "our", "their", "not", "no",
];

#[derive(Clone, Debug, PartialEq)]
pub struct SearchResult {
    pub id: String,
    pub score: f64,
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
}

impl Default for BM25 {
    fn default() -> Self {
        Self::new(1.5, 0.75)
    }
}

impl BM25 {
    pub fn new(k1: f64, b: f64) -> Self {
        Self {
            docs: HashMap::new(),
            doc_freq: HashMap::new(),
            avg_doc_len: 0.0,
            k1,
            b,
        }
    }

    pub fn add_document(&mut self, id: impl Into<String>, text: impl Into<String>) {
        let id = id.into();
        let text = text.into();

        if self.docs.contains_key(&id) {
            self.remove_document(&id);
        }

        let terms = tokenize(&text);
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
        let query_terms = tokenize(query);
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

fn tokenize(text: &str) -> Vec<String> {
    let normalized: String = text
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character.is_ascii_whitespace() {
                character.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect();

    normalized
        .split_whitespace()
        .filter(|word| word.len() > 1 && !STOP_WORDS.contains(word))
        .map(simple_stem)
        .collect()
}

fn simple_stem(word: &str) -> String {
    if word.len() < 4 {
        return word.to_string();
    }

    if let Some(stripped) = word.strip_suffix("ing") {
        return stripped.to_string();
    }
    if let Some(stripped) = word.strip_suffix("tion") {
        return stripped.to_string();
    }
    if let Some(stripped) = word.strip_suffix("ness") {
        return stripped.to_string();
    }
    if let Some(stripped) = word.strip_suffix("ment") {
        return stripped.to_string();
    }
    if let Some(stripped) = word.strip_suffix("able") {
        return stripped.to_string();
    }
    if let Some(stripped) = word.strip_suffix("ible") {
        return stripped.to_string();
    }
    if let Some(stripped) = word.strip_suffix("ally") {
        return stripped.to_string();
    }
    if let Some(stripped) = word.strip_suffix("ies") {
        return format!("{stripped}y");
    }
    if let Some(stripped) = word.strip_suffix("ed") {
        return stripped.to_string();
    }
    if let Some(stripped) = word.strip_suffix("ly") {
        return stripped.to_string();
    }
    if let Some(stripped) = word.strip_suffix("er") {
        return stripped.to_string();
    }
    if let Some(stripped) = word.strip_suffix("es") {
        return stripped.to_string();
    }
    if word.ends_with('s') && !word.ends_with("ss") {
        return word[..word.len() - 1].to_string();
    }

    word.to_string()
}

#[cfg(test)]
mod tests {
    use super::BM25;

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
}
