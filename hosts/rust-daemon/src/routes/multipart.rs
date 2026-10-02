//! A strict `multipart/form-data` reader for small uploads (spec §8.4's skill
//! import; M9's attachments reuse it with their own [`FormLimits`]). It takes
//! the boundary from the request's content type, expects CRLF line ends and
//! no preamble, and names each part by its `Content-Disposition: form-data;
//! name="…"` header, with an optional `filename`.
//!
//! The reader is bounded in every dimension before it copies anything: the
//! whole body, the number of parts, each part's header block, and each part's
//! content. The closing delimiter is found with one linear pass (a
//! Knuth-Morris-Pratt search that keeps its state across false starts), so
//! hostile content cannot make it quadratic. A `;` or an escaped quote
//! inside a quoted parameter is read as part of its value.

/// Parts one form may carry by default.
pub(crate) const MAX_FORM_PARTS: usize = 8;
/// The header block of one part (its `Content-Disposition` and `Content-Type`).
pub(crate) const MAX_FORM_HEADER_BYTES: usize = 2 * 1024;
pub(crate) const FORM_NOT_MULTIPART: &str = "expected multipart/form-data with a boundary";
pub(crate) const FORM_MALFORMED: &str = "malformed multipart body";
pub(crate) const FORM_TOO_MANY_PARTS: &str = "too many form parts";
pub(crate) const FORM_TOO_LARGE: &str = "multipart body too large";

/// What one form may hold; anything over a bound is refused, not truncated.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FormLimits {
    pub(crate) max_total_bytes: usize,
    pub(crate) max_parts: usize,
    pub(crate) max_header_bytes: usize,
    pub(crate) max_part_bytes: usize,
}

impl FormLimits {
    /// For small forms: 128 KiB in all, 64 KiB a part.
    #[cfg_attr(not(test), allow(dead_code))] // routes pass their own limits
    pub(crate) const SMALL: Self = Self {
        max_total_bytes: 128 * 1024,
        max_parts: MAX_FORM_PARTS,
        max_header_bytes: MAX_FORM_HEADER_BYTES,
        max_part_bytes: 64 * 1024,
    };
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FormPart {
    pub(crate) name: String,
    #[cfg_attr(not(test), allow(dead_code))] // read by M9's attachments; only tests read it today
    pub(crate) filename: Option<String>,
    pub(crate) bytes: Vec<u8>,
}

/// Splits `value` at the `;`s outside quoted strings. `None`: a quoted
/// string is never closed.
fn split_params(value: &str) -> Option<Vec<&str>> {
    let mut segments = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0, false, false);
    for (index, ch) in value.char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = false;
            }
        } else if ch == '"' {
            quoted = true;
        } else if ch == ';' {
            segments.push(&value[start..index]);
            start = index + 1;
        }
    }
    if quoted {
        return None;
    }
    segments.push(&value[start..]);
    Some(segments)
}

/// A parameter's value: a token, or a quoted string with `\` escapes.
fn param_value(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let Some(rest) = raw.strip_prefix('"') else {
        return (!raw.contains('"')).then(|| raw.to_string());
    };
    let inner = rest.strip_suffix('"')?;
    let (mut value, mut escaped) = (String::new(), false);
    for ch in inner.chars() {
        if escaped {
            value.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return None;
        } else {
            value.push(ch);
        }
    }
    (!escaped).then_some(value)
}

/// RFC 2046's `bchars`; a boundary may not end in a space.
fn is_boundary_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || "'()+_,-./:=? ".contains(ch)
}

/// The boundary of a `multipart/form-data` content type (RFC 2046: 1–70
/// characters of `bchars`).
pub(crate) fn boundary(content_type: &str) -> Option<String> {
    let mut segments = split_params(content_type)?.into_iter();
    if !segments
        .next()?
        .trim()
        .eq_ignore_ascii_case("multipart/form-data")
    {
        return None;
    }
    let raw = segments.find_map(|segment| {
        let (key, value) = segment.split_once('=')?;
        key.trim().eq_ignore_ascii_case("boundary").then_some(value)
    })?;
    param_value(raw).filter(|boundary| {
        (1..=70).contains(&boundary.len())
            && boundary.chars().all(is_boundary_char)
            && !boundary.ends_with(' ')
    })
}

