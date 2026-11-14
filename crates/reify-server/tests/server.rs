use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    routing::post,
};
use http::{Request, StatusCode};
use reify_core::{
    ReifyError,
    cache::CacheKey,
    domain::{AuthContext, CapturedFrame, ResourceType, Target},
    mcp::{ContentBlock, PROTOCOL_VERSION},
};
use reify_server::{
    AppBuilder, ReifyServer,
    healing::{DriftEvent, SelfHealingCoordinator, detect},
    store::{CachedApp, ServerStore},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tower::ServiceExt;

struct FixtureBuilder {
    calls: AtomicUsize,
    fail: AtomicBool,
    delay: Duration,
}
impl FixtureBuilder {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            fail: AtomicBool::new(false),
            delay: Duration::from_millis(10),
        })
    }
}
fn fixture(target: Target, version: usize) -> CachedApp {
    let url = format!(
        "{}/api/items/123?q=observed",
        target.url.origin().ascii_serialization()
    );
    let frame = CapturedFrame {
        id: uuid::Uuid::new_v4(),
        url: url.parse().unwrap(),
        method: "POST".into(),
        request_headers: HashMap::new(),
        request_body: Some(json!({"enabled":true})),
        status_code: 200,
        response_headers: HashMap::new(),
        response_body: Some(json!({"items":[]})),
        timestamp: chrono::Utc::now(),
        resource_type: ResourceType::Fetch,
    };
    let auth = AuthContext {
        cookies: HashMap::from([("session".into(), "cookie-secret".into())]),
        authorization_header: Some("Bearer auth-secret".into()),
        csrf_tokens: HashMap::from([("x-csrf-token".into(), "csrf-secret".into())]),
        custom_headers: HashMap::from([("x-api-key".into(), "api-secret".into())]),
        ..Default::default()
    };
    let mut tools = reify_synth::SynthEngine::new()
        .synthesize(&target, &[frame], &auth)
        .unwrap()
        .tools;
    tools[0].definition.name = format!("items_v{version}");
    CachedApp {
        target,
        tools,
        auth,
        created_at: Instant::now(),
        ttl: Duration::from_secs(60),
    }
}
impl AppBuilder for FixtureBuilder {
    fn build(
        &self,
        target: Target,
    ) -> Pin<Box<dyn Future<Output = Result<CachedApp, ReifyError>> + Send + '_>> {
        Box::pin(async move {
            let version = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            tokio::time::sleep(self.delay).await;
            if self.fail.load(Ordering::SeqCst) {
                Err(ReifyError::ExecutionError(
                    "fixture upstream unavailable".into(),
                ))
            } else {
                Ok(fixture(target, version))
            }
        })
    }
}
fn rpc(method: &str, mut params: Value) -> Value {
    params["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!(PROTOCOL_VERSION);
    params["_meta"]["io.modelcontextprotocol/clientCapabilities"] = json!({});
    json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
}
fn request(value: &Value) -> Request<Body> {
    let mut builder = Request::post("/mcp")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", PROTOCOL_VERSION)
        .header("Mcp-Method", value["method"].as_str().unwrap());
    if value["method"] == "tools/call" {
        builder = builder.header("Mcp-Name", value["params"]["name"].as_str().unwrap());
    }
    builder.body(Body::from(value.to_string())).unwrap()
}
async fn call(server: &ReifyServer, value: Value) -> (StatusCode, http::HeaderMap, Value) {
    let response = server.router().oneshot(request(&value)).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    (status, headers, body)
}
async fn mount(server: &ReifyServer, url: &str) -> String {
    let (status, _, body) = call(
        server,
        rpc(
            "tools/call",
            json!({"name":"reify","arguments":{"url":url,"goal":"items"}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"]["isError"], false, "{body}");
    body["result"]["structuredContent"]["mount"]
        .as_str()
        .unwrap()
        .to_owned()
}
#[tokio::test]
async fn test_initialize_and_tools_list() {
    let server = ReifyServer::with_builder(FixtureBuilder::new());
    for method in ["initialize", "server/discover", "tools/list"] {
        let (status, headers, body) = call(&server, rpc(method, json!({}))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers["mcp-protocol-version"], PROTOCOL_VERSION);
        assert_eq!(headers["mcp-stream"], "false");
        assert_eq!(headers["cache-control"], "no-store");
        assert_eq!(
            body["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
            "reify"
        );
        if method == "initialize" {
            assert_eq!(body["result"]["protocolVersion"], PROTOCOL_VERSION);
        }
        if method == "tools/list" {
            assert_eq!(body["result"]["tools"].as_array().unwrap().len(), 1);
            assert_eq!(body["result"]["tools"][0]["name"], "reify");
        }
    }
}
#[tokio::test]
async fn test_meta_tool_bootstrap() {
    let builder = FixtureBuilder::new();
    let server = ReifyServer::with_builder(builder.clone());
    let token = mount(&server, "http://example.test").await;
    let (_, _, body) = call(
        &server,
        rpc("tools/list", json!({"_meta":{"reify/mount":token}})),
    )
    .await;
    let tools = body["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 2);
    for value in tools {
        let tool: reify_core::mcp::ToolDefinition = serde_json::from_value(value.clone()).unwrap();
        tool.validate().unwrap();
    }
    assert_eq!(builder.calls.load(Ordering::SeqCst), 1);
    assert!(!body.to_string().contains("auth-secret"));
    // A different unscoped request does not inherit this mount.
    assert_eq!(
        call(&server, rpc("tools/list", json!({}))).await.2["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
#[tokio::test]
async fn test_cache_hit_latency() {
    let builder = FixtureBuilder::new();
    let server = ReifyServer::with_builder(builder.clone());
    let expected = mount(&server, "http://example.test").await;
    // Warm, in-process full HTTP dispatch. This is a smoke budget rather than a
    // production throughput benchmark; every sample verifies successful work.
    let mut timings = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        let token = mount(&server, "http://example.test").await;
        timings.push(start.elapsed());
        assert_eq!(token, expected);
    }
    timings.sort();
    assert!(
        timings[2] < Duration::from_millis(10),
        "warm-route median {:?}",
        timings[2]
    );
    assert_eq!(builder.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn simultaneous_cache_misses_compile_once() {
    let builder = FixtureBuilder::new();
    let server = ReifyServer::with_builder(builder.clone());
    let (a, b, c) = tokio::join!(
        mount(&server, "http://example.test"),
        mount(&server, "http://example.test"),
        mount(&server, "http://example.test")
    );
    assert_eq!(a, b);
    assert_eq!(b, c);
    assert_eq!(builder.calls.load(Ordering::SeqCst), 1);
}
#[derive(Clone)]
struct Upstream {
    status: Arc<AtomicUsize>,
    hits: Arc<AtomicUsize>,
}
async fn upstream(
    State(state): State<Upstream>,
    headers: http::HeaderMap,
    uri: http::Uri,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let parsed = url::Url::parse(&format!("http://fixture.test{uri}")).unwrap();
    let query: HashMap<_, _> = parsed
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    state.hits.fetch_add(1, Ordering::SeqCst);
    assert_eq!(headers["authorization"], "Bearer auth-secret");
    assert_eq!(headers["cookie"], "session=cookie-secret");
    assert_eq!(headers["x-csrf-token"], "csrf-secret");
    assert_eq!(headers["x-api-key"], "api-secret");
    assert_eq!(query["q"], "hello & world");
    assert_eq!(body, json!({"enabled":false}));
    (
        StatusCode::from_u16(state.status.load(Ordering::SeqCst) as u16).unwrap(),
        Json(json!({"items":[1,2]})),
    )
}
async fn upstream_server() -> (String, Upstream, tokio::task::JoinHandle<()>) {
    let state = Upstream {
        status: Arc::new(AtomicUsize::new(200)),
        hits: Arc::new(AtomicUsize::new(0)),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let router = Router::new()
        .route("/api/items/{item_id}", post(upstream))
        .with_state(state.clone());
    let job = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (url, state, job)
}
fn invocation(token: &str, name: &str) -> Value {
    rpc(
        "tools/call",
        json!({"name":name,"arguments":{"item_id":"456","q":"hello & world","body":{"enabled":false}},"_meta":{"reify/mount":token}}),
    )
}
#[tokio::test]
async fn test_tool_execution_proxy() {
    let (url, state, job) = upstream_server().await;
    let server = ReifyServer::with_builder(FixtureBuilder::new());
    let token = mount(&server, &url).await;
    let (status, headers, body) = call(&server, invocation(&token, "items_v1")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"]["isError"], false);
    assert_eq!(headers["mcp-ttl-ms"], "0");
    let data: Value =
        serde_json::from_str(body["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(data["items"], json!([1, 2]));
    assert_eq!(state.hits.load(Ordering::SeqCst), 1);
    job.abort();
}
#[tokio::test]
async fn test_drift_detection_and_self_healing() {
    let (url, state, job) = upstream_server().await;
    state.status.store(401, Ordering::SeqCst);
    let builder = FixtureBuilder::new();
    let server = ReifyServer::with_builder(builder.clone());
    let token = mount(&server, &url).await;
    let (_, _, body) = call(&server, invocation(&token, "items_v1")).await;
    assert_eq!(body["result"]["isError"], true);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let (_, _, body) = call(
                &server,
                rpc("tools/list", json!({"_meta":{"reify/mount":token}})),
            )
            .await;
            if body["result"]["tools"][1]["name"] == "items_v2" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(builder.calls.load(Ordering::SeqCst), 2);
    state.status.store(404, Ordering::SeqCst);
    let (_, _, body) = call(&server, invocation(&token, "items_v2")).await;
    assert_eq!(body["result"]["isError"], true);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(builder.calls.load(Ordering::SeqCst), 2);
    job.abort();
}
#[tokio::test]
async fn store_expiry_and_generation_compare_swap() {
    let store = ServerStore::new();
    let target = Target::new("https://example.test", None).unwrap();
    let key = CacheKey::from_target(&target);
    store.insert(key, fixture(target.clone(), 1)).await;
    let old = store.snapshot(&key).await.unwrap();
    store.insert(key, fixture(target.clone(), 2)).await;
    assert!(
        !store
            .replace_if_current(key, &old, fixture(target.clone(), 3))
            .await
    );
    store.invalidate(&key).await;
    assert!(
        !store
            .replace_if_current(key, &old, fixture(target.clone(), 3))
            .await
    );
    let mut expired = fixture(target, 4);
    expired.ttl = Duration::ZERO;
    store.insert(key, expired).await;
    assert!(store.get(&key).await.is_none());
}
#[tokio::test]
async fn failed_healing_keeps_previous_generation_and_coalesces_events() {
    let builder = FixtureBuilder::new();
    builder.fail.store(true, Ordering::SeqCst);
    let store = ServerStore::new();
    let target = Target::new("https://example.test", None).unwrap();
    let key = CacheKey::from_target(&target);
    store.insert(key, fixture(target, 1)).await;
    let old = store.current(&key).await.unwrap();
    let healing =
        SelfHealingCoordinator::new(store.clone(), builder.clone(), Duration::from_secs(30));
    for _ in 0..10 {
        assert!(healing.submit(DriftEvent::AuthDrift { key, status: 401 }));
    }
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(builder.calls.load(Ordering::SeqCst), 1);
    assert!(Arc::ptr_eq(&old, &store.current(&key).await.unwrap()));
}
#[test]
fn drift_classification_ignores_business_validation() {
    let key = CacheKey::from_target(&Target::new("https://example.test", None).unwrap());
    assert!(matches!(
        detect(key, 401, &[]),
        Some(DriftEvent::AuthDrift { .. })
    ));
    assert!(matches!(
        detect(key, 404, &[]),
        Some(DriftEvent::SchemaDrift { .. })
    ));
    assert!(
        detect(
            key,
            400,
            &[ContentBlock::Text {
                text: "unknown field: flight_id".into()
            }]
        )
        .is_some()
    );
    assert!(
        detect(
            key,
            400,
            &[ContentBlock::Text {
                text: "insufficient funds".into()
            }]
        )
        .is_none()
    );
    assert!(detect(key, 500, &[]).is_none());
}
#[tokio::test]
async fn malformed_requests_origins_and_headers_are_rejected() {
    let server = ReifyServer::with_builder(FixtureBuilder::new());
    let value = rpc("tools/list", json!({}));
    let mut req = request(&value);
    req.headers_mut().insert(
        "origin",
        http::HeaderValue::from_static("https://evil.test"),
    );
    assert_eq!(
        server.router().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let mut req = request(&value);
    req.headers_mut()
        .insert("mcp-method", http::HeaderValue::from_static("tools/call"));
    let response = server.router().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["error"]["code"], -32020);
    let mut req = request(&value);
    *req.body_mut() = Body::from("{");
    let response = server.router().oneshot(req).await.unwrap();
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["error"]["code"], -32700);
    assert_eq!(body["id"], Value::Null);
    assert_eq!(
        call(&server, rpc("unknown", json!({}))).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &server,
            rpc("tools/list", json!({"_meta":{"reify/mount":"missing"}}))
        )
        .await
        .2["error"]["code"],
        -32602
    );
}

#[tokio::test]
async fn schema_404_rebuilds_without_retrying_the_upstream_mutation() {
    let (url, state, job) = upstream_server().await;
    state.status.store(404, Ordering::SeqCst);
    let builder = FixtureBuilder::new();
    let server = ReifyServer::with_builder(builder.clone());
    let token = mount(&server, &url).await;
    assert_eq!(
        call(&server, invocation(&token, "items_v1")).await.2["result"]["isError"],
        true
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let (_, _, body) = call(
                &server,
                rpc("tools/list", json!({"_meta":{"reify/mount":token}})),
            )
            .await;
            if body["result"]["tools"][1]["name"] == "items_v2" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(state.hits.load(Ordering::SeqCst), 1);
    assert_eq!(builder.calls.load(Ordering::SeqCst), 2);
    job.abort();
}
#[tokio::test]
async fn invalid_arguments_and_missing_mount_never_touch_upstream() {
    let (url, state, job) = upstream_server().await;
    let server = ReifyServer::with_builder(FixtureBuilder::new());
    let token = mount(&server, &url).await;
    let mut invalid = invocation(&token, "items_v1");
    invalid["params"]["arguments"]["body"]["enabled"] = json!("not boolean");
    assert_eq!(call(&server, invalid).await.2["error"]["code"], -32602);
    assert_eq!(
        call(&server, invocation("unknown", "items_v1")).await.2["error"]["code"],
        -32602
    );
    assert_eq!(state.hits.load(Ordering::SeqCst), 0);
    job.abort();
}
#[tokio::test]
async fn expired_credentials_never_read_as_fresh() {
    let target = Target::new("https://example.test", None).unwrap();
    let key = CacheKey::from_target(&target);
    let mut app = fixture(target, 1);
    app.auth.expires_at = Some(chrono::Utc::now() - chrono::Duration::seconds(1));
    let store = ServerStore::new();
    store.insert(key, app).await;
    assert!(store.snapshot(&key).await.is_none());
}

#[tokio::test]
async fn static_mount_header_lists_scoped_tools_without_custom_client_metadata() {
    let server = ReifyServer::with_builder(FixtureBuilder::new());
    let token = mount(&server, "http://example.test").await;
    let value = rpc("tools/list", json!({}));
    let mut request = request(&value);
    request
        .headers_mut()
        .insert("reify-mount", http::HeaderValue::from_str(&token).unwrap());
    let response = server.router().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(body["result"]["tools"].as_array().unwrap().len(), 2);
}
