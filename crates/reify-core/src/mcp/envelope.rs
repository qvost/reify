use super::{McpError, ToolCallParams};
use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeStruct};
use serde_json::Value;

/// JSON-RPC correlation ID. Null and boolean IDs are deliberately unsupported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    /// Opaque textual identifier.
    String(String),
    /// JSON numeric identifier, preserving integers without conversion to f64.
    Number(serde_json::Number),
}
impl From<&str> for RequestId {
    fn from(id: &str) -> Self {
        Self::String(id.into())
    }
}
impl From<i64> for RequestId {
    fn from(id: i64) -> Self {
        Self::Number(id.into())
    }
}

/// One JSON-RPC request (notifications and batches are separate transport concerns).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(try_from = "RequestWire")]
pub struct McpRequest {
    /// Must be exactly `2.0`.
    pub jsonrpc: String,
    /// Correlation identifier.
    pub id: RequestId,
    /// Case-sensitive RPC method.
    pub method: String,
    /// Object or array parameters; omitted parameters are supported.
    pub params: Option<Value>,
}
#[derive(Deserialize)]
struct RequestWire {
    jsonrpc: String,
    id: RequestId,
    method: String,
    #[serde(default, deserialize_with = "present_value")]
    params: Option<Value>,
}
fn present_value<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}
impl TryFrom<RequestWire> for McpRequest {
    type Error = McpError;
    fn try_from(w: RequestWire) -> Result<Self, Self::Error> {
        let request = Self {
            jsonrpc: w.jsonrpc,
            id: w.id,
            method: w.method,
            params: w.params,
        };
        request.validate()?;
        Ok(request)
    }
}
impl McpRequest {
    /// Create a valid request without manually spelling the JSON-RPC version.
    pub fn new(
        id: RequestId,
        method: impl Into<String>,
        params: Option<Value>,
    ) -> Result<Self, McpError> {
        Self::try_from(RequestWire {
            jsonrpc: "2.0".into(),
            id,
            method: method.into(),
            params,
        })
    }
    /// Check JSON-RPC invariants after public fields have been edited.
    pub fn validate(&self) -> Result<(), McpError> {
        if self.jsonrpc != "2.0"
            || self
                .params
                .as_ref()
                .is_some_and(|v| !v.is_object() && !v.is_array())
        {
            return Err(McpError::ProtocolViolation(
                "expected jsonrpc 2.0 and structured params".into(),
            ));
        }
        Ok(())
    }
    /// Detect the legacy initialization method.
    pub fn is_initialize(&self) -> bool {
        self.method == "initialize"
    }
    /// Detect tool discovery.
    pub fn is_tools_list(&self) -> bool {
        self.method == "tools/list"
    }
    /// Detect tool invocation.
    pub fn is_tools_call(&self) -> bool {
        self.method == "tools/call"
    }
    /// Move tool-call fields out of the request while retaining per-request metadata.
    /// This consumes name/arguments even on a decoding failure; use the borrowed
    /// `extract_tool_call` when the original fields must remain available.
    pub fn take_tool_call(&mut self) -> Result<ToolCallParams, McpError> {
        self.validate()?;
        if !self.is_tools_call() {
            return Err(McpError::ProtocolViolation("expected tools/call".into()));
        }
        let params = self
            .params
            .as_mut()
            .and_then(Value::as_object_mut)
            .ok_or_else(|| McpError::ProtocolViolation("missing tool call params".into()))?;
        let name = match params.remove("name") {
            Some(Value::String(name)) => name,
            _ => {
                return Err(McpError::ProtocolViolation(
                    "tool name must be a string".into(),
                ));
            }
        };
        let arguments = match params.remove("arguments") {
            None => serde_json::Map::new(),
            Some(Value::Object(arguments)) => arguments,
            _ => {
                return Err(McpError::ProtocolViolation(
                    "tool arguments must be an object".into(),
                ));
            }
        };
        Ok(ToolCallParams { name, arguments })
    }
    /// Extract a typed call; omitted arguments become an empty object.
    pub fn extract_tool_call(&self) -> Result<ToolCallParams, McpError> {
        self.validate()?;
        if !self.is_tools_call() {
            return Err(McpError::ProtocolViolation("expected tools/call".into()));
        }
        ToolCallParams::deserialize(
            self.params
                .as_ref()
                .ok_or_else(|| McpError::ProtocolViolation("missing tool call params".into()))?,
        )
        .map_err(|e| McpError::ProtocolViolation(format!("invalid tool call params: {e}")))
    }
}
impl Serialize for McpRequest {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.validate().map_err(serde::ser::Error::custom)?;
        let mut s = s.serialize_struct("McpRequest", 3 + usize::from(self.params.is_some()))?;
        s.serialize_field("jsonrpc", &self.jsonrpc)?;
        s.serialize_field("id", &self.id)?;
        s.serialize_field("method", &self.method)?;
        if let Some(params) = &self.params {
            s.serialize_field("params", params)?;
        }
        s.end()
    }
}

/// Structured JSON-RPC error; error data is application-defined.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpRpcError {
    /// JSON-RPC or implementation-defined error code.
    pub code: i64,
    /// Human-readable failure description.
    pub message: String,
    /// Optional details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}
/// One response containing exactly one of result or error.
/// A successful JSON null is represented by `Some(Value::Null)`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(try_from = "ResponseWire")]
pub struct McpResponse {
    /// Must be `2.0`.
    pub jsonrpc: String,
    /// Identifier copied from the request.
    pub id: RequestId,
    /// Successful result, including JSON null.
    pub result: Option<Value>,
    /// RPC failure.
    pub error: Option<McpRpcError>,
}
#[derive(Deserialize)]
struct ResponseWire {
    jsonrpc: String,
    id: RequestId,
    #[serde(default, deserialize_with = "present_value")]
    result: Option<Value>,
    #[serde(default, deserialize_with = "present_value")]
    error: Option<Value>,
}
impl TryFrom<ResponseWire> for McpResponse {
    type Error = McpError;
    fn try_from(w: ResponseWire) -> Result<Self, Self::Error> {
        let response = Self {
            jsonrpc: w.jsonrpc,
            id: w.id,
            result: w.result,
            error: w.error.map(serde_json::from_value).transpose()?,
        };
        response.validate()?;
        Ok(response)
    }
}
impl McpResponse {
    /// Construct a successful response.
    pub fn success(id: RequestId, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }
    /// Construct a failed response.
    pub fn error(
        id: RequestId,
        code: i64,
        message: impl Into<String>,
        data: Option<Value>,
    ) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(McpRpcError {
                code,
                message: message.into(),
                data,
            }),
        }
    }
    /// Enforce mutually exclusive response outcomes and the version marker.
    pub fn validate(&self) -> Result<(), McpError> {
        if self.jsonrpc != "2.0" || self.result.is_some() == self.error.is_some() {
            return Err(McpError::ProtocolViolation(
                "response requires jsonrpc 2.0 and exactly one of result/error".into(),
            ));
        }
        Ok(())
    }
}
impl Serialize for McpResponse {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.validate().map_err(serde::ser::Error::custom)?;
        let mut s = s.serialize_struct("McpResponse", 3)?;
        s.serialize_field("jsonrpc", &self.jsonrpc)?;
        s.serialize_field("id", &self.id)?;
        if let Some(result) = &self.result {
            s.serialize_field("result", result)?;
        }
        if let Some(error) = &self.error {
            s.serialize_field("error", error)?;
        }
        s.end()
    }
}