/// A linear substring search that resumes where its last match ended
/// (Knuth-Morris-Pratt), so a delimiter-like text inside a part costs one
/// step per byte, however often it repeats.
struct Searcher {
    needle: Vec<u8>,
    fail: Vec<usize>,
    matched: usize,
}

impl Searcher {
    fn new(needle: Vec<u8>) -> Self {
        let mut fail = vec![0; needle.len()];
        let mut known = 0;
        for index in 1..needle.len() {
            while known > 0 && needle[index] != needle[known] {
                known = fail[known - 1];
            }
            if needle[index] == needle[known] {
                known += 1;
            }
            fail[index] = known;
        }
        Self {
            needle,
            fail,
            matched: 0,
        }
    }

    /// Forgets any partial match: the next search starts a fresh text.
    fn reset(&mut self) {
        self.matched = 0;
    }

    /// The index just past the next match at or after `from`. Calling again
    /// with that index finds the match after it.
    fn next_end(&mut self, haystack: &[u8], from: usize) -> Option<usize> {
        for (offset, &byte) in haystack.get(from..)?.iter().enumerate() {
            while self.matched > 0 && self.needle[self.matched] != byte {
                self.matched = self.fail[self.matched - 1];
            }
            if self.needle[self.matched] == byte {
                self.matched += 1;
            }
            if self.matched == self.needle.len() {
                self.matched = self.fail[self.matched - 1];
                return Some(from + offset + 1);
            }
        }
        None
    }
}

/// `name` and `filename` from a part's headers; the part must be
/// `Content-Disposition: form-data` with a `name`.
fn disposition(headers: &str) -> Option<(String, Option<String>)> {
    let mut value = None;
    for line in headers.split("\r\n") {
        if line.contains(['\r', '\n']) {
            return None;
        }
        let (key, rest) = line.split_once(':')?;
        if value.is_none() && key.trim().eq_ignore_ascii_case("content-disposition") {
            value = Some(rest);
        }
    }
    let mut segments = split_params(value?)?.into_iter();
    if !segments.next()?.trim().eq_ignore_ascii_case("form-data") {
        return None;
    }
    let (mut name, mut filename) = (None, None);
    for segment in segments {
        if segment.trim().is_empty() {
            continue;
        }
        let (key, raw) = segment.split_once('=')?;
        let value = param_value(raw)?;
        match key.trim().to_ascii_lowercase().as_str() {
            "name" => name = Some(value),
            "filename" => filename = Some(value),
            _ => {}
        }
    }
    Some((name?, filename))
}

/// Every part of a form, in order, within [`FormLimits::SMALL`].
#[cfg_attr(not(test), allow(dead_code))] // routes pass their own limits
pub(crate) fn parse_form(content_type: &str, body: &[u8]) -> Result<Vec<FormPart>, &'static str> {
    parse_form_with(content_type, body, &FormLimits::SMALL)
}

