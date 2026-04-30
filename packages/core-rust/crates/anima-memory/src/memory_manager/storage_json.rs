use std::str;

use super::storage::MemoryStore;
use super::{
    validate_importance, AgentRelationship, Memory, MemoryEntity, MemoryScope, MemoryType,
    RelationshipEndpointKind,
};

pub(super) fn serialize_memory_store(
    memories: &[Memory],
    memory_entities: &[MemoryEntity],
    agent_relationships: &[AgentRelationship],
) -> String {
    let mut output = String::from("{\n  \"version\":1,\n  \"memories\":");
    push_memories_array(&mut output, memories, "  ");
    output.push_str(",\n  \"entities\":");
    push_entities_array(&mut output, memory_entities, "  ");
    output.push_str(",\n  \"agentRelationships\":");
    push_relationships_array(&mut output, agent_relationships, "  ");
    output.push_str("\n}\n");
    output
}

fn push_memories_array(output: &mut String, memories: &[Memory], indent: &str) {
    let item_indent = format!("{indent}  ");
    output.push_str("[\n");
    for (index, memory) in memories.iter().enumerate() {
        if index > 0 {
            output.push_str(",\n");
        }
        output.push_str(&item_indent);
        output.push('{');
        push_string_field(output, "id", &memory.id);
        output.push(',');
        push_string_field(output, "agentId", &memory.agent_id);
        output.push(',');
        push_string_field(output, "agentName", &memory.agent_name);
        output.push(',');
        push_string_field(output, "type", memory.memory_type.as_str());
        output.push(',');
        push_string_field(output, "content", &memory.content);
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
        output.push(',');
        push_string_field(output, "scope", memory.scope.as_str());
        output.push(',');
        push_optional_string_field(output, "roomId", memory.room_id.as_deref());
        output.push(',');
        push_optional_string_field(output, "worldId", memory.world_id.as_deref());
        output.push(',');
        push_optional_string_field(output, "sessionId", memory.session_id.as_deref());
        output.push('}');
    }
    output.push('\n');
    output.push_str(indent);
    output.push(']');
}

fn push_entities_array(output: &mut String, entities: &[MemoryEntity], indent: &str) {
    let item_indent = format!("{indent}  ");
    output.push_str("[\n");
    for (index, entity) in entities.iter().enumerate() {
        if index > 0 {
            output.push_str(",\n");
        }
        output.push_str(&item_indent);
        output.push('{');
        push_string_field(output, "kind", entity.kind.as_str());
        output.push(',');
        push_string_field(output, "id", &entity.id);
        output.push(',');
        push_string_field(output, "name", &entity.name);
        output.push(',');
        push_string_array_field(output, "aliases", &entity.aliases);
        output.push(',');
        push_optional_string_field(output, "summary", entity.summary.as_deref());
        output.push(',');
        output.push_str(&format!("\"createdAt\":{}", entity.created_at));
        output.push(',');
        output.push_str(&format!("\"updatedAt\":{}", entity.updated_at));
        output.push('}');
    }
    output.push('\n');
    output.push_str(indent);
    output.push(']');
}

