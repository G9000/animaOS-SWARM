use jsonschema::{Draft, JSONSchema};
use serde_json::Value;

use super::CapabilityError;

pub(super) fn compile_schema(schema: &Value) -> Result<JSONSchema, ()> {
    let draft = match schema.get("$schema") {
        None => Draft::Draft7,
        Some(Value::String(uri))
            if matches!(
                uri.trim_end_matches('#'),
                "http://json-schema.org/draft-07/schema"
                    | "https://json-schema.org/draft-07/schema"
            ) =>
        {
            Draft::Draft7
        }
        Some(Value::String(uri))
            if uri.trim_end_matches('#') == "https://json-schema.org/draft/2020-12/schema" =>
        {
            Draft::Draft202012
        }
        _ => return Err(()),
    };
    JSONSchema::options()
        .with_draft(draft)
        .compile(schema)
        .map_err(|_| ())
}

pub(super) fn validate_instance(
    validator: &JSONSchema,
    instance: &Value,
    output: bool,
) -> Result<(), CapabilityError> {
    if validator.is_valid(instance) {
        Ok(())
    } else if output {
        Err(CapabilityError::output_validation())
    } else {
        Err(CapabilityError::validation())
    }
}
