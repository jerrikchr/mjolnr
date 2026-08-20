//! OpenAI-specific lowering for provider-neutral function schemas.

/// Collapse a JSON Schema union `type` into the single scalar OpenAI and
/// Jinja-based tool calling templates (LM Studio, Ollama, `LocalAI`) accept.
/// `["integer", "null"]` becomes `"integer"`, `["string", "null"]` becomes `"string"`.
fn collapse_type(value: &serde_json::Value) -> serde_json::Value {
    if let Some(variants) = value.as_array() {
        if let Some(concrete) = variants.iter().find(|variant| *variant != "null") {
            return concrete.clone();
        }
        return serde_json::Value::String("string".to_owned());
    }
    value.clone()
}

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
                .map(|(key, value)| {
                    let lowered = if key == "type" {
                        collapse_type(value)
                    } else {
                        strict_parameters(value)
                    };
                    (key.clone(), lowered)
                })
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
/// cannot use strict mode for mjolnr's `spawn_subagent.result_schema`: that
/// field intentionally accepts a user-declared schema whose properties are
/// not known when mjolnr publishes the tool definition.
pub(crate) fn compatible_parameters(schema: &serde_json::Value) -> serde_json::Value {
    match schema {
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(compatible_parameters).collect())
        }
        serde_json::Value::Object(object) => serde_json::Value::Object(
            object
                .iter()
                .filter(|(key, _)| key.as_str() != "$schema")
                .map(|(key, value)| {
                    let lowered = if key == "type" {
                        collapse_type(value)
                    } else {
                        compatible_parameters(value)
                    };
                    (key.clone(), lowered)
                })
                .collect(),
        ),
        scalar => scalar.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatible_parameters_collapses_union_types() {
        let schema = serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "line_count": { "type": ["integer", "null"], "maximum": 1000 },
                "path": { "type": "string" }
            },
            "required": ["path"]
        });

        let lowered = compatible_parameters(&schema);
        assert!(lowered.get("$schema").is_none());
        let properties = lowered.get("properties").expect("properties");
        assert_eq!(
            properties.get("line_count").unwrap().get("type"),
            Some(&serde_json::json!("integer"))
        );
        assert_eq!(
            properties.get("path").unwrap().get("type"),
            Some(&serde_json::json!("string"))
        );
    }
}