fn push_relationships_array(
    output: &mut String,
    relationships: &[AgentRelationship],
    indent: &str,
) {
    let item_indent = format!("{indent}  ");
    output.push_str("[\n");
    for (index, relationship) in relationships.iter().enumerate() {
        if index > 0 {
            output.push_str(",\n");
        }
        output.push_str(&item_indent);
        output.push('{');
        push_string_field(output, "id", &relationship.id);
        output.push(',');
        push_string_field(output, "sourceKind", relationship.source_kind.as_str());
        output.push(',');
        push_string_field(output, "sourceAgentId", &relationship.source_agent_id);
        output.push(',');
        push_string_field(output, "sourceAgentName", &relationship.source_agent_name);
        output.push(',');
        push_string_field(output, "targetKind", relationship.target_kind.as_str());
        output.push(',');
        push_string_field(output, "targetAgentId", &relationship.target_agent_id);
        output.push(',');
        push_string_field(output, "targetAgentName", &relationship.target_agent_name);
        output.push(',');
        push_string_field(output, "relationshipType", &relationship.relationship_type);
        output.push(',');
        push_optional_string_field(output, "summary", relationship.summary.as_deref());
        output.push(',');
        output.push_str(&format!("\"strength\":{}", relationship.strength));
        output.push(',');
        output.push_str(&format!("\"confidence\":{}", relationship.confidence));
        output.push(',');
        push_string_array_field(
            output,
            "evidenceMemoryIds",
            &relationship.evidence_memory_ids,
        );
        output.push(',');
        push_optional_string_array_field(output, "tags", relationship.tags.as_deref());
        output.push(',');
        push_optional_string_field(output, "roomId", relationship.room_id.as_deref());
        output.push(',');
        push_optional_string_field(output, "worldId", relationship.world_id.as_deref());
        output.push(',');
        push_optional_string_field(output, "sessionId", relationship.session_id.as_deref());
        output.push(',');
        output.push_str(&format!("\"createdAt\":{}", relationship.created_at));
        output.push(',');
        output.push_str(&format!("\"updatedAt\":{}", relationship.updated_at));
        output.push('}');
    }
    output.push('\n');
    output.push_str(indent);
    output.push(']');
}

pub(super) fn deserialize_memory_store(input: &str) -> Result<MemoryStore, ()> {
    JsonParser::new(input).parse_memory_store()
}

fn push_string_field(output: &mut String, key: &str, value: &str) {
    output.push('"');
    output.push_str(key);
    output.push_str("\":\"");
    output.push_str(&escape_json(value));
    output.push('"');
}

fn push_optional_string_field(output: &mut String, key: &str, value: Option<&str>) {
    output.push('"');
    output.push_str(key);
    output.push_str("\":");
    match value {
        Some(value) => {
            output.push('"');
            output.push_str(&escape_json(value));
            output.push('"');
        }
        None => output.push_str("null"),
    }
}

fn push_string_array_field(output: &mut String, key: &str, values: &[String]) {
    output.push('"');
    output.push_str(key);
    output.push_str("\":");
    push_string_array(output, values);
}

fn push_optional_string_array_field(output: &mut String, key: &str, values: Option<&[String]>) {
    output.push('"');
    output.push_str(key);
    output.push_str("\":");
    match values {
        Some(values) => push_string_array(output, values),
        None => output.push_str("null"),
    }
}

