use serde_json::Value;

use super::{
    LogicalInvocationError, MAX_CAPABILITY_ARGUMENT_BYTES, MAX_CAPABILITY_ARGUMENT_DEPTH,
    MAX_CAPABILITY_ARGUMENT_NODES,
};

pub(super) fn validate_argument_bounds(arguments: &Value) -> Result<(), LogicalInvocationError> {
    let mut nodes = 0usize;
    let mut pending = vec![(arguments, 1usize)];
    while let Some((value, depth)) = pending.pop() {
        nodes += 1;
        if nodes > MAX_CAPABILITY_ARGUMENT_NODES {
            return Err(LogicalInvocationError::ArgumentsTooManyNodes);
        }
        if depth > MAX_CAPABILITY_ARGUMENT_DEPTH {
            return Err(LogicalInvocationError::ArgumentsTooDeep);
        }
        match value {
            Value::Array(values) => pending.extend(values.iter().map(|value| (value, depth + 1))),
            Value::Object(values) => {
                pending.extend(values.values().map(|value| (value, depth + 1)))
            }
            _ => {}
        }
    }
    let bytes = serde_json::to_vec(arguments)
        .map_err(|_| LogicalInvocationError::CanonicalizationFailed)?;
    if bytes.len() > MAX_CAPABILITY_ARGUMENT_BYTES {
        Err(LogicalInvocationError::ArgumentsTooLarge)
    } else {
        Ok(())
    }
}

pub(super) fn canonicalize_arguments(arguments: Value) -> Result<Value, LogicalInvocationError> {
    let bytes = serde_jcs::to_vec(&arguments)
        .map_err(|_| LogicalInvocationError::CanonicalizationFailed)?;
    serde_json::from_slice(&bytes).map_err(|_| LogicalInvocationError::CanonicalizationFailed)
}
