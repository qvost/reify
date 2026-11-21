//! Schema-specific MCP header mirrors, independent of transport dispatch.
use http::{HeaderMap, HeaderName};
use reify_core::mcp::decode_header_value;
use serde_json::Value;
use std::collections::HashSet;
#[derive(Clone)]
struct Annotation {
    name: HeaderName,
    path: Vec<String>,
    kind: String,
}
fn annotations(
    schema: &Value,
    path: &mut Vec<String>,
    reachable: bool,
    output: &mut Vec<Annotation>,
    names: &mut HashSet<String>,
    depth: usize,
) -> Result<(), ()> {
    if depth > 64 {
        return Err(());
    }
    if let Some(header) = schema.get("x-mcp-header") {
        if !reachable || path.is_empty() {
            return Err(());
        }
        let header = header.as_str().ok_or(())?;
        if header.is_empty() || HeaderName::from_bytes(header.as_bytes()).is_err() {
            return Err(());
        }
        let name =
            HeaderName::from_bytes(format!("Mcp-Param-{header}").as_bytes()).map_err(|_| ())?;
        if !names.insert(name.to_string()) {
            return Err(());
        }
        let kind = schema
            .get("type")
            .and_then(Value::as_str)
            .filter(|s| matches!(*s, "string" | "integer" | "boolean"))
            .ok_or(())?;
        output.push(Annotation {
            name,
            path: path.clone(),
            kind: kind.into(),
        });
    }
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for (key, child) in properties {
            path.push(key.clone());
            annotations(child, path, reachable, output, names, depth + 1)?;
            path.pop();
        }
    }
    for key in [
        "$defs",
        "definitions",
        "patternProperties",
        "dependentSchemas",
    ] {
        if let Some(map) = schema.get(key).and_then(Value::as_object) {
            for child in map.values() {
                annotations(child, path, false, output, names, depth + 1)?;
            }
        }
    }
    for key in [
        "items",
        "additionalItems",
        "additionalProperties",
        "contains",
        "propertyNames",
        "unevaluatedItems",
        "unevaluatedProperties",
        "not",
        "if",
        "then",
        "else",
    ] {
        if let Some(child) = schema.get(key) {
            annotations(child, path, false, output, names, depth + 1)?;
        }
    }
    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(children) = schema.get(key).and_then(Value::as_array) {
            for child in children {
                annotations(child, path, false, output, names, depth + 1)?;
            }
        }
    }
    Ok(())
}
#[derive(Clone)]
pub(crate) struct ParameterMirrors {
    mirrors: Vec<Annotation>,
}
impl ParameterMirrors {
    pub(crate) fn new(schema: &Value) -> Result<Self, ()> {
        let mut mirrors = Vec::new();
        annotations(
            schema,
            &mut Vec::new(),
            true,
            &mut mirrors,
            &mut HashSet::new(),
            0,
        )?;
        Ok(Self { mirrors })
    }
    pub(crate) fn validate(&self, arguments: &Value, headers: &HeaderMap) -> Result<(), ()> {
        for mirror in &self.mirrors {
            let mut value = Some(arguments);
            for component in &mirror.path {
                value = value
                    .and_then(|v| v.as_object())
                    .and_then(|o| o.get(component));
            }
            let value = value.filter(|v| !v.is_null());
            let mut fields = headers.get_all(&mirror.name).iter();
            let header = fields.next();
            if fields.next().is_some() {
                return Err(());
            }
            let Some(value) = value else {
                if header.is_some() {
                    return Err(());
                }
                continue;
            };
            let header = header.and_then(|v| v.to_str().ok()).ok_or(())?;
            let header = decode_header_value(header).map_err(|_| ())?;
            let matches = match mirror.kind.as_str() {
                "string" => value.as_str() == Some(header.as_ref()),
                "boolean" => match value.as_bool() {
                    Some(true) => header == "true",
                    Some(false) => header == "false",
                    None => false,
                },
                "integer" => {
                    value
                        .as_f64()
                        .zip(header.parse::<f64>().ok())
                        .is_some_and(|(a, b)| {
                            a.is_finite()
                                && b.is_finite()
                                && a.fract() == 0.0
                                && a.abs() <= 9_007_199_254_740_991.0
                                && a == b
                        })
                }
                _ => false,
            };
            if !matches {
                return Err(());
            }
        }
        Ok(())
    }
}
pub(crate) fn validate(schema: &Value, arguments: &Value, headers: &HeaderMap) -> Result<(), ()> {
    ParameterMirrors::new(schema)?.validate(arguments, headers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn validates_nested_encoded_and_numeric_mirrors() {
        let schema = json!({"properties":{"region":{"type":"string","x-mcp-header":"Region"},"nested":{"properties":{"count":{"type":"integer","x-mcp-header":"Count"}}}}});
        let arguments = json!({"region":"世界","nested":{"count":42}});
        let mut headers = HeaderMap::new();
        headers.insert(
            "mcp-param-region",
            reify_core::mcp::encode_header_value("世界"),
        );
        headers.insert("mcp-param-count", http::HeaderValue::from_static("42.0"));
        assert!(validate(&schema, &arguments, &headers).is_ok());
        headers.remove("mcp-param-count");
        assert!(validate(&schema, &arguments, &headers).is_err());
    }
    #[test]
    fn rejects_unreachable_duplicate_and_unsafe_annotations() {
        for schema in [
            json!({"items":{"type":"string","x-mcp-header":"X"}}),
            json!({"properties":{"a":{"type":"string","x-mcp-header":"same"},"b":{"type":"string","x-mcp-header":"Same"}}}),
            json!({"properties":{"a":{"type":"number","x-mcp-header":"A"}}}),
        ] {
            assert!(validate(&schema, &json!({}), &HeaderMap::new()).is_err());
        }
    }
}
