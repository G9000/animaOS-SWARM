//! Memory text rules shared by the owner's memory routes and the companion's
//! memory tool (spec §10, §16): limits, the hidden-text check, tag cleaning.

/// The most characters `PATCH /api/memories/{id}` accepts as content.
pub(crate) const MAX_MEMORY_EDIT_CHARS: usize = 8_000;
/// The most tags a memory keeps after cleaning.
pub(crate) const MAX_MEMORY_TAGS: usize = 20;
/// The most characters one tag may have.
pub(crate) const MAX_MEMORY_TAG_CHARS: usize = 40;
/// The most characters a fact's value may have.
#[allow(dead_code)] // Used by the facts routes (M7 Task 3).
pub(crate) const MAX_FACT_VALUE_CHARS: usize = 500;

pub(crate) const MEMORY_TEXT_HIDDEN: &str =
    "Memory text must not contain invisible tag or direction-override characters";
pub(crate) const MEMORY_CONTENT_INVALID: &str = "content must be 1 to 8000 characters";
pub(crate) const MEMORY_TAGS_INVALID: &str =
    "tags must be at most 20 non-empty tags of at most 40 characters";
#[allow(dead_code)] // Used by the facts routes (M7 Task 3).
pub(crate) const FACT_VALUE_INVALID: &str = "value must be 1 to 500 characters";

/// True when the text smuggles Unicode tag characters, bidirectional
/// embeddings, overrides, isolates, or variation-selector supplements.
pub(crate) fn has_hidden_text(text: &str) -> bool {
    text.chars().any(crate::skills::is_smuggling_character)
}

/// Trims each tag, drops empty ones, and keeps the first of duplicates.
/// Refuses more than `MAX_MEMORY_TAGS` tags, a tag longer than
/// `MAX_MEMORY_TAG_CHARS` characters, or a tag with hidden text.
pub(crate) fn clean_tags(tags: Vec<String>) -> Result<Vec<String>, &'static str> {
    let mut cleaned: Vec<String> = Vec::with_capacity(tags.len());
    for tag in tags {
        let tag = tag.trim();
        if tag.is_empty() || cleaned.iter().any(|kept| kept == tag) {
            continue;
        }
        if tag.chars().count() > MAX_MEMORY_TAG_CHARS || has_hidden_text(tag) {
            return Err(MEMORY_TAGS_INVALID);
        }
        cleaned.push(tag.to_string());
    }
    if cleaned.len() > MAX_MEMORY_TAGS {
        return Err(MEMORY_TAGS_INVALID);
    }
    Ok(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_messages_are_exact() {
        assert_eq!(
            MEMORY_TEXT_HIDDEN,
            "Memory text must not contain invisible tag or direction-override characters"
        );
        assert_eq!(
            MEMORY_CONTENT_INVALID,
            "content must be 1 to 8000 characters"
        );
        assert_eq!(
            MEMORY_TAGS_INVALID,
            "tags must be at most 20 non-empty tags of at most 40 characters"
        );
        assert_eq!(FACT_VALUE_INVALID, "value must be 1 to 500 characters");
        assert_eq!(MAX_MEMORY_EDIT_CHARS, 8_000);
        assert_eq!(MAX_FACT_VALUE_CHARS, 500);
    }

    #[test]
    fn hidden_text_flags_smuggling_characters_only() {
        for hidden in ['\u{E0041}', '\u{202E}', '\u{2066}', '\u{E0100}'] {
            assert!(has_hidden_text(&format!("note{hidden}x")), "{hidden:?}");
        }
        assert!(!has_hidden_text("zero\u{200B}width"));
        assert!(!has_hidden_text("plain text, café"));
    }

    #[test]
    fn clean_tags_trims_drops_empties_and_deduplicates() {
        let cleaned = clean_tags(vec![
            " work ".into(),
            "".into(),
            "   ".into(),
            "work".into(),
            "home".into(),
        ])
        .unwrap();
        assert_eq!(cleaned, vec!["work".to_string(), "home".to_string()]);
        assert_eq!(clean_tags(vec![" ".into()]).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn clean_tags_enforces_the_limits_by_characters() {
        let forty = "é".repeat(MAX_MEMORY_TAG_CHARS);
        assert!(forty.len() > MAX_MEMORY_TAG_CHARS);
        assert_eq!(clean_tags(vec![forty.clone()]).unwrap(), vec![forty]);
        assert_eq!(
            clean_tags(vec!["é".repeat(MAX_MEMORY_TAG_CHARS + 1)]),
            Err(MEMORY_TAGS_INVALID)
        );

        let twenty: Vec<String> = (0..MAX_MEMORY_TAGS).map(|n| format!("t{n}")).collect();
        assert_eq!(clean_tags(twenty.clone()).unwrap(), twenty);
        let mut twenty_one = twenty.clone();
        twenty_one.push("t20".into());
        assert_eq!(clean_tags(twenty_one), Err(MEMORY_TAGS_INVALID));
        // Duplicates collapse before counting.
        let mut with_duplicate = twenty;
        with_duplicate.push("t0".into());
        assert_eq!(clean_tags(with_duplicate).unwrap().len(), MAX_MEMORY_TAGS);

        assert_eq!(
            clean_tags(vec!["a\u{E0041}".into()]),
            Err(MEMORY_TAGS_INVALID)
        );
    }
}