/// Every part of a form, in order, within `limits`.
pub(crate) fn parse_form_with(
    content_type: &str,
    body: &[u8],
    limits: &FormLimits,
) -> Result<Vec<FormPart>, &'static str> {
    let boundary = boundary(content_type).ok_or(FORM_NOT_MULTIPART)?;
    if body.len() > limits.max_total_bytes {
        return Err(FORM_TOO_LARGE);
    }
    let delimiter = format!("--{boundary}");
    if !body.starts_with(delimiter.as_bytes()) {
        return Err(FORM_MALFORMED);
    }
    let mut closing = Searcher::new(format!("\r\n{delimiter}").into_bytes());
    let mut position = delimiter.len();
    let mut parts = Vec::new();
    loop {
        // `position` is just past a delimiter.
        let rest = &body[position..];
        if let Some(tail) = rest.strip_prefix(b"--") {
            return if tail.is_empty() || tail == b"\r\n" {
                Ok(parts)
            } else {
                Err(FORM_MALFORMED)
            };
        }
        let Some(headers) = rest.strip_prefix(b"\r\n") else {
            return Err(FORM_MALFORMED);
        };
        if parts.len() >= limits.max_parts {
            return Err(FORM_TOO_MANY_PARTS);
        }
        // A part without headers has no name.
        if headers.starts_with(b"\r\n") {
            return Err(FORM_MALFORMED);
        }
        let headers_start = body.len() - headers.len();
        let window_end = body.len().min(headers_start + limits.max_header_bytes + 4);
        let window = &body[headers_start..window_end];
        let Some(headers_len) = window.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
            return Err(if window_end < body.len() {
                FORM_TOO_LARGE
            } else {
                FORM_MALFORMED
            });
        };
        let headers = std::str::from_utf8(&window[..headers_len]).map_err(|_| FORM_MALFORMED)?;
        let (name, filename) = disposition(headers).ok_or(FORM_MALFORMED)?;

        let content_start = headers_start + headers_len + 4;
        let window_end = body
            .len()
            .min(content_start + limits.max_part_bytes + closing.needle.len());
        let window = &body[..window_end];
        closing.reset();
        let mut search_from = content_start;
        let content_end = loop {
            let Some(end) = closing.next_end(window, search_from) else {
                return Err(if window_end < body.len() {
                    FORM_TOO_LARGE
                } else {
                    FORM_MALFORMED
                });
            };
            // A real delimiter ends its line or closes the form; text such
            // as `\r\n--boundary-extra` inside the part is content.
            let after = &body[end..];
            if after.starts_with(b"\r\n") || after.starts_with(b"--") {
                position = end;
                break end - closing.needle.len();
            }
            search_from = end;
        };
        parts.push(FormPart {
            name,
            filename,
            bytes: body[content_start..content_end].to_vec(),
        });
    }
}

