//! OpenAI-specific lowering for provider-neutral function schemas.

/// Convert a tool schema into OpenAI's strict function subset.
///
/// OpenAI rejects dialect markers and requires every property on every object
/// to appear in `required`. Provider-neutral schemas may omit nullable
/// properties there so non-OpenAI callers can leave them out entirely.
pub(crate) fn strict_parameters(schema: &serde_json::Value) -> serde_json::Value {
    match schema {
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(strict_parameters).collect())
        }
        serde_json::Value::Object(object) => {
            let mut lowered = object
                .iter()
                .filter(|(key, _)| key.as_str() != "$schema")
                .map(|(key, value)| (key.clone(), strict_parameters(value)))
                .collect::<serde_json::Map<_, _>>();

            if let Some(properties) = lowered
                .get("properties")
                .and_then(serde_json::Value::as_object)
            {
                lowered.insert(
                    "required".to_owned(),
                    serde_json::Value::Array(
                        properties
                            .keys()
                            .cloned()
                            .map(serde_json::Value::String)
                            .collect(),
                    ),
                );
            }

            serde_json::Value::Object(lowered)
        }
        scalar => scalar.clone(),
    }
}

/// Remove JSON Schema dialect markers without changing optionality.
///
/// The `ChatGPT` subscription endpoint accepts ordinary function schemas but
/// cannot use strict mode for smed's `spawn_subagent.result_schema`: that
/// field intentionally accepts a user-declared schema whose properties are
/// not known when smed publishes the tool definition.
pub(crate) fn compatible_parameters(schema: &serde_json::Value) -> serde_json::Value {
    match schema {
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(compatible_parameters).collect())
        }
        serde_json::Value::Object(object) => serde_json::Value::Object(
            object
                .iter()
                .filter(|(key, _)| key.as_str() != "$schema")
                .map(|(key, value)| (key.clone(), compatible_parameters(value)))
                .collect(),
        ),
        scalar => scalar.clone(),
    }
}
