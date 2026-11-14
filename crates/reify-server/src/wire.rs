//! Borrowed response views stream cached definition JSON without rebuilding trees.
use axum::{
    Json,
    response::{IntoResponse, Response},
};
use http::{HeaderValue, StatusCode};
use reify_core::mcp::{CacheHint, CacheScope, PROTOCOL_VERSION};
use serde::{Serialize, Serializer, ser::SerializeSeq};
use serde_json::{Value, value::RawValue};
use std::{sync::LazyLock, time::Duration};
#[derive(Serialize)]
pub(crate) struct ServerInfo {
    name: &'static str,
    version: &'static str,
}
pub(crate) fn info() -> ServerInfo {
    ServerInfo {
        name: "reify",
        version: env!("CARGO_PKG_VERSION"),
    }
}
#[derive(Serialize)]
struct Meta {
    #[serde(rename = "io.modelcontextprotocol/serverInfo")]
    server: ServerInfo,
    #[serde(rename = "reify/cacheHint")]
    cache: CacheHint,
}
fn meta(ttl: Duration, scope: CacheScope) -> Meta {
    Meta {
        server: info(),
        cache: CacheHint {
            ttl_ms: ttl.as_millis().min(u64::MAX as u128) as u64,
            cache_scope: scope,
            revalidate_in_background: false,
        },
    }
}
static ROOT: LazyLock<Box<RawValue>> = LazyLock::new(|| {
    crate::handler::meta_tool()
        .to_mcp_json()
        .expect("static meta tool")
});
struct Tools<'a> {
    definitions: &'a [Box<RawValue>],
    root: bool,
}
impl Serialize for Tools<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.definitions.len() + usize::from(self.root)))?;
        if self.root {
            seq.serialize_element(&*ROOT)?;
        }
        for tool in self.definitions {
            seq.serialize_element(tool)?;
        }
        seq.end()
    }
}
#[derive(Serialize)]
struct Envelope<'a, T> {
    jsonrpc: &'static str,
    id: &'a Value,
    result: T,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct List<'a> {
    result_type: &'static str,
    tools: Tools<'a>,
    ttl_ms: u64,
    cache_scope: &'static str,
    #[serde(rename = "_meta")]
    meta: Meta,
}
pub(crate) fn list(
    id: &Value,
    definitions: &[Box<RawValue>],
    ttl: Duration,
    scope: CacheScope,
) -> Response {
    finish(
        Json(Envelope {
            jsonrpc: "2.0",
            id,
            result: List {
                result_type: "complete",
                tools: Tools {
                    definitions,
                    root: true,
                },
                ttl_ms: ttl.as_millis().min(u64::MAX as u128) as u64,
                cache_scope: "private",
                meta: meta(ttl, scope),
            },
        })
        .into_response(),
        ttl,
        scope,
    )
}
#[derive(Serialize)]
struct Text {
    r#type: &'static str,
    text: &'static str,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Mounted<'a> {
    mount: &'a str,
    tools: Tools<'a>,
    primary_tool_name: Option<&'a str>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Bootstrap<'a> {
    result_type: &'static str,
    content: [Text; 1],
    is_error: bool,
    structured_content: Mounted<'a>,
    #[serde(rename = "_meta")]
    meta: Meta,
}
pub(crate) fn bootstrap(
    id: &Value,
    definitions: &[Box<RawValue>],
    token: &str,
    primary: Option<&str>,
    ttl: Duration,
    scope: CacheScope,
) -> Response {
    let result = Bootstrap {
        result_type: "complete",
        content: [Text {
            r#type: "text",
            text: "Tools are mounted. Include the returned mount token as params._meta[\"reify/mount\"] on tools/list and tools/call requests.",
        }],
        is_error: false,
        structured_content: Mounted {
            mount: token,
            tools: Tools {
                definitions,
                root: false,
            },
            primary_tool_name: primary,
        },
        meta: meta(ttl, scope),
    };
    finish(
        Json(Envelope {
            jsonrpc: "2.0",
            id,
            result,
        })
        .into_response(),
        ttl,
        scope,
    )
}
pub(crate) fn finish(mut response: Response, ttl: Duration, scope: CacheScope) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        "mcp-protocol-version",
        HeaderValue::from_static(PROTOCOL_VERSION),
    );
    headers.insert(
        "mcp-cache-scope",
        HeaderValue::from_static(match scope {
            CacheScope::Task => "task",
            CacheScope::Domain => "domain",
            CacheScope::Global => "global",
        }),
    );
    headers.insert(
        "mcp-ttl-ms",
        HeaderValue::from_str(&ttl.as_millis().min(u64::MAX as u128).to_string())
            .expect("decimal TTL"),
    );
    headers.insert("mcp-stream", HeaderValue::from_static("false"));
    headers.insert("cache-control", HeaderValue::from_static("no-store"));
    response
}
pub(crate) fn invalid_cache(id: Value) -> Response {
    crate::handler::error(
        StatusCode::INTERNAL_SERVER_ERROR,
        id,
        -32603,
        "Invalid cached tool schema",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn borrowed_wire_matches_owned_results_without_losing_json_escaping() {
        let id = serde_json::json!("id \" 世界");
        let ttl = Duration::from_millis(123);
        let scope = CacheScope::Domain;
        let raw = crate::handler::meta_tool().to_mcp_json().unwrap();
        let definitions = vec![raw];
        let response = list(&id, &definitions, ttl, scope);
        assert_eq!(response.headers()["mcp-cache-scope"], "domain");
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["id"], id);
        assert_eq!(value["result"]["tools"].as_array().unwrap().len(), 2);
        assert_eq!(value["result"]["ttlMs"], 123);
        let response = bootstrap(&id, &definitions, "token", Some("reify"), ttl, scope);
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            value["result"]["structuredContent"]["tools"][0],
            crate::handler::meta_tool().to_mcp_value().unwrap()
        );
        assert_eq!(
            value["result"]["_meta"]["reify/cacheHint"]["cacheScope"],
            "domain"
        );
        assert_eq!(value["result"]["isError"], false);
    }
}
