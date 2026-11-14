use super::McpError;
use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeStruct};
use serde_json::{Map, Value};

/// Reify cache visibility, distinct from standard MCP list-result cache scopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheScope {
    /// Current compilation task only.
    Task,
    /// Requests for one canonical domain.
    Domain,
    /// Publicly reusable across domains.
    Global,
}
/// Reify cache freshness policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheHint {
    /// Freshness duration in milliseconds; zero means immediately stale.
    pub ttl_ms: u64,
    /// Visibility boundary.
    pub cache_scope: CacheScope,
    /// Whether stale data may trigger asynchronous revalidation.
    pub revalidate_in_background: bool,
}
/// Provenance attached to a compiled tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolMetadata {
    /// Searchable categorization tags.
    pub tags: Vec<String>,
    /// Original upstream endpoint, if known.
    pub source_endpoint: Option<String>,
}
/// Discoverable tool definition. Schemas are meta-validated on deserialization.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    /// Unique, case-sensitive tool name.
    pub name: String,
    /// Human-readable operation description.
    pub description: String,
    /// JSON Schema object (defaults to Draft 2020-12).
    #[serde(deserialize_with = "deserialize_schema")]
    pub input_schema: Value,
    /// Reify provenance, optional.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ToolMetadata>,
}
impl Serialize for ToolDefinition {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.validate().map_err(serde::ser::Error::custom)?;
        let mut state = serializer
            .serialize_struct("ToolDefinition", 3 + usize::from(self.metadata.is_some()))?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("description", &self.description)?;
        state.serialize_field("inputSchema", &self.input_schema)?;
        if let Some(metadata) = &self.metadata {
            state.serialize_field("metadata", metadata)?;
        }
        state.end()
    }
}
fn deserialize_schema<'de, D: Deserializer<'de>>(d: D) -> Result<Value, D::Error> {
    let value = Value::deserialize(d)?;
    validate_schema(&value).map_err(serde::de::Error::custom)?;
    Ok(value)
}
fn validate_schema(schema: &Value) -> Result<(), McpError> {
    if !schema.is_object() {
        return Err(McpError::SchemaValidationError(
            "inputSchema must be an object".into(),
        ));
    }
    // Reject unsupported dialects before using the library's infallible draft detection.
    if let Some(dialect) = schema.get("$schema") {
        let supported = [
            "https://json-schema.org/draft/2020-12/schema",
            "https://json-schema.org/draft/2019-09/schema",
            "http://json-schema.org/draft-07/schema#",
            "http://json-schema.org/draft-06/schema#",
            "http://json-schema.org/draft-04/schema#",
        ];
        if !dialect.as_str().is_some_and(|d| supported.contains(&d)) {
            return Err(McpError::SchemaValidationError(
                "unsupported JSON Schema dialect".into(),
            ));
        }
    }
    jsonschema::meta::validate(schema).map_err(|e| McpError::SchemaValidationError(e.to_string()))
}
impl ToolDefinition {
    /// Serialize validated wire JSON directly from borrowed fields. Caches can
    /// retain this compact immutable representation without copying a Value tree.
    pub fn to_mcp_json(&self) -> Result<Box<serde_json::value::RawValue>, McpError> {
        self.validate()?;
        #[derive(Serialize)]
        struct Meta<'a> {
            #[serde(rename = "reify/provenance")]
            provenance: &'a ToolMetadata,
        }
        #[derive(Serialize)]
        struct Wire<'a> {
            name: &'a str,
            description: &'a str,
            #[serde(rename = "inputSchema")]
            schema: &'a Value,
            #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
            meta: Option<Meta<'a>>,
        }
        Ok(serde_json::value::to_raw_value(&Wire {
            name: &self.name,
            description: &self.description,
            schema: &self.input_schema,
            meta: self.metadata.as_ref().map(|provenance| Meta { provenance }),
        })?)
    }
    /// Publish a tool definition with Reify provenance in namespaced MCP metadata.
    pub fn to_mcp_value(&self) -> Result<Value, McpError> {
        self.validate()?;
        let mut value = serde_json::json!({"name":self.name, "description":self.description, "inputSchema":self.input_schema});
        if let Some(metadata) = &self.metadata {
            value["_meta"] = serde_json::json!({"reify/provenance":metadata});
        }
        Ok(value)
    }
    /// Validate a schema after programmatic construction or mutation.
    pub fn validate(&self) -> Result<(), McpError> {
        validate_schema(&self.input_schema)
    }
}
fn empty_arguments() -> Map<String, Value> {
    Map::new()
}
/// Typed tool call parameters; unknown metadata stays in the original request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallParams {
    /// Tool to invoke.
    pub name: String,
    /// Named arguments. Null, arrays, and scalar arguments are rejected.
    #[serde(default = "empty_arguments")]
    pub arguments: Map<String, Value>,
}
/// Embedded textual resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceContent {
    /// Resource URI, including non-HTTP schemes.
    pub uri: String,
    /// Optional MIME type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Resource contents.
    pub text: String,
}
/// Reify content representation. `Json` is an internal extension; use
/// `ToolCallResult::to_mcp_value` when publishing content to standard MCP clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ContentBlock {
    /// Plain text.
    Text {
        /// Text payload.
        text: String,
    },
    /// Structured JSON retained internally without stringifying.
    Json {
        /// Arbitrary JSON payload.
        json: Value,
    },
    /// Embedded resource.
    Resource {
        /// Resource description and payload.
        resource: ResourceContent,
    },
}
/// Owned execution result with a Reify cache policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallResult {
    /// Ordered content blocks.
    pub content: Vec<ContentBlock>,
    /// Tool execution failed (separate from a JSON-RPC failure).
    pub is_error: bool,
    /// Reify cache policy, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_hint: Option<CacheHint>,
}
impl ToolCallResult {
    /// Publish owned results without copying text/resource payloads or rebuilding
    /// an intermediate ContentBlock tree. JSON blocks still stringify once.
    pub fn into_mcp_value(self) -> Result<Value, McpError> {
        let mut content = Vec::with_capacity(self.content.len());
        for block in self.content {
            let mut item = Map::new();
            match block {
                ContentBlock::Text { text } => {
                    item.insert("type".into(), Value::String("text".into()));
                    item.insert("text".into(), Value::String(text));
                }
                ContentBlock::Json { json } => {
                    item.insert("type".into(), Value::String("text".into()));
                    item.insert("text".into(), Value::String(serde_json::to_string(&json)?));
                }
                ContentBlock::Resource { resource } => {
                    item.insert("type".into(), Value::String("resource".into()));
                    let mut value = Map::new();
                    value.insert("uri".into(), Value::String(resource.uri));
                    value.insert("text".into(), Value::String(resource.text));
                    if let Some(mime) = resource.mime_type {
                        value.insert("mimeType".into(), Value::String(mime));
                    }
                    item.insert("resource".into(), Value::Object(value));
                }
            }
            content.push(Value::Object(item));
        }
        let mut result = serde_json::json!({"resultType":"complete","isError":self.is_error});
        result["content"] = Value::Array(content);
        if let Some(hint) = self.cache_hint {
            result["_meta"] = serde_json::json!({"reify/cacheHint":hint});
        }
        Ok(result)
    }
    /// Publish a completed MCP result: JSON blocks become portable text blocks,
    /// and Reify cache hints use namespaced metadata rather than standard cacheScope.
    pub fn to_mcp_value(&self) -> Result<Value, McpError> {
        let content: Vec<Value> = self
            .content
            .iter()
            .map(|block| match block {
                ContentBlock::Json { json } => {
                    Ok(serde_json::json!({"type":"text", "text":serde_json::to_string(json)?}))
                }
                other => serde_json::to_value(other),
            })
            .collect::<Result<_, serde_json::Error>>()?;
        let mut result = serde_json::json!({"resultType":"complete", "isError": self.is_error});
        result["content"] = Value::Array(content);
        if let Some(hint) = &self.cache_hint {
            result["_meta"] = serde_json::json!({"reify/cacheHint": hint});
        }
        Ok(result)
    }
}
