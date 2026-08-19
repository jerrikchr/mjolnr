use crate::core::error::ToolError;

pub(super) fn required_string(
    arguments: &serde_json::Value,
    key: &str,
) -> Result<String, ToolError> {
    arguments
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ToolError::SchemaInvalid {
            detail: format!("missing string argument `{key}`"),
        })
}

pub(super) fn optional_u64(arguments: &serde_json::Value, key: &str, default: u64) -> u64 {
    arguments
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(default)
}

pub(super) fn optional_bool(arguments: &serde_json::Value, key: &str, default: bool) -> bool {
    arguments
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(default)
}

pub(super) fn string_array(
    arguments: &serde_json::Value,
    key: &str,
) -> Result<Vec<String>, ToolError> {
    arguments
        .get(key)
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ToolError::SchemaInvalid {
            detail: format!("missing array argument `{key}`"),
        })?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| ToolError::SchemaInvalid {
                    detail: format!("`{key}` must contain only strings"),
                })
        })
        .collect()
}
