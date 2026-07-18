use serde_json::Value;

use super::{
    LogicalInvocationError, MAX_CAPABILITY_ARGUMENT_BYTES, MAX_CAPABILITY_ARGUMENT_DEPTH,
    MAX_CAPABILITY_ARGUMENT_NODES,
};

pub(super) fn validate_argument_bounds(arguments: &Value) -> Result<(), LogicalInvocationError> {
    let mut nodes = 0usize;
    let mut encoded_bytes = 0usize;
    let mut pending = vec![(arguments, 1usize)];
    while let Some((value, depth)) = pending.pop() {
        nodes += 1;
        if nodes > MAX_CAPABILITY_ARGUMENT_NODES {
            return Err(LogicalInvocationError::ArgumentsTooManyNodes);
        }
        if depth > MAX_CAPABILITY_ARGUMENT_DEPTH {
            return Err(LogicalInvocationError::ArgumentsTooDeep);
        }
        encoded_bytes = encoded_bytes.saturating_add(match value {
            Value::Null => 4,
            Value::Bool(_) => 5,
            Value::Number(_) => 32,
            Value::String(value) => json_string_encoded_len(value),
            Value::Array(values) => values.len().saturating_add(2),
            Value::Object(values) => values
                .keys()
                .map(|key| json_string_encoded_len(key).saturating_add(2))
                .sum::<usize>()
                .saturating_add(2),
        });
        if encoded_bytes > MAX_CAPABILITY_ARGUMENT_BYTES {
            return Err(LogicalInvocationError::ArgumentsTooLarge);
        }
        match value {
            Value::Array(values) => pending.extend(values.iter().map(|value| (value, depth + 1))),
            Value::Object(values) => {
                pending.extend(values.values().map(|value| (value, depth + 1)))
            }
            _ => {}
        }
    }
    Ok(())
}

fn json_string_encoded_len(value: &str) -> usize {
    value.chars().fold(2usize, |length, character| {
        length.saturating_add(match character {
            '"' | '\\' | '\u{0008}' | '\u{000c}' | '\n' | '\r' | '\t' => 2,
            '\u{0000}'..='\u{001f}' => 6,
            _ => character.len_utf8(),
        })
    })
}

pub(super) fn canonicalize_arguments(arguments: Value) -> Result<Value, LogicalInvocationError> {
    let bytes = serde_jcs::to_vec(&arguments)
        .map_err(|_| LogicalInvocationError::CanonicalizationFailed)?;
    serde_json::from_slice(&bytes).map_err(|_| LogicalInvocationError::CanonicalizationFailed)
}
