//! Validated targets, captured traffic, authentication, and compiled execution plans.
mod auth;
use crate::{ReifyError, mcp::ToolDefinition};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use url::Url;
use uuid::Uuid;

/// A canonical HTTP(S) compilation target.
/// Deserialization also passes through the constructor's validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "TargetWire")]
pub struct Target {
    /// Canonical URL. URL serialization retains `/` for an origin's root path.
    pub url: Url,
    /// Lowercased goal with Unicode whitespace collapsed; blank becomes absent.
    pub goal: Option<String>,
}
#[derive(Deserialize)]
struct TargetWire {
    url: String,
    goal: Option<String>,
}
impl TryFrom<TargetWire> for Target {
    type Error = ReifyError;
    fn try_from(w: TargetWire) -> Result<Self, Self::Error> {
        Self::new(&w.url, w.goal)
    }
}
/// Normalize a goal for deterministic matching and caching.
pub fn normalize_goal(goal: &str) -> String {
    let mut normalized = String::with_capacity(goal.len());
    for word in goal.split_whitespace() {
        if !normalized.is_empty() {
            normalized.push(' ');
        }
        normalized.push_str(word);
    }
    if normalized.is_ascii() {
        normalized.make_ascii_lowercase();
        normalized
    } else {
        normalized.to_lowercase()
    }
}
impl Target {
    /// Validate an absolute HTTP(S) URL, remove fragments, empty queries, and
    /// non-root trailing slashes. Nonempty queries and encoded slashes survive.
    pub fn new(url: &str, goal: Option<String>) -> Result<Self, ReifyError> {
        let url = Url::parse(url).map_err(|e| ReifyError::InvalidTargetUrl(e.to_string()))?;
        Self::from_url(url, goal)
    }
    /// Normalize an already parsed URL without parsing and allocating it again.
    pub fn from_url(mut url: Url, goal: Option<String>) -> Result<Self, ReifyError> {
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(ReifyError::InvalidTargetUrl(
                "expected absolute HTTP or HTTPS URL".into(),
            ));
        }
        url.set_fragment(None);
        if url.query() == Some("") {
            url.set_query(None);
        }
        let path = url.path().trim_end_matches('/');
        if path != url.path() {
            let path = if path.is_empty() {
                "/".to_owned()
            } else {
                path.to_owned()
            };
            url.set_path(&path);
        }
        let goal = goal.map(|g| normalize_goal(&g)).filter(|g| !g.is_empty());
        Ok(Self { url, goal })
    }
}
/// Browser request initiator category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceType {
    /// XMLHttpRequest.
    Xhr,
    /// Fetch API.
    Fetch,
    /// WebSocket connection.
    WebSocket,
    /// All other browser resource types.
    Other,
}
/// One observed upstream exchange. Header lookup helpers are case-insensitive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapturedFrame {
    /// Globally unique capture identifier, typically `Uuid::new_v4()`.
    pub id: Uuid,
    /// Request URL.
    pub url: Url,
    /// HTTP method.
    pub method: String,
    /// Observed request headers.
    pub request_headers: HashMap<String, String>,
    /// Parsed JSON request body, when available.
    pub request_body: Option<Value>,
    /// HTTP response status.
    pub status_code: u16,
    /// Observed response headers.
    pub response_headers: HashMap<String, String>,
    /// Parsed JSON response body, when available.
    pub response_body: Option<Value>,
    /// Capture time in UTC.
    pub timestamp: DateTime<Utc>,
    /// Browser initiator category.
    pub resource_type: ResourceType,
}
fn has_json_mime(headers: &HashMap<String, String>) -> bool {
    headers.iter().any(|(k, v)| {
        if !k.eq_ignore_ascii_case("content-type") {
            return false;
        }
        let mime = v.split(';').next().unwrap_or("").trim();
        let Some((kind, subtype)) = mime.split_once('/') else {
            return false;
        };
        !kind.is_empty()
            && !subtype.contains('/')
            && (subtype.eq_ignore_ascii_case("json")
                || subtype
                    .get(subtype.len().saturating_sub(5)..)
                    .is_some_and(|s| s.eq_ignore_ascii_case("+json")))
    })
}
fn graphql_body(value: &Value) -> bool {
    match value {
        Value::Object(o) => {
            o.get("query").is_some_and(Value::is_string)
                || o.get("extensions")
                    .and_then(|e| e.get("persistedQuery"))
                    .is_some_and(Value::is_object)
        }
        Value::Array(a) => !a.is_empty() && a.iter().all(graphql_body),
        _ => false,
    }
}
impl CapturedFrame {
    /// Detect JSON MIME types, including structured `+json` suffixes. An already
    /// parsed response body also establishes JSON when Content-Type is absent/wrong.
    pub fn is_json_response(&self) -> bool {
        self.response_body.is_some() || has_json_mime(&self.response_headers)
    }
    /// Heuristically detect GraphQL endpoints, query bodies (including batches),
    /// persisted queries, and GET query parameters. Does not parse GraphQL syntax.
    pub fn is_graphql(&self) -> bool {
        self.url.path_segments().is_some_and(|segments| {
            segments
                .into_iter()
                .any(|s| s.eq_ignore_ascii_case("graphql"))
        }) || self.request_body.as_ref().is_some_and(graphql_body)
            || self.url.query_pairs().any(|(k, v)| {
                (k == "query" && !v.trim().is_empty())
                    || (k == "extensions"
                        && serde_json::from_str::<Value>(&v)
                            .is_ok_and(|e| e.get("persistedQuery").is_some_and(Value::is_object)))
            })
    }
}
/// Per-execution credentials. Intentionally does not implement Debug to avoid
/// accidentally logging secrets. Serialization is for trusted bridge transport.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthContext {
    /// Cookie names and values.
    pub cookies: HashMap<String, String>,
    /// Complete Authorization header value.
    pub authorization_header: Option<String>,
    /// CSRF token names and values.
    pub csrf_tokens: HashMap<String, String>,
    /// Additional upstream headers.
    pub custom_headers: HashMap<String, String>,
    /// Credential expiry, if known.
    pub expires_at: Option<DateTime<Utc>>,
}
/// A discoverable tool paired with its upstream execution plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledTool {
    /// Public tool signature.
    pub definition: ToolDefinition,
    /// Upstream request construction rules.
    pub execution: ExecutionSpec,
}
/// Declarative execution configuration; interpolation belongs to the executor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionSpec {
    /// URL template containing executor-defined placeholders.
    pub upstream_url_template: String,
    /// Upstream HTTP method.
    pub http_method: String,
    /// Tool argument name to upstream query parameter name.
    pub query_param_mappings: HashMap<String, String>,
    /// Optional request JSON template.
    pub body_template: Option<Value>,
    /// Credential keys the executor must resolve before making a request.
    pub required_auth_keys: Vec<String>,
}
