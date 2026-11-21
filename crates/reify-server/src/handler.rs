//! POST dispatch and explicit per-request mount routing. No connection sessions.
use crate::{healing, proxy::UpstreamProxy, server::State, store::CachedApp};
use axum::{
    Json,
    body::Bytes,
    extract::State as ExtractState,
    response::{IntoResponse, Response, sse::Event},
};
use futures_util::{StreamExt, stream};
use http::{HeaderMap, StatusCode};
use reify_core::{
    ReifyError,
    cache::CacheKey,
    domain::Target,
    mcp::{
        CacheHint, CacheScope, ContentBlock, McpHeaders, McpRequest, PROTOCOL_VERSION, RequestId,
        ToolCallResult, ToolDefinition,
    },
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

/// Return the always-available bootstrap tool, with a strict input schema.
pub fn meta_tool() -> ToolDefinition {
    ToolDefinition {name:"reify".into(),description:"Compile an HTTP(S) website into task-scoped API tools. Embedded profiles skip discovery; fresh forces live capture. Return its mount token on subsequent calls.".into(),input_schema:json!({"type":"object","properties":{"url":{"type":"string","format":"uri"},"goal":{"type":["string","null"]},"fresh":{"type":"boolean","default":false}},"required":["url"],"additionalProperties":false}),metadata:None}
}
fn server_info() -> Value {
    json!({"name":"reify","version":env!("CARGO_PKG_VERSION")})
}
fn response(
    status: StatusCode,
    id: Value,
    result: Result<Value, (i64, &str, Option<Value>)>,
    ttl: Duration,
) -> Response {
    let (body, scope) = rpc_body(id, result);
    crate::wire::finish((status, Json(body)).into_response(), ttl, scope)
}
fn rpc_body(id: Value, result: Result<Value, (i64, &str, Option<Value>)>) -> (Value, CacheScope) {
    let scope = match result
        .as_ref()
        .ok()
        .and_then(|v| v.pointer("/_meta/reify~1cacheHint/cacheScope"))
        .and_then(Value::as_str)
    {
        Some("domain") => CacheScope::Domain,
        _ => CacheScope::Task,
    };
    let body = match result {
        Ok(mut result) => {
            result["_meta"]["io.modelcontextprotocol/serverInfo"] = server_info();
            let mut body = serde_json::Map::new();
            body.insert("jsonrpc".into(), Value::String("2.0".into()));
            body.insert("id".into(), id);
            body.insert("result".into(), result);
            Value::Object(body)
        }
        Err((code, message, data)) => {
            let mut error = json!({"code":code,"message":message});
            if let Some(data) = data {
                error["data"] = data;
            }
            let mut body = serde_json::Map::new();
            body.insert("jsonrpc".into(), Value::String("2.0".into()));
            body.insert("id".into(), id);
            body.insert("error".into(), error);
            Value::Object(body)
        }
    };
    (body, scope)
}

struct ToolSseStream {
    chunks: crate::proxy::UpstreamChunks,
    collected: Vec<u8>,
    status: u16,
    chunk_index: usize,
    id: Value,
    key: CacheKey,
    cached: Arc<CachedApp>,
    state: Arc<State>,
    done: bool,
}
impl ToolSseStream {
    async fn next(mut self) -> Option<(Result<Event, std::convert::Infallible>, Self)> {
        if self.done {
            return None;
        }
        if let Some(chunk) = self.chunks.next().await {
            match chunk {
                Ok(chunk) => {
                    self.collected.extend_from_slice(&chunk);
                    self.chunk_index += 1;
                    let notification = json!({
                        "jsonrpc":"2.0",
                        "method":"notifications/message",
                        "params":{
                            "level":"info",
                            "logger":"reify.upstream",
                            "data":{
                                "chunk":String::from_utf8_lossy(&chunk),
                                "index":self.chunk_index
                            }
                        }
                    });
                    return Some((Ok(crate::wire::sse_message(&notification)), self));
                }
                Err(message) => return Some((Ok(self.final_event(Some(message)).await), self)),
            }
        }
        Some((Ok(self.final_event(None).await), self))
    }
    async fn final_event(&mut self, read_error: Option<&str>) -> Event {
        self.done = true;
        let mut upstream = match read_error {
            Some(message) => crate::proxy::body_failure(self.status, message),
            None => crate::proxy::body_result(self.status, std::mem::take(&mut self.collected)),
        };
        if let Some(event) = healing::detect(self.key, upstream.status, &upstream.result.content)
            && self
                .state
                .store
                .current(&self.key)
                .await
                .is_some_and(|current| Arc::ptr_eq(&current, &self.cached))
        {
            self.state.healing.submit(event);
        }
        upstream.result.cache_hint = Some(hint(Duration::ZERO, app_scope(&self.cached)));
        let result = upstream
            .result
            .into_mcp_value()
            .expect("captured JSON serializes");
        let (body, _) = rpc_body(self.id.clone(), Ok(result));
        crate::wire::sse_message(&body)
    }
}

pub(crate) fn error(status: StatusCode, id: Value, code: i64, message: &str) -> Response {
    response(status, id, Err((code, message, None)), Duration::ZERO)
}
fn accepts(headers: &HeaderMap, mime: &str) -> bool {
    quality(headers, mime) > 0.0
}
fn quality(headers: &HeaderMap, mime: &str) -> f32 {
    let mut exact = None::<f32>;
    let mut wildcard = None::<f32>;
    for part in headers
        .get_all("accept")
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|h| h.split(','))
    {
        let mut pieces = part.trim().split(';');
        let Some(media) = pieces.next().map(str::trim) else {
            continue;
        };
        let mut q = 1.0;
        let mut valid = true;
        for parameter in pieces {
            if let Some((name, value)) = parameter.trim().split_once('=')
                && name.trim().eq_ignore_ascii_case("q")
            {
                match value.trim().parse::<f32>() {
                    Ok(parsed) if (0.0..=1.0).contains(&parsed) => q = parsed,
                    _ => {
                        valid = false;
                        break;
                    }
                }
            }
        }
        if !valid {
            continue;
        }
        if media.eq_ignore_ascii_case(mime) {
            exact = Some(exact.map_or(q, |current| current.max(q)));
        } else if media == "*/*" {
            wildcard = Some(wildcard.map_or(q, |current| current.max(q)));
        }
    }
    exact.or(wildcard).unwrap_or(0.0)
}
fn prefers_sse(headers: &HeaderMap) -> bool {
    let json = quality(headers, "application/json");
    let sse = quality(headers, "text/event-stream");
    sse > 0.0 && sse > json
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReifyArguments {
    url: String,
    goal: Option<String>,
    #[serde(default)]
    fresh: bool,
}
fn mount_key(
    state: &State,
    request: &McpRequest,
    headers: &HeaderMap,
) -> Result<Option<CacheKey>, ()> {
    let meta = request
        .params
        .as_ref()
        .and_then(|p| p.pointer("/_meta/reify~1mount"));
    let meta = match meta {
        None => None,
        Some(Value::String(s)) => Some(s.as_str()),
        Some(_) => return Err(()),
    };
    let mut values = headers.get_all("reify-mount").iter();
    let header = values
        .next()
        .map(|h| h.to_str().map_err(|_| ()))
        .transpose()?;
    if values.next().is_some() || meta.zip(header).is_some_and(|(a, b)| a != b) {
        return Err(());
    }
    match meta.or(header) {
        None => Ok(None),
        Some(token) => state.resolve(token).map(Some).ok_or(()),
    }
}
pub(crate) async fn handle(
    ExtractState(state): ExtractState<Arc<State>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if headers.get_all("origin").iter().count() > 1
        || headers.get("origin").is_some_and(|h| {
            h.to_str()
                .map_or(true, |s| !state.origins.iter().any(|origin| origin == s))
        })
    {
        return error(
            StatusCode::FORBIDDEN,
            Value::Null,
            -32600,
            "Origin is not allowed",
        );
    }
    let content_type = headers
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.split(';').next());
    if !content_type.is_some_and(|s| s.trim().eq_ignore_ascii_case("application/json")) {
        return error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Value::Null,
            -32600,
            "Content-Type must be application/json",
        );
    }
    let has_accept = headers.get_all("accept").iter().next().is_some();
    if has_accept
        && !accepts(&headers, "application/json")
        && !accepts(&headers, "text/event-stream")
    {
        return error(
            StatusCode::NOT_ACCEPTABLE,
            Value::Null,
            -32600,
            "Accept must allow application/json or text/event-stream",
        );
    }
    let stream_response = has_accept && prefers_sse(&headers);
    let mut request: McpRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => {
            let parsed = serde_json::from_slice::<Value>(&body);
            let id = parsed
                .as_ref()
                .ok()
                .and_then(|p| p.get("id"))
                .cloned()
                .filter(|id| serde_json::from_value::<RequestId>(id.clone()).is_ok())
                .unwrap_or(Value::Null);
            return if parsed.is_err() {
                error(StatusCode::BAD_REQUEST, id, -32700, "Parse error")
            } else {
                error(
                    StatusCode::BAD_REQUEST,
                    id,
                    -32600,
                    "Invalid Request: expected a single request envelope",
                )
            };
        }
    };
    let id = serde_json::to_value(&request.id).expect("JSON-RPC ID serializes");
    let parsed = match McpHeaders::parse(&headers) {
        Ok(parsed) => parsed,
        Err(_) => return error(StatusCode::BAD_REQUEST, id, -32020, "HeaderMismatch"),
    };
    let body_version = request
        .params
        .as_ref()
        .and_then(|p| p.pointer("/_meta/io.modelcontextprotocol~1protocolVersion"))
        .and_then(Value::as_str);
    if parsed.protocol_version.is_some()
        && parsed.protocol_version == body_version
        && parsed.protocol_version != Some(PROTOCOL_VERSION)
    {
        return response(
            StatusCode::BAD_REQUEST,
            id,
            Err((
                -32022,
                "Unsupported protocol version",
                Some(json!({"supported":[PROTOCOL_VERSION],"requested":parsed.protocol_version})),
            )),
            Duration::ZERO,
        );
    }
    if parsed.validate_request(&request).is_err() {
        return error(StatusCode::BAD_REQUEST, id, -32020, "HeaderMismatch");
    }
    match request.method.as_str() {
        "initialize" => response(
            StatusCode::OK,
            id,
            Ok(
                json!({"resultType":"complete","protocolVersion":PROTOCOL_VERSION,"capabilities":{"tools":{}},"serverInfo":server_info()}),
            ),
            Duration::ZERO,
        ),
        "server/discover" => response(
            StatusCode::OK,
            id,
            Ok(
                json!({"resultType":"complete","supportedVersions":[PROTOCOL_VERSION],"capabilities":{"tools":{}},"ttlMs":0,"cacheScope":"private"}),
            ),
            Duration::ZERO,
        ),
        "tools/list" => {
            let key = match mount_key(&state, &request, &headers) {
                Ok(key) => key,
                Err(()) => {
                    return error(
                        StatusCode::BAD_REQUEST,
                        id,
                        -32602,
                        "Unknown or invalid reify/mount token",
                    );
                }
            };
            if let Some(key) = key {
                let prepared = match state.store.prepared(&key).await {
                    Ok(Some(p)) => p,
                    Ok(None) => {
                        return error(
                            StatusCode::BAD_REQUEST,
                            id,
                            -32602,
                            "Mount expired; call reify again",
                        );
                    }
                    Err(_) => return crate::wire::invalid_cache(id),
                };
                let ttl = prepared.app.remaining();
                crate::wire::list(&id, &prepared.definitions, ttl, app_scope(&prepared.app))
            } else {
                crate::wire::list(&id, &[], Duration::ZERO, CacheScope::Task)
            }
        }

        "tools/call" => {
            let route = mount_key(&state, &request, &headers);
            let call = match request.take_tool_call() {
                Ok(call) => call,
                Err(_) => {
                    return error(
                        StatusCode::BAD_REQUEST,
                        id,
                        -32602,
                        "Invalid tool call parameters",
                    );
                }
            };
            if call.name == "reify" {
                let arguments: ReifyArguments =
                    match serde_json::from_value(Value::Object(call.arguments)) {
                        Ok(a) => a,
                        Err(_) => {
                            return error(
                                StatusCode::BAD_REQUEST,
                                id,
                                -32602,
                                "reify requires url, optional goal, and optional fresh boolean",
                            );
                        }
                    };
                let target = match Target::new(&arguments.url, arguments.goal) {
                    Ok(t) => t,
                    Err(_) => {
                        return error(
                            StatusCode::BAD_REQUEST,
                            id,
                            -32602,
                            "Invalid HTTP(S) target URL",
                        );
                    }
                };
                if !target.url.username().is_empty() || target.url.password().is_some() {
                    return error(
                        StatusCode::BAD_REQUEST,
                        id,
                        -32602,
                        "URL credentials are not supported",
                    );
                }
                let mounted = match state.mount_with_policy(target, arguments.fresh).await {
                    Ok(mounted) => mounted,
                    Err(_) => return tool_failure(id, "Website compilation failed or timed out"),
                };
                let prepared = match state.store.prepared(&mounted.key).await {
                    Ok(Some(p)) => p,
                    _ => return tool_failure(id, "Invalid cached tool definitions"),
                };
                let ttl = prepared.app.remaining();
                crate::wire::bootstrap(
                    &id,
                    &prepared.definitions,
                    &mounted.token,
                    prepared
                        .app
                        .tools
                        .first()
                        .map(|t| t.definition.name.as_str()),
                    ttl,
                    app_scope(&prepared.app),
                )
            } else {
                let key = match route {
                    Ok(Some(key)) => key,
                    _ => {
                        return error(
                            StatusCode::BAD_REQUEST,
                            id,
                            -32602,
                            "Compiled calls require a reify/mount token",
                        );
                    }
                };
                let prepared = match state.store.prepared(&key).await {
                    Ok(Some(p)) => p,
                    Ok(None) => {
                        return error(
                            StatusCode::BAD_REQUEST,
                            id,
                            -32602,
                            "Mount expired; call reify again",
                        );
                    }
                    Err(_) => return crate::wire::invalid_cache(id),
                };
                let cached = &prepared.app;
                let Some(index) = cached
                    .tools
                    .iter()
                    .position(|t| t.definition.name == call.name)
                else {
                    return error(
                        StatusCode::BAD_REQUEST,
                        id,
                        -32602,
                        "Tool is not in this mount",
                    );
                };
                let tool = &cached.tools[index];
                let arguments = Value::Object(call.arguments);
                if prepared.mirrors[index].as_ref().map_or(true, |mirrors| {
                    mirrors.validate(&arguments, &headers).is_err()
                }) {
                    return error(
                        StatusCode::BAD_REQUEST,
                        id,
                        -32020,
                        "HeaderMismatch: invalid mirrored tool parameters",
                    );
                }
                let renderer = match prepared.renderer(index) {
                    Ok(r) => r,
                    Err(_) => {
                        return error(
                            StatusCode::BAD_REQUEST,
                            id,
                            -32602,
                            "Invalid compiled tool arguments",
                        );
                    }
                };
                let auth_headers = match &prepared.headers {
                    Ok(h) => h,
                    Err(_) => {
                        return if stream_response {
                            tool_failure_sse(id, "Upstream invocation failed")
                        } else {
                            tool_failure(id, "Upstream invocation failed")
                        };
                    }
                };
                if state.proxy.get().is_none() {
                    match UpstreamProxy::new() {
                        Ok(proxy) => {
                            let _ = state.proxy.set(proxy);
                        }
                        Err(_) => {
                            return if stream_response {
                                tool_failure_sse(id, "Cannot configure upstream client")
                            } else {
                                tool_failure(id, "Cannot configure upstream client")
                            };
                        }
                    };
                }
                let proxy = state.proxy.get().expect("proxy initialized");
                if stream_response {
                    let upstream = match proxy
                        .execute_prepared_streaming(
                            tool,
                            renderer,
                            &arguments,
                            &cached.auth,
                            auth_headers,
                        )
                        .await
                    {
                        Ok(result) => result,
                        Err(ReifyError::SchemaValidationError(_)) => {
                            return error(
                                StatusCode::BAD_REQUEST,
                                id,
                                -32602,
                                "Invalid compiled tool arguments",
                            );
                        }
                        Err(_) => return tool_failure_sse(id, "Upstream invocation failed"),
                    };
                    let stream = stream::unfold(
                        ToolSseStream {
                            chunks: upstream.chunks,
                            collected: Vec::new(),
                            status: upstream.status,
                            chunk_index: 0,
                            id,
                            key,
                            cached: Arc::clone(cached),
                            state: Arc::clone(&state),
                            done: false,
                        },
                        |state| state.next(),
                    );
                    return crate::wire::sse_response(stream, Duration::ZERO, app_scope(cached));
                }
                let mut upstream = match proxy
                    .execute_prepared(tool, renderer, &arguments, &cached.auth, auth_headers)
                    .await
                {
                    Ok(result) => result,
                    Err(ReifyError::SchemaValidationError(_)) => {
                        return error(
                            StatusCode::BAD_REQUEST,
                            id,
                            -32602,
                            "Invalid compiled tool arguments",
                        );
                    }
                    Err(_) => return tool_failure(id, "Upstream invocation failed"),
                };
                if let Some(event) = healing::detect(key, upstream.status, &upstream.result.content)
                    && state
                        .store
                        .current(&key)
                        .await
                        .is_some_and(|current| Arc::ptr_eq(&current, cached))
                {
                    state.healing.submit(event);
                }
                // Mutations and authenticated responses are not cached as reusable data.
                // Hint freshness describes only the compiled mount.
                upstream.result.cache_hint = Some(hint(Duration::ZERO, app_scope(cached)));
                let result = upstream
                    .result
                    .into_mcp_value()
                    .expect("captured JSON serializes");
                response(StatusCode::OK, id, Ok(result), Duration::ZERO)
            }
        }
        _ => error(StatusCode::NOT_FOUND, id, -32601, "Method not found"),
    }
}
fn app_scope(app: &CachedApp) -> CacheScope {
    if app.target.goal.is_some() {
        CacheScope::Task
    } else {
        CacheScope::Domain
    }
}
fn hint(ttl: Duration, scope: CacheScope) -> CacheHint {
    CacheHint {
        ttl_ms: ttl.as_millis().min(u64::MAX as u128) as u64,
        cache_scope: scope,
        revalidate_in_background: false,
    }
}
fn tool_failure(id: Value, message: &str) -> Response {
    response(
        StatusCode::OK,
        id,
        Ok(tool_failure_result(message)),
        Duration::ZERO,
    )
}
fn tool_failure_sse(id: Value, message: &str) -> Response {
    let (body, scope) = rpc_body(id, Ok(tool_failure_result(message)));
    crate::wire::sse_response(
        stream::iter([Ok(crate::wire::sse_message(&body))]),
        Duration::ZERO,
        scope,
    )
}
fn tool_failure_result(message: &str) -> Value {
    ToolCallResult {
        content: vec![ContentBlock::Text {
            text: message.into(),
        }],
        is_error: true,
        cache_hint: None,
    }
    .into_mcp_value()
    .expect("text result serializes")
}
