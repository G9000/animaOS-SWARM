//! Pattern-based secret redaction for captured log lines.
//!
//! Every line the log layer captures passes through [`redact`] before it is
//! stored, so neither the log routes nor a copy in the UI can hold a secret.
//! The patterns are built once and applied in a fixed order; the `regex`
//! crate guarantees linear time, and the caller caps the input at 64 KiB.

use std::borrow::Cow;
use std::sync::OnceLock;

use regex::{Captures, Regex};

use super::LOG_REDACTED;

struct Patterns {
    authorization: Regex,
    bearer: Regex,
    labeled: Regex,
    query: Regex,
    telegram_bot: Regex,
    telegram_bare: Regex,
    prefixed: Regex,
    jwt: Regex,
    long_token: Regex,
}

fn patterns() -> &'static Patterns {
    static PATTERNS: OnceLock<Patterns> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let build = |pattern: &str| Regex::new(pattern).expect("log redaction pattern is valid");
        Patterns {
            // (1) `Authorization` / `Proxy-Authorization` values, with an optional scheme word.
            authorization: build(
                r#"(?i)\b((?:proxy-)?authorization\\?["']?\s*[:=]\s*\\?["']?)((?:bearer|basic|token)\s+)?(?:\[redacted\]|[^\s"',;\\]+)"#,
            ),
            // (2) `Bearer <token>` anywhere.
            bearer: build(r"(?i)\b(bearer\s+)[A-Za-z0-9\-._~+/]{8,}=*"),
            // (3) `key=value`, `key: value`, and JSON `"key":"value"` with a secret-looking key.
            labeled: build(
                r#"(?i)([A-Za-z0-9_\-]*(?:api[_\-]?key|secret|token|password|passwd|credential|cookie|session_id|private[_\-]?key)[A-Za-z0-9_\-]*)(\\?["']?\s*[:=]\s*)(\[redacted\]|\\"(?:[^"\\]|\\[^"])*\\"|"(?:[^"\\]|\\.)*"|'[^']*'|[^\s,;&"'}\]]+)"#,
            ),
            // (4) URL query values for credential-bearing parameters.
            query: build(
                r#"(?i)([?&](?:key|api_key|apikey|token|access_token|refresh_token|id_token|client_secret|secret|password|code|state|signature|sig|auth)=)[^&\s#"']+"#,
            ),
            // (5) Telegram bot tokens, inside `/bot<token>/` URLs and bare.
            telegram_bot: build(r"bot\d{6,12}:[A-Za-z0-9_\-]{30,}"),
            telegram_bare: build(r"\b\d{6,12}:[A-Za-z0-9_\-]{30,}"),
            // (6) Known key prefixes.
            prefixed: build(
                r"\b(?:(?:sk-ant-|sk-proj-|sk-|pk-|rk-|ghp_|gho_|ghu_|ghs_|github_pat_|xox[abprs]-|glpat-|AIza)[A-Za-z0-9_\-]{8,}|ya29\.[A-Za-z0-9_\-.]{8,}|AKIA[0-9A-Z]{16})",
            ),
            // (7) JWTs.
            jwt: build(r"\beyJ[A-Za-z0-9_\-]{2,}\.[A-Za-z0-9_\-]{2,}\.[A-Za-z0-9_\-]*"),
            // (8) Unlabeled long mixed-case tokens (checked by `is_mixed_token`).
            long_token: build(r"[A-Za-z0-9_\-]{32,}"),
        }
    })
}

/// Replaces every secret the patterns recognize with [`LOG_REDACTED`],
/// keeping the label (header name, key, query name) so the line still reads.
pub(crate) fn redact(text: &str) -> String {
    let patterns = patterns();
    let text = patterns
        .authorization
        .replace_all(text, format!("${{1}}${{2}}{LOG_REDACTED}").as_str());
    let text = replace(&text, &patterns.bearer, |caps| {
        format!("{}{LOG_REDACTED}", &caps[1])
    });
    let text = replace(&text, &patterns.labeled, |caps| {
        let key = caps[1].to_ascii_lowercase();
        if is_count_key(&key) {
            return caps[0].to_string();
        }
        let value = &caps[3];
        let quoted = if value.starts_with("\\\"") {
            format!("\\\"{LOG_REDACTED}\\\"")
        } else if value.starts_with('"') {
            format!("\"{LOG_REDACTED}\"")
        } else if value.starts_with('\'') {
            format!("'{LOG_REDACTED}'")
        } else {
            LOG_REDACTED.to_string()
        };
        format!("{}{}{quoted}", &caps[1], &caps[2])
    });
    let text = replace(&text, &patterns.query, |caps| {
        format!("{}{LOG_REDACTED}", &caps[1])
    });
    let text = replace(&text, &patterns.telegram_bot, |_| {
        format!("bot{LOG_REDACTED}")
    });
    let text = replace(&text, &patterns.telegram_bare, |_| LOG_REDACTED.to_string());
    let text = replace(&text, &patterns.prefixed, |_| LOG_REDACTED.to_string());
    let text = replace(&text, &patterns.jwt, |_| LOG_REDACTED.to_string());
    let text = replace(&text, &patterns.long_token, |caps| {
        if is_mixed_token(&caps[0]) {
            LOG_REDACTED.to_string()
        } else {
            caps[0].to_string()
        }
    });
    text.into_owned()
}

