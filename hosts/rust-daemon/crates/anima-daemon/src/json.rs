use std::collections::BTreeMap;
use std::str;

pub(crate) enum JsonValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

impl JsonValue {
    fn into_object(self) -> Result<BTreeMap<String, JsonValue>, ()> {
        match self {
            Self::Object(object) => Ok(object),
            _ => Err(()),
        }
    }
}

const MAX_NESTING_DEPTH: usize = 128;

pub(crate) struct JsonParser<'a> {
    input: &'a [u8],
    position: usize,
    depth: usize,
}

impl<'a> JsonParser<'a> {
    pub(crate) fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            position: 0,
            depth: 0,
        }
    }

    pub(crate) fn parse_object(mut self) -> Result<BTreeMap<String, JsonValue>, ()> {
        self.skip_whitespace();
        let object = self.parse_value()?.into_object()?;
        self.skip_whitespace();
        if self.position != self.input.len() {
            return Err(());
        }
        Ok(object)
    }

    fn parse_value(&mut self) -> Result<JsonValue, ()> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'"') => self.parse_string().map(JsonValue::String),
            Some(b'-' | b'0'..=b'9') => self.parse_number().map(JsonValue::Number),
            Some(b'[') => self.parse_array().map(JsonValue::Array),
            Some(b'{') => self.parse_object_value().map(JsonValue::Object),
            Some(b'n') => {
                self.expect_literal("null")?;
                Ok(JsonValue::Null)
            }
            Some(b't') => {
                self.expect_literal("true")?;
                Ok(JsonValue::Bool(true))
            }
            Some(b'f') => {
                self.expect_literal("false")?;
                Ok(JsonValue::Bool(false))
            }
            _ => Err(()),
        }
    }

    fn parse_object_value(&mut self) -> Result<BTreeMap<String, JsonValue>, ()> {
        if self.depth >= MAX_NESTING_DEPTH {
            return Err(());
        }
        self.depth += 1;
        self.expect(b'{')?;
        self.skip_whitespace();

        let mut object = BTreeMap::new();
        if self.consume(b'}') {
            self.depth -= 1;
            return Ok(object);
        }

        loop {
            let key = self.parse_string()?;
            self.skip_whitespace();
            self.expect(b':')?;
            let value = self.parse_value()?;
            object.insert(key, value);
            self.skip_whitespace();
            if self.consume(b'}') {
                break;
            }
            self.expect(b',')?;
            self.skip_whitespace();
        }

        self.depth -= 1;
        Ok(object)
    }

    fn parse_array(&mut self) -> Result<Vec<JsonValue>, ()> {
        if self.depth >= MAX_NESTING_DEPTH {
            return Err(());
        }
        self.depth += 1;
        self.expect(b'[')?;
        self.skip_whitespace();

        let mut values = Vec::new();
        if self.consume(b']') {
            self.depth -= 1;
            return Ok(values);
        }

        loop {
            values.push(self.parse_value()?);
            self.skip_whitespace();
            if self.consume(b']') {
                break;
            }
            self.expect(b',')?;
            self.skip_whitespace();
        }

        self.depth -= 1;
        Ok(values)
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
                            output.extend_from_slice(codepoint.encode_utf8(&mut buffer).as_bytes());
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

    fn parse_number(&mut self) -> Result<f64, ()> {
        let start = self.position;
        if self.consume(b'-') {
            // optional minus
        }

        // Integer part: no leading zeros unless the value is just `0`.
        match self.peek() {
            Some(b'0') => {
                self.position += 1;
                // After a leading 0 the next char must NOT be a digit.
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(());
                }
            }
            Some(b'1'..=b'9') => {
                self.position += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.position += 1;
                }
            }
            _ => return Err(()), // lone minus or empty
        }

        // Fractional part — at least one digit required after the dot.
        if self.consume(b'.') {
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(());
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.position += 1;
            }
        }

        // Exponent part — at least one digit required.
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.position += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.position += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(());
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.position += 1;
            }
        }

        let value = str::from_utf8(&self.input[start..self.position]).map_err(|_| ())?;
        value.parse::<f64>().map_err(|_| ())
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

    fn expect_literal(&mut self, literal: &str) -> Result<(), ()> {
        let bytes = literal.as_bytes();
        if self
            .input
            .get(self.position..)
            .map_or(false, |rest| rest.starts_with(bytes))
        {
            self.position += bytes.len();
            Ok(())
        } else {
            Err(())
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

pub(crate) fn escape_json(value: &str) -> String {
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
