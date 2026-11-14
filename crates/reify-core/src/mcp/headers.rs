use super::{CacheScope, McpError, McpRequest, PROTOCOL_VERSION};
use base64::{Engine, engine::general_purpose::STANDARD};
use http::{HeaderMap, HeaderValue};
use std::borrow::Cow;

/// Protocol version header (HTTP names are case-insensitive).
pub const MCP_PROTOCOL_VERSION: &str = "mcp-protocol-version";
/// Mirrored RPC method.
pub const MCP_METHOD: &str = "mcp-method";
/// Mirrored tool/prompt name or resource URI.
pub const MCP_NAME: &str = "mcp-name";
/// Reify streaming preference.
pub const MCP_STREAM: &str = "mcp-stream";
/// Reify cache visibility scope.
pub const MCP_CACHE_SCOPE: &str = "mcp-cache-scope";
/// Reify TTL hint in milliseconds.
pub const MCP_TTL_MS: &str = "mcp-ttl-ms";
/// Reify companion session routing hint, not an MCP protocol session.
pub const MCP_SESSION_BRIDGE: &str = "mcp-session-bridge";

/// Parsed headers borrowing the original `HeaderMap`, with no string allocations.
/// Encoded names are decoded lazily by `decoded_name`, allocating only for Base64.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpHeaders<'a> {
    /// Optional protocol revision; required by `validate_request`.
    pub protocol_version: Option<&'a str>,
    /// Case-sensitive RPC method.
    pub method: Option<&'a str>,
    /// Raw, possibly sentinel-encoded routing name.
    pub name: Option<&'a str>,
    /// Optional streaming preference.
    pub stream: Option<bool>,
    /// Optional Reify cache scope.
    pub cache_scope: Option<CacheScope>,
    /// Optional millisecond TTL, including zero.
    pub ttl_ms: Option<u64>,
    /// Optional opaque bridge identifier.
    pub session_bridge: Option<&'a str>,
}
fn field<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, McpError> {
    let mut values = headers.get_all(name).iter();
    let first = values.next();
    if values.next().is_some() {
        return Err(McpError::HeaderParseError(format!("duplicate {name}")));
    }
    first
        .map(|v| {
            let value = v
                .to_str()
                .map_err(|_| McpError::HeaderParseError(format!("non-ASCII {name}")))?;
            if value.is_empty()
                || value.trim_matches([' ', '\t']) != value
                || !value
                    .bytes()
                    .all(|b| b == b'\t' || (0x20..=0x7e).contains(&b))
            {
                return Err(McpError::HeaderParseError(format!("invalid {name}")));
            }
            Ok(value)
        })
        .transpose()
}
impl<'a> McpHeaders<'a> {
    /// Parse all recognized headers, rejecting duplicate and malformed values.
    /// Missing headers are allowed here for intermediary and response use.
    pub fn parse(headers: &'a HeaderMap) -> Result<Self, McpError> {
        let stream = field(headers, MCP_STREAM)?
            .map(|v| match v {
                "true" => Ok(true),
                "false" => Ok(false),
                _ => Err(McpError::HeaderParseError(
                    "Mcp-Stream must be true or false".into(),
                )),
            })
            .transpose()?;
        let cache_scope = field(headers, MCP_CACHE_SCOPE)?
            .map(|v| match v {
                "task" => Ok(CacheScope::Task),
                "domain" => Ok(CacheScope::Domain),
                "global" => Ok(CacheScope::Global),
                _ => Err(McpError::HeaderParseError("invalid Mcp-Cache-Scope".into())),
            })
            .transpose()?;
        let ttl_ms = field(headers, MCP_TTL_MS)?
            .map(|v| {
                if !v.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(McpError::HeaderParseError(
                        "Mcp-Ttl-Ms requires decimal digits".into(),
                    ));
                }
                v.parse()
                    .map_err(|_| McpError::HeaderParseError("Mcp-Ttl-Ms exceeds u64".into()))
            })
            .transpose()?;
        Ok(Self {
            protocol_version: field(headers, MCP_PROTOCOL_VERSION)?,
            method: field(headers, MCP_METHOD)?,
            name: field(headers, MCP_NAME)?,
            stream,
            cache_scope,
            ttl_ms,
            session_bridge: field(headers, MCP_SESSION_BRIDGE)?,
        })
    }
    /// Decode a sentinel-encoded name; ordinary ASCII names remain borrowed.
    pub fn decoded_name(&self) -> Result<Option<Cow<'a, str>>, McpError> {
        self.name.map(decode_header_value).transpose()
    }
    /// Validate required routing mirrors and per-request protocol metadata.
    /// HTTP adapters map routing failures to HTTP 400 / JSON-RPC -32020.
    /// This checks standard mirrors only, not schema-specific Mcp-Param headers.
    pub fn validate_request(&self, request: &McpRequest) -> Result<(), McpError> {
        request.validate()?;
        let mismatch = || {
            McpError::ProtocolViolation(
                "HeaderMismatch (-32020): missing or unequal routing metadata".into(),
            )
        };
        let version = request
            .params
            .as_ref()
            .and_then(|p| p.get("_meta"))
            .and_then(|m| m.get("io.modelcontextprotocol/protocolVersion"))
            .and_then(|v| v.as_str());
        if self.method != Some(request.method.as_str())
            || self.protocol_version.is_none()
            || self.protocol_version != version
        {
            return Err(mismatch());
        }
        if self.protocol_version != Some(PROTOCOL_VERSION) {
            return Err(McpError::ProtocolViolation(format!(
                "unsupported protocol version; supported: {PROTOCOL_VERSION}"
            )));
        }
        let key = match request.method.as_str() {
            "tools/call" | "prompts/get" => Some("name"),
            "resources/read" => Some("uri"),
            _ => None,
        };
        if let Some(key) = key {
            let body = request
                .params
                .as_ref()
                .and_then(|p| p.get(key))
                .and_then(|v| v.as_str())
                .ok_or_else(mismatch)?;
            if self.decoded_name()?.as_deref() != Some(body) {
                return Err(mismatch());
            }
        }
        Ok(())
    }
}
/// Decode MCP's case-sensitive Base64 sentinel, preserving plain values by reference.
pub fn decode_header_value(value: &str) -> Result<Cow<'_, str>, McpError> {
    if let Some(encoded) = value
        .strip_prefix("=?base64?")
        .and_then(|v| v.strip_suffix("?="))
    {
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|e| McpError::HeaderParseError(format!("invalid Base64 header: {e}")))?;
        return String::from_utf8(bytes)
            .map(Cow::Owned)
            .map_err(|e| McpError::HeaderParseError(format!("header is not UTF-8: {e}")));
    }
    if value.is_empty()
        || value.trim_matches([' ', '\t']) != value
        || !value
            .bytes()
            .all(|b| b == b'\t' || (0x20..=0x7e).contains(&b))
    {
        return Err(McpError::HeaderParseError(
            "unsafe plain header value".into(),
        ));
    }
    Ok(Cow::Borrowed(value))
}
/// Encode unsafe UTF-8 or sentinel-shaped names into an injection-safe HTTP value.
pub fn encode_header_value(value: &str) -> HeaderValue {
    let safe = !value.is_empty()
        && value.trim_matches([' ', '\t']) == value
        && value
            .bytes()
            .all(|b| b == b'\t' || (0x20..=0x7e).contains(&b))
        && !(value.starts_with("=?base64?") && value.ends_with("?="));
    if safe {
        HeaderValue::from_str(value).expect("validated ASCII header")
    } else {
        HeaderValue::from_str(&format!("=?base64?{}?=", STANDARD.encode(value)))
            .expect("Base64 is header safe")
    }
}