fn push_string_array(output: &mut String, values: &[String]) {
    output.push('[');
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push('"');
        output.push_str(&escape_json(value));
        output.push('"');
    }
    output.push(']');
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

    fn parse_memory_store(&mut self) -> Result<MemoryStore, ()> {
        self.skip_whitespace();
        if self.peek() == Some(b'[') {
            let memories = self.parse_memory_array()?;
            self.expect_end()?;
            return Ok(MemoryStore {
                memories,
                memory_entities: Vec::new(),
                agent_relationships: Vec::new(),
            });
        }

        self.expect(b'{')?;
        self.skip_whitespace();

        let mut memories = None;
        let mut memory_entities = None;
        let mut agent_relationships = None;
        if self.consume(b'}') {
            return Ok(MemoryStore {
                memories: Vec::new(),
                memory_entities: Vec::new(),
                agent_relationships: Vec::new(),
            });
        }

        loop {
            let key = self.parse_string()?;
            self.skip_whitespace();
            self.expect(b':')?;
            self.skip_whitespace();

            match key.as_str() {
                "version" => {
                    self.parse_number()?;
                }
                "memories" => memories = Some(self.parse_memory_array()?),
                "entities" => memory_entities = Some(self.parse_entity_array()?),
                "agentRelationships" => {
                    agent_relationships = Some(self.parse_relationship_array()?)
                }
                _ => return Err(()),
            }

            self.skip_whitespace();
            if self.consume(b'}') {
                break;
            }
            self.expect(b',')?;
            self.skip_whitespace();
        }

        self.expect_end()?;
        Ok(MemoryStore {
            memories: memories.unwrap_or_default(),
            memory_entities: memory_entities.unwrap_or_default(),
            agent_relationships: agent_relationships.unwrap_or_default(),
        })
    }

    fn parse_memory_array(&mut self) -> Result<Vec<Memory>, ()> {
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

        Ok(memories)
    }

    fn parse_entity_array(&mut self) -> Result<Vec<MemoryEntity>, ()> {
        self.expect(b'[')?;
        self.skip_whitespace();

        let mut entities = Vec::new();
        if self.consume(b']') {
            return Ok(entities);
        }

        loop {
            entities.push(self.parse_entity()?);
            self.skip_whitespace();
            if self.consume(b']') {
                break;
            }
            self.expect(b',')?;
            self.skip_whitespace();
        }

        Ok(entities)
    }

    fn parse_entity(&mut self) -> Result<MemoryEntity, ()> {
        self.skip_whitespace();
        self.expect(b'{')?;

        let mut kind = None;
        let mut id = None;
        let mut name = None;
        let mut aliases = None;
        let mut summary = None;
        let mut created_at = None;
        let mut updated_at = None;

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
                "kind" => {
                    kind = Some(
                        RelationshipEndpointKind::from_str(&self.parse_string()?)
                            .map_err(|_| ())?,
                    )
                }
                "id" => id = Some(self.parse_string()?),
                "name" => name = Some(self.parse_string()?),
                "aliases" => aliases = Some(self.parse_string_array()?),
                "summary" => summary = self.parse_optional_string()?,
                "createdAt" => {
                    created_at = Some(self.parse_number()?.parse::<u128>().map_err(|_| ())?)
                }
                "updatedAt" => {
                    updated_at = Some(self.parse_number()?.parse::<u128>().map_err(|_| ())?)
                }
                _ => return Err(()),
            }

            self.skip_whitespace();
            if self.consume(b'}') {
                break;
            }
            self.expect(b',')?;
        }

        Ok(MemoryEntity {
            kind: kind.ok_or(())?,
            id: id.ok_or(())?,
            name: name.ok_or(())?,
            aliases: aliases.unwrap_or_default(),
            summary,
            created_at: created_at.ok_or(())?,
            updated_at: updated_at.ok_or(())?,
        })
    }

    fn parse_relationship_array(&mut self) -> Result<Vec<AgentRelationship>, ()> {
        self.expect(b'[')?;
        self.skip_whitespace();

        let mut relationships = Vec::new();
        if self.consume(b']') {
            return Ok(relationships);
        }

        loop {
            relationships.push(self.parse_relationship()?);
            self.skip_whitespace();
            if self.consume(b']') {
                break;
            }
            self.expect(b',')?;
            self.skip_whitespace();
        }

        Ok(relationships)
    }

    fn parse_relationship(&mut self) -> Result<AgentRelationship, ()> {
        self.skip_whitespace();
        self.expect(b'{')?;

        let mut id = None;
        let mut source_kind = None;
        let mut source_agent_id = None;
        let mut source_agent_name = None;
        let mut target_kind = None;
        let mut target_agent_id = None;
        let mut target_agent_name = None;
        let mut relationship_type = None;
        let mut summary = None;
        let mut strength = None;
        let mut confidence = None;
        let mut evidence_memory_ids = None;
        let mut tags = None;
        let mut room_id = None;
        let mut world_id = None;
        let mut session_id = None;
        let mut created_at = None;
        let mut updated_at = None;

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
                "sourceKind" => {
                    source_kind = Some(
                        RelationshipEndpointKind::from_str(&self.parse_string()?)
                            .map_err(|_| ())?,
                    )
                }
                "sourceAgentId" => source_agent_id = Some(self.parse_string()?),
                "sourceAgentName" => source_agent_name = Some(self.parse_string()?),
                "targetKind" => {
                    target_kind = Some(
                        RelationshipEndpointKind::from_str(&self.parse_string()?)
                            .map_err(|_| ())?,
                    )
                }
                "targetAgentId" => target_agent_id = Some(self.parse_string()?),
                "targetAgentName" => target_agent_name = Some(self.parse_string()?),
                "relationshipType" => relationship_type = Some(self.parse_string()?),
                "summary" => summary = self.parse_optional_string()?,
                "strength" => strength = Some(parse_unit_interval(&self.parse_number()?)?),
                "confidence" => confidence = Some(parse_unit_interval(&self.parse_number()?)?),
                "evidenceMemoryIds" => evidence_memory_ids = Some(self.parse_string_array()?),
                "tags" => tags = self.parse_optional_string_array()?,
                "roomId" => room_id = self.parse_optional_string()?,
                "worldId" => world_id = self.parse_optional_string()?,
                "sessionId" => session_id = self.parse_optional_string()?,
                "createdAt" => {
                    created_at = Some(self.parse_number()?.parse::<u128>().map_err(|_| ())?)
                }
                "updatedAt" => {
                    updated_at = Some(self.parse_number()?.parse::<u128>().map_err(|_| ())?)
                }
                _ => return Err(()),
            }

            self.skip_whitespace();
            if self.consume(b'}') {
                break;
            }
            self.expect(b',')?;
        }

        Ok(AgentRelationship {
            id: id.ok_or(())?,
            source_kind: source_kind.unwrap_or_default(),
            source_agent_id: source_agent_id.ok_or(())?,
            source_agent_name: source_agent_name.ok_or(())?,
            target_kind: target_kind.unwrap_or_default(),
            target_agent_id: target_agent_id.ok_or(())?,
            target_agent_name: target_agent_name.ok_or(())?,
            relationship_type: relationship_type.ok_or(())?,
            summary,
            strength: strength.ok_or(())?,
            confidence: confidence.ok_or(())?,
            evidence_memory_ids: evidence_memory_ids.unwrap_or_default(),
            tags,
            room_id,
            world_id,
            session_id,
            created_at: created_at.ok_or(())?,
            updated_at: updated_at.ok_or(())?,
        })
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
        let mut scope = None;
        let mut room_id = None;
        let mut world_id = None;
        let mut session_id = None;

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
                "scope" => scope = Some(MemoryScope::parse(&self.parse_string()?)?),
                "roomId" => room_id = self.parse_optional_string()?,
                "worldId" => world_id = self.parse_optional_string()?,
                "sessionId" => session_id = self.parse_optional_string()?,
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
            scope: scope.unwrap_or_else(|| default_scope(&room_id)),
            room_id,
            world_id,
            session_id,
        })
    }

    fn parse_optional_string(&mut self) -> Result<Option<String>, ()> {
        if self.consume_literal("null") {
            return Ok(None);
        }
        Ok(Some(self.parse_string()?))
    }

    fn parse_tags(&mut self) -> Result<Option<Vec<String>>, ()> {
        self.parse_optional_string_array()
    }

    fn parse_optional_string_array(&mut self) -> Result<Option<Vec<String>>, ()> {
        if self.consume_literal("null") {
            return Ok(None);
        }

        Ok(Some(self.parse_string_array()?))
    }

    fn parse_string_array(&mut self) -> Result<Vec<String>, ()> {
        self.expect(b'[')?;
        self.skip_whitespace();

        let mut values = Vec::new();
        if self.consume(b']') {
            return Ok(values);
        }

        loop {
            values.push(self.parse_string()?);
            self.skip_whitespace();
            if self.consume(b']') {
                break;
            }
            self.expect(b',')?;
            self.skip_whitespace();
        }

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

    fn expect_end(&mut self) -> Result<(), ()> {
        self.skip_whitespace();
        if self.position == self.input.len() {
            Ok(())
        } else {
            Err(())
        }
    }
}

fn parse_unit_interval(value: &str) -> Result<f64, ()> {
    let value = value.parse::<f64>().map_err(|_| ())?;
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(value)
    } else {
        Err(())
    }
}

fn default_scope(room_id: &Option<String>) -> MemoryScope {
    if room_id.as_deref().is_some_and(|value| !value.is_empty()) {
        MemoryScope::Room
    } else {
        MemoryScope::Private
    }
}
