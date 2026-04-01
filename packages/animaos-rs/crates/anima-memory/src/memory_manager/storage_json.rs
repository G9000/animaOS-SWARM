use std::str;

use super::{validate_importance, Memory, MemoryType};

pub(super) fn serialize_memories(memories: &[Memory]) -> String {
    let mut output = String::from("[\n");
    for (index, memory) in memories.iter().enumerate() {
        if index > 0 {
            output.push_str(",\n");
        }
        output.push_str("  {");
        push_string_field(&mut output, "id", &memory.id);
        output.push(',');
        push_string_field(&mut output, "agentId", &memory.agent_id);
        output.push(',');
        push_string_field(&mut output, "agentName", &memory.agent_name);
        output.push(',');
        push_string_field(&mut output, "type", memory.memory_type.as_str());
        output.push(',');
        push_string_field(&mut output, "content", &memory.content);
        output.push(',');
        output.push_str(&format!("\"importance\":{}", memory.importance));
        output.push(',');
        output.push_str(&format!("\"createdAt\":{}", memory.created_at));
        output.push(',');
        output.push_str("\"tags\":");
        match &memory.tags {
            None => output.push_str("null"),
            Some(tags) => {
                output.push('[');
                for (tag_index, tag) in tags.iter().enumerate() {
                    if tag_index > 0 {
                        output.push(',');
                    }
                    output.push('"');
                    output.push_str(&escape_json(tag));
                    output.push('"');
                }
                output.push(']');
            }
        }
        output.push('}');
    }
    output.push_str("\n]\n");
    output
}

pub(super) fn deserialize_memories(input: &str) -> Result<Vec<Memory>, ()> {
    JsonParser::new(input).parse_memories()
}

fn push_string_field(output: &mut String, key: &str, value: &str) {
    output.push('"');
    output.push_str(key);
    output.push_str("\":\"");
    output.push_str(&escape_json(value));
    output.push('"');
}

fn escape_json(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                escaped.push_str(&format!("\\u{:04x}", character as u32))
            }
            _ => escaped.push(character),
        }
    }
    escaped
}

