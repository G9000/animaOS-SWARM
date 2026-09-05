//! RFC 2047 subject folding and RFC 2045 base64 line wrapping for Gmail raw MIME.
use base64::{engine::general_purpose::STANDARD, Engine};

pub(super) fn serialize(to: &[String], subject: &str, body: &str) -> String {
    // 42 UTF-8 bytes become at most 56 base64 characters. With the encoded-word
    // delimiters and "Subject: " this keeps the first line under 78 columns.
    let mut words = Vec::new();
    let mut chunk = String::new();
    for character in subject.chars() {
        if chunk.len() + character.len_utf8() > 42 {
            words.push(format!("=?UTF-8?B?{}?=", STANDARD.encode(chunk.as_bytes())));
            chunk.clear();
        }
        chunk.push(character);
    }
    if !chunk.is_empty() {
        words.push(format!("=?UTF-8?B?{}?=", STANDARD.encode(chunk.as_bytes())));
    }
    // DraftInput validates each address before this serializer is called. Folding
    // between addresses preserves mailbox syntax and the 998-byte hard line limit.
    let mut mime = format!(
        "To: {}\r\nSubject: {}\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Transfer-Encoding: base64\r\n\r\n",
        to.join(",\r\n "),
        words.join("\r\n "),
    );
    let encoded = STANDARD.encode(body.as_bytes());
    for line in encoded.as_bytes().chunks(76) {
        mime.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
        mime.push_str("\r\n");
    }
    mime
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_unicode_subject_and_body_roundtrip_with_bounded_lines() {
        let subject = "Lunch tomorrow — 明日の予定 🥗 ".repeat(15);
        let body = "Hello, 世界!\nA message with more than one line.\r\n".repeat(300);
        let mime = serialize(
            &["first@example.com".into(), "second@example.com".into()],
            &subject,
            &body,
        );
        let (headers, encoded_body) = mime.split_once("\r\n\r\n").expect("MIME header separator");
        assert!(headers.lines().all(|line| line.len() <= 78));
        let encoded_subject = headers
            .split("Subject: ")
            .nth(1)
            .unwrap()
            .split("\r\nMIME-Version:")
            .next()
            .unwrap();
        let mut decoded_subject = String::new();
        for word in encoded_subject.split_whitespace() {
            assert!(word.len() <= 75);
            let payload = word
                .strip_prefix("=?UTF-8?B?")
                .unwrap()
                .strip_suffix("?=")
                .unwrap();
            // Every individual encoded word must contain complete UTF-8 characters.
            decoded_subject
                .push_str(&String::from_utf8(STANDARD.decode(payload).unwrap()).unwrap());
        }
        assert_eq!(decoded_subject, subject);
        assert!(encoded_body.lines().all(|line| line.len() <= 76));
        let joined = encoded_body.replace("\r\n", "");
        assert_eq!(STANDARD.decode(joined).unwrap(), body.as_bytes());
        assert!(headers.starts_with("To: first@example.com,\r\n second@example.com\r\n"));
    }

    #[test]
    fn long_validated_recipients_and_empty_subject_stay_below_header_limit() {
        let addresses = (0..20)
            .map(|i| format!("{}{}@example.com", "a".repeat(235), i))
            .collect::<Vec<_>>();
        let mime = serialize(&addresses, "", "");
        let (headers, body) = mime.split_once("\r\n\r\n").unwrap();
        assert!(headers.lines().all(|line| line.len() <= 998));
        assert!(headers.contains("\r\nSubject: \r\nMIME-Version:"));
        assert!(body.is_empty());
        for address in addresses {
            assert!(headers.contains(&address));
        }
    }
}
