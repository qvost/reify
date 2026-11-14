//! Order-independent structural joins. Every observed instance satisfies its schema.
use chrono::{DateTime, NaiveDate};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::OnceLock};
use url::Url;
use uuid::Uuid;

/// Infer a Draft 2020-12-compatible schema. No observations yield an unconstrained
/// object schema `{}`. Recursion beyond 64 levels also widens to `{}`.
pub fn induce(samples: &[Value]) -> Value {
    induce_refs(&samples.iter().collect::<Vec<_>>())
}
/// Infer without cloning captured payloads. Format annotations require unanimity.
pub fn induce_refs(samples: &[&Value]) -> Value {
    infer(samples, 0)
}
fn infer(samples: &[&Value], depth: usize) -> Value {
    if samples.is_empty() || depth >= 64 {
        return json!({});
    }
    let mut groups: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    for &value in samples {
        let kind = match value {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        groups.entry(kind).or_default().push(value);
    }
    let mut alternatives = Vec::new();
    for (kind, values) in groups {
        let schema = match kind {
            "object" => {
                let mut fields: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
                for value in &values {
                    for (key, value) in value.as_object().expect("object partition") {
                        fields.entry(key).or_default().push(value);
                    }
                }
                let mut properties = serde_json::Map::new();
                let mut required = Vec::new();
                for (key, present) in fields {
                    if present.len() == values.len() {
                        required.push(Value::String(key.to_owned()));
                    }
                    properties.insert(key.to_owned(), infer(&present, depth + 1));
                }
                let mut result = json!({"type":"object","additionalProperties":false});
                result["properties"] = Value::Object(properties);
                result["required"] = Value::Array(required);
                result
            }
            "array" => {
                let items: Vec<_> = values
                    .iter()
                    .flat_map(|v| v.as_array().into_iter().flatten())
                    .collect();
                let mut result = json!({"type":"array"});
                result["items"] = infer(&items, depth + 1);
                result
            }
            "number" => {
                let integral = values.iter().all(|v| {
                    v.is_i64() || v.is_u64() || v.as_f64().is_some_and(|n| n.fract() == 0.0)
                });
                json!({"type":if integral { "integer" } else { "number" }})
            }
            "string" => {
                let first = format(values[0].as_str().expect("string partition"));
                if let Some(format) = first.filter(|f| {
                    values
                        .iter()
                        .all(|v| format(v.as_str().expect("string partition")) == Some(*f))
                }) {
                    json!({"type":"string","format":format})
                } else {
                    json!({"type":"string"})
                }
            }
            other => json!({"type":other}),
        };
        alternatives.push(schema);
    }
    if alternatives.len() == 1 {
        alternatives.pop().expect("one alternative")
    } else {
        let mut result = serde_json::Map::new();
        result.insert("anyOf".into(), Value::Array(alternatives));
        Value::Object(result)
    }
}
fn format(value: &str) -> Option<&'static str> {
    let candidate = if value.len() == 36 && Uuid::parse_str(value).is_ok() {
        Some("uuid")
    } else if value.len() == 10 && NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok() {
        Some("date")
    } else if value.contains('T') && DateTime::parse_from_rfc3339(value).is_ok() {
        Some("date-time")
    } else if email(value) {
        Some("email")
    } else if Url::parse(value).is_ok() && !value.chars().any(char::is_whitespace) {
        Some("uri")
    } else {
        None
    }?;
    // URL/chrono parsers accept some inputs outside JSON Schema format syntax.
    // Check the proposed annotation with the same standard's format validator.
    static FORMATS: OnceLock<Vec<(&str, jsonschema::Validator)>> = OnceLock::new();
    let validators = FORMATS.get_or_init(|| {
        ["uuid", "date", "date-time", "uri", "email"]
            .into_iter()
            .map(|name| {
                let validator = jsonschema::options()
                    .should_validate_formats(true)
                    .build(&json!({"type":"string","format":name}))
                    .expect("built-in string format schema");
                (name, validator)
            })
            .collect()
    });
    validators
        .iter()
        .find(|(name, _)| *name == candidate)
        .filter(|(_, validator)| validator.is_valid(&Value::String(value.to_owned())))
        .map(|(name, _)| *name)
}
fn email(s: &str) -> bool {
    let Some((local, host)) = s.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && local
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b".!#$%&'*+-/=?^_`{|}~".contains(&c))
        && host.contains('.')
        && host.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
}