struct JsonParser<'a> {
    input: &'a [u8],
    position: usize,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            position: 0,
        }
    }

    fn parse_memories(&mut self) -> Result<Vec<Memory>, ()> {
        self.skip_whitespace();
        self.expect(b'[')?;
        self.skip_whitespace();

        let mut memories = Vec::new();
        if self.consume(b']') {
            return Ok(memories);
        }

        loop {
            memories.push(self.parse_memory()?);
            self.skip_whitespace();
            if self.consume(b']') {
                break;
            }
            self.expect(b',')?;
            self.skip_whitespace();
        }

        self.skip_whitespace();
        if self.position != self.input.len() {
            return Err(());
        }
        Ok(memories)
    }

    fn parse_memory(&mut self) -> Result<Memory, ()> {
        self.skip_whitespace();
        self.expect(b'{')?;

        let mut id = None;
        let mut agent_id = None;
        let mut agent_name = None;
        let mut memory_type = None;
        let mut content = None;
        let mut importance = None;
        let mut created_at = None;
        let mut tags = None;

        loop {
            self.skip_whitespace();
            if self.consume(b'}') {
                break;
            }

            let key = self.parse_string()?;
            self.skip_whitespace();
            self.expect(b':')?;
            self.skip_whitespace();

            match key.as_str() {
                "id" => id = Some(self.parse_string()?),
                "agentId" => agent_id = Some(self.parse_string()?),
                "agentName" => agent_name = Some(self.parse_string()?),
                "type" => memory_type = Some(MemoryType::parse(&self.parse_string()?)?),
                "content" => content = Some(self.parse_string()?),
                "importance" => {
                    let value = self.parse_number()?.parse::<f64>().map_err(|_| ())?;
                    importance = Some(validate_importance(value).map_err(|_| ())?);
                }
                "createdAt" => {
                    created_at = Some(self.parse_number()?.parse::<u128>().map_err(|_| ())?)
                }
                "tags" => tags = Some(self.parse_tags()?),
                _ => return Err(()),
            }

            self.skip_whitespace();
            if self.consume(b'}') {
                break;
            }
            self.expect(b',')?;
        }

        Ok(Memory {
            id: id.ok_or(())?,
            agent_id: agent_id.ok_or(())?,
            agent_name: agent_name.ok_or(())?,
            memory_type: memory_type.ok_or(())?,
            content: content.ok_or(())?,
            importance: importance.ok_or(())?,
            created_at: created_at.ok_or(())?,
            tags: tags.ok_or(())?,
        })
    }

    fn parse_tags(&mut self) -> Result<Option<Vec<String>>, ()> {
        if self.consume_literal("null") {
            return Ok(None);
        }

        self.expect(b'[')?;
        self.skip_whitespace();

        let mut tags = Vec::new();
        if self.consume(b']') {
            return Ok(Some(tags));
        }

        loop {
            tags.push(self.parse_string()?);
            self.skip_whitespace();
            if self.consume(b']') {
                break;
            }
            self.expect(b',')?;
            self.skip_whitespace();
        }

        Ok(Some(tags))
    }

    fn parse_string(&mut self) -> Result<String, ()> {
        self.expect(b'"')?;
        let mut output = Vec::new();

        while let Some(byte) = self.peek() {
            self.position += 1;
            match byte {
                b'"' => return String::from_utf8(output).map_err(|_| ()),
                b'\\' => {
                    let Some(escaped) = self.peek() else {
                        return Err(());
                    };
                    self.position += 1;
                    match escaped {
                        b'"' => output.push(b'"'),
                        b'\\' => output.push(b'\\'),
                        b'/' => output.push(b'/'),
                        b'b' => output.push(8),
                        b'f' => output.push(12),
                        b'n' => output.push(b'\n'),
                        b'r' => output.push(b'\r'),
                        b't' => output.push(b'\t'),
                        b'u' => {
                            let codepoint = self.parse_unicode_scalar()?;
                            let mut buffer = [0_u8; 4];
                            let encoded = codepoint.encode_utf8(&mut buffer);
                            output.extend_from_slice(encoded.as_bytes());
                        }
                        _ => return Err(()),
                    }
                }
                byte if byte < 0x20 => return Err(()),
                byte => output.push(byte),
            }
        }

        Err(())
    }

    fn parse_unicode_scalar(&mut self) -> Result<char, ()> {
        let first = self.parse_unicode_code_unit()?;
        match first {
            0xD800..=0xDBFF => {
                self.expect(b'\\')?;
                self.expect(b'u')?;
                let second = self.parse_unicode_code_unit()?;
                if !(0xDC00..=0xDFFF).contains(&second) {
                    return Err(());
                }
                let scalar =
                    0x1_0000 + ((u32::from(first - 0xD800) << 10) | u32::from(second - 0xDC00));
                char::from_u32(scalar).ok_or(())
            }
            0xDC00..=0xDFFF => Err(()),
            value => char::from_u32(u32::from(value)).ok_or(()),
        }
    }

    fn parse_unicode_code_unit(&mut self) -> Result<u16, ()> {
        let mut value = 0_u16;
        for _ in 0..4 {
            let Some(byte) = self.peek() else {
                return Err(());
            };
            self.position += 1;
            value = (value << 4)
                | match byte {
                    b'0'..=b'9' => u16::from(byte - b'0'),
                    b'a'..=b'f' => u16::from(byte - b'a' + 10),
                    b'A'..=b'F' => u16::from(byte - b'A' + 10),
                    _ => return Err(()),
                };
        }

        Ok(value)
    }

    fn parse_number(&mut self) -> Result<String, ()> {
        let start = self.position;
        while let Some(byte) = self.peek() {
            if byte.is_ascii_digit() || matches!(byte, b'.' | b'-') {
                self.position += 1;
            } else {
                break;
            }
        }

        if start == self.position {
            return Err(());
        }

        str::from_utf8(&self.input[start..self.position])
            .map(|value| value.to_string())
            .map_err(|_| ())
    }

    fn consume_literal(&mut self, literal: &str) -> bool {
        let bytes = literal.as_bytes();
        if self.input[self.position..].starts_with(bytes) {
            self.position += bytes.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), ()> {
        if self.consume(expected) {
            Ok(())
        } else {
            Err(())
        }
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.position).copied()
    }

    fn skip_whitespace(&mut self) {
        while let Some(byte) = self.peek() {
            if byte.is_ascii_whitespace() {
                self.position += 1;
            } else {
                break;
            }
        }
    }
}