/// A form as a browser sends it, for tests: `(name, filename, bytes)` parts.
#[cfg(test)]
pub(crate) fn encode_form(boundary: &str, parts: &[(&str, Option<&str>, &[u8])]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, filename, bytes) in parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        let disposition = match filename {
            Some(filename) => {
                format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: text/markdown\r\n")
            }
            None => format!("Content-Disposition: form-data; name=\"{name}\"\r\n"),
        };
        body.extend_from_slice(disposition.as_bytes());
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content_type(boundary: &str) -> String {
        format!("multipart/form-data; boundary={boundary}")
    }

    #[test]
    fn the_boundary_comes_from_the_content_type() {
        assert_eq!(
            boundary("multipart/form-data; boundary=abc").as_deref(),
            Some("abc")
        );
        assert_eq!(
            boundary("Multipart/Form-Data; charset=utf-8; boundary=\"q b\"").as_deref(),
            Some("q b")
        );
        assert_eq!(boundary("text/plain; boundary=abc"), None);
        assert_eq!(boundary("multipart/form-data"), None);
        assert_eq!(
            boundary(&format!("multipart/form-data; boundary={}", "b".repeat(71))),
            None
        );
        assert_eq!(boundary("multipart/form-data; boundary=\"a b \""), None);
        assert_eq!(boundary("multipart/form-data; boundary=a\"b"), None);
        assert_eq!(
            boundary("multipart/form-data; boundary=\"unterminated"),
            None
        );
    }

    #[test]
    fn a_form_with_a_file_and_a_field_is_read() {
        let body = encode_form(
            "XyZ",
            &[
                (
                    "file",
                    Some("SKILL.md"),
                    &b"---\nname: n\n---\r\n\r\nbody\r\n"[..],
                ),
                ("slug", None, &b"chosen"[..]),
                ("empty", None, &b""[..]),
            ],
        );
        let parts = parse_form(&content_type("XyZ"), &body).unwrap();
        assert_eq!(
            parts,
            vec![
                FormPart {
                    name: "file".into(),
                    filename: Some("SKILL.md".into()),
                    bytes: b"---\nname: n\n---\r\n\r\nbody\r\n".to_vec(),
                },
                FormPart {
                    name: "slug".into(),
                    filename: None,
                    bytes: b"chosen".to_vec(),
                },
                FormPart {
                    name: "empty".into(),
                    filename: None,
                    bytes: Vec::new(),
                },
            ]
        );
    }

    #[test]
    fn a_malformed_form_is_refused() {
        assert_eq!(
            parse_form("application/json", b"{}"),
            Err(FORM_NOT_MULTIPART)
        );
        let complete = encode_form("b", &[("file", Some("a.md"), &b"x"[..])]);
        let unterminated = &complete[..complete.len() - 8];
        assert_eq!(
            parse_form(&content_type("b"), unterminated),
            Err(FORM_MALFORMED)
        );
        let nameless = b"--b\r\nContent-Disposition: form-data\r\n\r\nx\r\n--b--\r\n";
        assert_eq!(
            parse_form(&content_type("b"), nameless),
            Err(FORM_MALFORMED)
        );
        let parts: Vec<(&str, Option<&str>, &[u8])> = (0..=MAX_FORM_PARTS)
            .map(|_| ("f", None, &b"x"[..]))
            .collect();
        assert_eq!(
            parse_form(&content_type("b"), &encode_form("b", &parts)),
            Err(FORM_TOO_MANY_PARTS)
        );
    }

    #[test]
    fn framing_that_is_not_strict_is_refused() {
        let ct = content_type("b");
        let disposition = "Content-Disposition: form-data; name=\"f\"\r\n\r\nx";
        for body in [
            // A preamble before the first delimiter.
            format!("junk\r\n--b\r\n{disposition}\r\n--b--\r\n"),
            // No CRLF after the delimiter.
            format!("--bX\r\n{disposition}\r\n--b--\r\n"),
            // Bare LF line ends.
            "--b\nContent-Disposition: form-data; name=\"f\"\n\nx\n--b--\n".to_string(),
            // An epilogue after the closing delimiter.
            format!("--b\r\n{disposition}\r\n--b--\r\ntrailing"),
            // A header line with a bare carriage return.
            "--b\r\nContent-Disposition: form-data; name=\"f\"\r\nX: a\rb\r\n\r\nx\r\n--b--\r\n"
                .to_string(),
            // Not form-data.
            "--b\r\nContent-Disposition: attachment; name=\"f\"\r\n\r\nx\r\n--b--\r\n".to_string(),
        ] {
            assert_eq!(
                parse_form(&ct, body.as_bytes()),
                Err(FORM_MALFORMED),
                "{body:?}"
            );
        }
        let mut not_utf8 = b"--b\r\nContent-Disposition: form-data; name=\"f".to_vec();
        not_utf8.extend_from_slice(&[0xff, 0xfe]);
        not_utf8.extend_from_slice(b"\"\r\n\r\nx\r\n--b--\r\n");
        assert_eq!(parse_form(&ct, &not_utf8), Err(FORM_MALFORMED));
        assert_eq!(
            parse_form(&ct, b"--b--\r\n").unwrap(),
            Vec::<FormPart>::new(),
            "an empty form is a form"
        );
        assert_eq!(
            parse_form(&ct, b"--b--").unwrap(),
            Vec::<FormPart>::new(),
            "the final CRLF is optional"
        );
    }

    #[test]
    fn content_that_only_looks_like_a_delimiter_stays_content() {
        let content = b"a\r\n--bold line\r\n--b-x\r\n--\r\n--";
        let body = encode_form("b", &[("file", Some("a.md"), &content[..])]);
        let parts = parse_form(&content_type("b"), &body).unwrap();
        assert_eq!(parts[0].bytes, content.to_vec());
    }

    #[test]
    fn a_quoted_parameter_may_hold_a_semicolon_and_an_escaped_quote() {
        let body = b"--b\r\nContent-Disposition: form-data; name=\"fi;le\"; filename=\"a;\\\"b.md\"\r\n\r\nx\r\n--b--\r\n";
        let parts = parse_form(&content_type("b"), body).unwrap();
        assert_eq!(parts[0].name, "fi;le");
        assert_eq!(parts[0].filename.as_deref(), Some("a;\"b.md"));
        let unterminated = b"--b\r\nContent-Disposition: form-data; name=\"f\r\n\r\nx\r\n--b--\r\n";
        assert_eq!(
            parse_form(&content_type("b"), unterminated),
            Err(FORM_MALFORMED)
        );
    }

    #[test]
    fn the_limits_are_enforced() {
        let limits = FormLimits {
            max_total_bytes: 400,
            max_parts: 3,
            max_header_bytes: 120,
            max_part_bytes: 50,
        };
        let ct = content_type("b");
        let small = encode_form("b", &[("f", None, &[b'x'; 50][..])]);
        assert_eq!(parse_form_with(&ct, &small, &limits).unwrap().len(), 1);

        let part = encode_form("b", &[("f", None, &[b'x'; 51][..])]);
        assert_eq!(parse_form_with(&ct, &part, &limits), Err(FORM_TOO_LARGE));
        // An unterminated oversized part stops at the part bound too.
        let mut open = b"--b\r\nContent-Disposition: form-data; name=\"f\"\r\n\r\n".to_vec();
        open.extend_from_slice(&[b'x'; 300]);
        assert_eq!(parse_form_with(&ct, &open, &limits), Err(FORM_TOO_LARGE));

        let three = encode_form("b", &[("f", None, &[b'x'; 40][..]); 3]);
        assert_eq!(
            parse_form_with(&ct, &three, &limits).map(|parts| parts.len()),
            Ok(3)
        );
        let mut padded = three.clone();
        padded.extend_from_slice(&[b' '; 400]);
        assert_eq!(parse_form_with(&ct, &padded, &limits), Err(FORM_TOO_LARGE));

        let long_name = "n".repeat(130);
        let header = encode_form("b", &[(long_name.as_str(), None, &b"x"[..])]);
        assert_eq!(parse_form_with(&ct, &header, &limits), Err(FORM_TOO_LARGE));
        let mut endless = b"--b\r\nContent-Disposition: form-data; name=\"f\"; ".to_vec();
        endless.extend_from_slice(&[b'a'; 300]);
        assert_eq!(parse_form_with(&ct, &endless, &limits), Err(FORM_TOO_LARGE));

        let four: Vec<(&str, Option<&str>, &[u8])> =
            (0..4).map(|_| ("f", None, &b"x"[..])).collect();
        assert_eq!(
            parse_form_with(&ct, &encode_form("b", &four), &limits),
            Err(FORM_TOO_MANY_PARTS)
        );
    }

    #[test]
    fn a_hostile_boundary_search_stays_linear() {
        // Content that repeats all but the last byte of the delimiter would
        // cost a naive search `content × delimiter` comparisons.
        let boundary = "a".repeat(70);
        let near_miss = format!("\r\n--{}", "a".repeat(69));
        let content = near_miss.repeat(1500);
        let body = encode_form(&boundary, &[("file", Some("a.md"), content.as_bytes())]);
        let limits = FormLimits {
            max_total_bytes: 256 * 1024,
            max_parts: 8,
            max_header_bytes: 1024,
            max_part_bytes: 256 * 1024,
        };
        let started = std::time::Instant::now();
        let parts = parse_form_with(&content_type(&boundary), &body, &limits).unwrap();
        assert_eq!(parts[0].bytes, content.as_bytes());
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }
}
