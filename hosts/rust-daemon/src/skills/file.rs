//! The `SKILL.md` format (spec §8.1): YAML front matter with `name` and
//! `description` between `---` lines, one blank line, then the Markdown
//! body. The daemon always writes the canonical form `compose_skill_file`
//! makes; hand-written files may add a byte-order mark, CRLF line ends, and
//! other front-matter keys.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    validate_body, validate_description, validate_name, MAX_SKILL_FRONT_MATTER_BYTES,
    SKILL_FILE_FRONT_MATTER_INVALID, SKILL_FILE_FRONT_MATTER_TOO_LARGE, SKILL_FILE_NOT_UTF8,
    SKILL_FILE_NO_FRONT_MATTER,
};

/// What a valid `SKILL.md` holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SkillFile {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) body: String,
}

#[derive(Deserialize)]
struct FrontMatterIn {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Serialize)]
struct FrontMatterOut<'a> {
    name: &'a str,
    description: &'a str,
}

/// Lowercase hex SHA-256 of a whole `SKILL.md` (spec §8.1 `approvedHash`).
pub(crate) fn skill_hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The canonical `SKILL.md` for already-validated parts.
pub(crate) fn compose_skill_file(name: &str, description: &str, body: &str) -> String {
    let front = serde_yaml::to_string(&FrontMatterOut { name, description })
        .expect("two strings always serialize as YAML");
    format!("---\n{front}---\n\n{body}")
}

/// Reads a `SKILL.md`: front matter at most 4 KiB with a valid one-line
/// `name` and `description`, and a body that is not blank and at most 32 KiB.
pub(crate) fn parse_skill_file(bytes: &[u8]) -> Result<SkillFile, &'static str> {
    let text = std::str::from_utf8(bytes).map_err(|_| SKILL_FILE_NOT_UTF8)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
        .ok_or(SKILL_FILE_NO_FRONT_MATTER)?;
    let (front, body) = split_front_matter(rest).ok_or(SKILL_FILE_NO_FRONT_MATTER)?;
    if front.len() > MAX_SKILL_FRONT_MATTER_BYTES {
        return Err(SKILL_FILE_FRONT_MATTER_TOO_LARGE);
    }
    let parsed: FrontMatterIn =
        serde_yaml::from_str(front).map_err(|_| SKILL_FILE_FRONT_MATTER_INVALID)?;
    let (Some(name), Some(description)) = (parsed.name, parsed.description) else {
        return Err(SKILL_FILE_FRONT_MATTER_INVALID);
    };
    let name = validate_name(&name)?;
    let description = validate_description(&description)?;
    validate_body(body)?;
    Ok(SkillFile {
        name,
        description,
        body: body.to_string(),
    })
}

/// After the opening `---` line: the front matter up to the closing `---`
/// line, and the body after it. One line end right after the closing line
/// belongs to the format, not the body.
fn split_front_matter(rest: &str) -> Option<(&str, &str)> {
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\n', '\r']) == "---" {
            let front = &rest[..offset];
            let after = &rest[offset + line.len()..];
            let body = after
                .strip_prefix("\r\n")
                .or_else(|| after.strip_prefix('\n'))
                .unwrap_or(after);
            return Some((front, body));
        }
        offset += line.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{
        MAX_SKILL_BODY_BYTES, SKILL_BODY_TOO_LARGE, SKILL_DESCRIPTION_INVALID,
        SKILL_FILE_FRONT_MATTER_INVALID, SKILL_FILE_FRONT_MATTER_TOO_LARGE, SKILL_FILE_NOT_UTF8,
        SKILL_FILE_NO_FRONT_MATTER, SKILL_TEXT_HIDDEN,
    };

    #[test]
    fn composed_files_parse_back_to_the_same_content() {
        for (name, description, body) in [
            ("Notes", "Take meeting notes", "Write them down.\n"),
            (
                "Plan: \"weekly\" #1",
                "- starts with a dash: and 'quotes'",
                "\nLeading blank line\r\nand CRLF",
            ),
            (
                "Été",
                "Résumé en français",
                "# Heading\n\n---\n\nA rule above",
            ),
        ] {
            let composed = compose_skill_file(name, description, body);
            assert!(composed.starts_with("---\n"), "{composed}");
            let parsed = parse_skill_file(composed.as_bytes()).unwrap();
            assert_eq!(
                parsed,
                SkillFile {
                    name: name.into(),
                    description: description.into(),
                    body: body.into(),
                }
            );
        }
    }

    #[test]
    fn a_hand_written_file_with_a_bom_crlf_and_extra_keys_parses() {
        let text = "\u{feff}---\r\nname: Triage\r\ndescription: Sort the inbox\r\nversion: 2\r\n---\r\n\r\nRead each mail.\r\n";
        let parsed = parse_skill_file(text.as_bytes()).unwrap();
        assert_eq!(parsed.name, "Triage");
        assert_eq!(parsed.description, "Sort the inbox");
        assert_eq!(parsed.body, "Read each mail.\r\n");
    }

    #[test]
    fn a_file_without_valid_front_matter_is_refused() {
        let big_front = format!(
            "---\nname: n\ndescription: {}\n---\n\nbody",
            "d".repeat(4 * 1024)
        );
        let big_body = format!(
            "---\nname: n\ndescription: d\n---\n\n{}",
            "b".repeat(MAX_SKILL_BODY_BYTES + 1)
        );
        for (text, problem) in [
            ("no front matter", SKILL_FILE_NO_FRONT_MATTER),
            ("---\nname: n\ndescription: d\n", SKILL_FILE_NO_FRONT_MATTER),
            (big_front.as_str(), SKILL_FILE_FRONT_MATTER_TOO_LARGE),
            (
                "---\nname: [a, b]\ndescription: d\n---\n\nbody",
                SKILL_FILE_FRONT_MATTER_INVALID,
            ),
            ("---\nname: n\n---\n\nbody", SKILL_FILE_FRONT_MATTER_INVALID),
            (
                "---\nname: n\ndescription: \"two\\nlines\"\n---\n\nbody",
                SKILL_DESCRIPTION_INVALID,
            ),
            (big_body.as_str(), SKILL_BODY_TOO_LARGE),
        ] {
            assert_eq!(
                parse_skill_file(text.as_bytes()),
                Err(problem),
                "{text:.60}"
            );
        }
        assert_eq!(
            parse_skill_file(&[0xff, 0xfe, 0x00]),
            Err(SKILL_FILE_NOT_UTF8)
        );
    }

    #[test]
    fn a_file_with_hidden_text_is_refused() {
        for text in [
            "---\nname: \"a\\u200Bb\"\ndescription: d\n---\n\nbody",
            "---\nname: n\ndescription: d\n---\n\nbody\u{E0041}",
        ] {
            assert_eq!(
                parse_skill_file(text.as_bytes()),
                Err(SKILL_TEXT_HIDDEN),
                "{text:?}"
            );
        }
    }

    #[test]
    fn the_hash_is_the_lowercase_hex_sha256_of_the_bytes() {
        assert_eq!(
            skill_hash(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