fn replace<'a>(
    text: &'a str,
    pattern: &Regex,
    with: impl FnMut(&Captures<'_>) -> String,
) -> Cow<'a, str> {
    pattern.replace_all(text, with)
}

/// A key naming a count or a limit (`prompt_tokens`, `max_tokens`,
/// `token_count`, `token_limit`) is not a secret.
fn is_count_key(lowercase_key: &str) -> bool {
    lowercase_key.ends_with("tokens")
        || lowercase_key.ends_with("token_count")
        || lowercase_key.ends_with("token_limit")
}

/// A long run of key characters is treated as a token only when it mixes a
/// lowercase letter, an uppercase letter, and a digit, so ids such as
/// `run_<uuid>`, lowercase hex hashes, and plain words survive.
fn is_mixed_token(run: &str) -> bool {
    run.bytes().any(|byte| byte.is_ascii_lowercase())
        && run.bytes().any(|byte| byte.is_ascii_uppercase())
        && run.bytes().any(|byte| byte.is_ascii_digit())
}

/// True when a structured field's name looks secret, so its value is
/// replaced without being formatted.
pub(crate) fn is_secret_field(name: &str) -> bool {
    let name = name.to_ascii_lowercase().replace('-', "_");
    if name.contains("token") && !is_count_key(&name) {
        return true;
    }
    [
        "api_key",
        "apikey",
        "secret",
        "password",
        "passwd",
        "authorization",
        "credential",
        "cookie",
        "private_key",
        "signature",
    ]
    .iter()
    .any(|word| name.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_redacted(input: &str, secrets: &[&str]) {
        let output = redact(input);
        for secret in secrets {
            assert!(
                !output.contains(secret),
                "{secret:?} survived in {output:?} (from {input:?})"
            );
        }
        assert!(
            output.contains(LOG_REDACTED),
            "{output:?} has no marker (from {input:?})"
        );
    }

    #[test]
    fn redacts_every_secret_shape() {
        let telegram = "AAE_x1Yz2Wv3Ut4Sr5Qp6On7Ml8Kj9Ih0GfEd";
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let cases: Vec<(String, Vec<&str>)> = vec![
            (
                "request header Authorization: Bearer abc.def-ghi".into(),
                vec!["abc.def-ghi"],
            ),
            (
                "Proxy-Authorization: Basic dXNlcjpwYXNzd29yZA==".into(),
                vec!["dXNlcjpwYXNzd29yZA"],
            ),
            (
                "sent Bearer opaqueAccessValue77 upstream".into(),
                vec!["opaqueAccessValue77"],
            ),
            (
                "api_key=sk-ant-api03-AbCdEfGhIjKlMnOp".into(),
                vec!["sk-ant-api03-AbCdEfGhIjKlMnOp", "AbCdEfGhIjKlMnOp"],
            ),
            (
                r#"body {"access_token":"ya29.accessvalue","refresh_token":"1//refreshvalue"}"#
                    .into(),
                vec!["ya29.accessvalue", "1//refreshvalue"],
            ),
            (
                r#"config {"apiKey":"plainkeyvalue","token":"plaintokenvalue","secret":"plainsecretvalue"}"#
                    .into(),
                vec!["plainkeyvalue", "plaintokenvalue", "plainsecretvalue"],
            ),
            (
                r#"error body "{\"client_secret\":\"escapedsecretvalue\"}""#.into(),
                vec!["escapedsecretvalue"],
            ),
            ("client_secret=GOCSPX-clientvalue".into(), vec!["GOCSPX-clientvalue"]),
            ("password: hunter2hunter2".into(), vec!["hunter2hunter2"]),
            (
                "Cookie: session_id=cookievalue123; theme=dark".into(),
                vec!["cookievalue123"],
            ),
            (
                "redirect https://x/cb?code=oauthcode123&state=statevalue456&ok=1".into(),
                vec!["oauthcode123", "statevalue456"],
            ),
            (
                "fetch https://api.example.com/v1?key=querykeyvalue&api_key=other".into(),
                vec!["querykeyvalue"],
            ),
            (
                format!(
                    "request to https://api.telegram.org/bot123456789:{telegram}/getUpdates failed: timeout"
                ),
                vec![telegram, "123456789"],
            ),
            (format!("token value 987654321:{telegram} pasted"), vec![telegram]),
            (
                "key sk-proj-AbCdEfGh12345678 used".into(),
                vec!["sk-proj-AbCdEfGh12345678"],
            ),
            ("key sk-abcdefgh12345678".into(), vec!["sk-abcdefgh12345678"]),
            (
                "google AIzaSyA1b2C3d4E5f6G7h8I9j0 rejected".into(),
                vec!["AIzaSyA1b2C3d4E5f6G7h8I9j0"],
            ),
            (
                "github ghp_abcdefghijklmnop1234 expired".into(),
                vec!["ghp_abcdefghijklmnop1234"],
            ),
            (
                "slack xoxb-123456789012-abcdefghijkl failed".into(),
                vec!["xoxb-123456789012-abcdefghijkl"],
            ),
            (
                "google ya29.a0AfH6SMBx-accessvalue expired".into(),
                vec!["ya29.a0AfH6SMBx-accessvalue"],
            ),
            ("aws AKIAIOSFODNN7EXAMPLE".into(), vec!["AKIAIOSFODNN7EXAMPLE"]),
            (format!("id token {jwt}"), vec![jwt, "eyJhbGciOiJIUzI1NiJ9"]),
            (
                "opaque Q7wErTy9uIoP3aSdF6gHjK2lZxCvB8nM4qWeRt5Y seen".into(),
                vec!["Q7wErTy9uIoP3aSdF6gHjK2lZxCvB8nM4qWeRt5Y"],
            ),
        ];
        for (input, secrets) in &cases {
            assert_redacted(input, secrets);
        }
    }

    #[test]
    fn keeps_the_label_and_the_rest_of_the_line() {
        assert_eq!(redact("token=abc other=1"), "token=[redacted] other=1");
        assert_eq!(
            redact("Authorization: Bearer abc.def-ghi done"),
            "Authorization: Bearer [redacted] done"
        );
        assert_eq!(
            redact(r#"{"access_token":"abc","n":1}"#),
            r#"{"access_token":"[redacted]","n":1}"#
        );
        assert_eq!(
            redact("https://x/cb?code=abc&ok=1"),
            "https://x/cb?code=[redacted]&ok=1"
        );
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        for text in [
            "prompt_tokens=120 completion_tokens=45 max_tokens=2000 token_count=3",
            "run_3f2b8c0e-1111-4222-8333-444455556666 finished",
            "sha 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
            r"C:\Users\leoca\OneDrive\Desktop\anima\animaos-kit\target\debug",
            "the token expired, so the daemon asks for a new one",
            "local owner authorization required",
        ] {
            assert_eq!(redact(text), text);
        }
    }

    #[test]
    fn redaction_is_idempotent() {
        for text in [
            "Authorization: Bearer abc.def-ghi",
            r#"{"access_token":"abc","refresh_token":"def"}"#,
            r#""{\"client_secret\":\"abc\"}""#,
            "Cookie: session_id=abc; other=1",
            "https://x/cb?code=abc&state=def",
            "password: 'hunter2'",
            "key sk-ant-api03-AbCdEfGhIjKl and Q7wErTy9uIoP3aSdF6gHjK2lZxCvB8nM4qWeRt5Y",
        ] {
            let once = redact(text);
            assert_eq!(redact(&once), once, "from {text:?}");
        }
    }

    #[test]
    fn case_insensitive_labels() {
        assert_redacted("API_KEY=abcdef123", &["abcdef123"]);
        assert_redacted("Password: hunter2", &["hunter2"]);
        assert_redacted("AUTHORIZATION: bearer abc.def", &["abc.def"]);
        assert_redacted("X-Api-Key: headervalue", &["headervalue"]);
        assert_redacted("https://x/?ACCESS_TOKEN=abc123", &["abc123"]);
    }

    #[test]
    fn several_secrets_on_one_line() {
        let output = redact(
            "api_key=first1 then Bearer secondvalue22 then ghp_thirdvalue3333 then password=fourth4",
        );
        for secret in ["first1", "secondvalue22", "ghp_thirdvalue3333", "fourth4"] {
            assert!(!output.contains(secret), "{output}");
        }
        assert_eq!(output.matches(LOG_REDACTED).count(), 4, "{output}");
    }

    #[test]
    fn redaction_is_linear_on_large_input() {
        for unit in ["=", "a", "token=", "Aa1"] {
            let input = unit.repeat(65_536 / unit.len());
            let output = redact(&input);
            assert!(output.len() <= input.len(), "{unit}");
        }
    }

    #[test]
    fn is_secret_field_names() {
        for name in [
            "api_key",
            "X-Api-Key",
            "access_token",
            "client_secret",
            "Authorization",
            "password",
            "cookie",
            "signature",
        ] {
            assert!(is_secret_field(name), "{name}");
        }
        for name in [
            "prompt_tokens",
            "max_tokens",
            "session",
            "agent_id",
            "message",
        ] {
            assert!(!is_secret_field(name), "{name}");
        }
    }
}
