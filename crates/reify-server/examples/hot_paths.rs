//! Reproducible warm HTTP fixture; no browser, sockets, retries, or rejected responses.
use axum::body::{Body, to_bytes};
use http::Request;
use reify_core::{
    cache::CacheKey,
    domain::{AuthContext, CapturedFrame, ResourceType, Target},
    mcp::PROTOCOL_VERSION,
};
use reify_server::{ReifyServer, store::CachedApp};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    hint::black_box,
    sync::Arc,
    time::{Duration, Instant},
};
use tower::ServiceExt;
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let iterations = std::env::args()
        .nth(1)
        .map(|s| s.parse().unwrap())
        .unwrap_or(1000usize);
    assert!(iterations > 0);
    let target = Target::new("https://example.com", Some("items".into())).unwrap();
    let body: Value = (0..64)
        .map(|i| (format!("field_{i}"), json!({"value":i,"label":"benchmark"})))
        .collect::<serde_json::Map<_, _>>()
        .into();
    let frame = CapturedFrame {
        id: uuid::Uuid::new_v4(),
        url: "https://example.com/api/items/123?q=abc".parse().unwrap(),
        method: "POST".into(),
        request_headers: HashMap::new(),
        request_body: Some(body),
        status_code: 200,
        response_headers: HashMap::new(),
        response_body: Some(json!({"items":[]})),
        timestamp: chrono::Utc::now(),
        resource_type: ResourceType::Fetch,
    };
    let tools = reify_synth::SynthEngine::new()
        .synthesize(&target, &[frame], &AuthContext::default())
        .unwrap()
        .tools;
    let server = ReifyServer::new(
        Arc::new(reify_probe::ProbeEngine::new()),
        Arc::new(reify_synth::SynthEngine::new()),
    );
    let key = CacheKey::from_target(&target);
    server
        .store()
        .insert(
            key,
            CachedApp {
                target: target.clone(),
                tools,
                auth: AuthContext::default(),
                created_at: Instant::now(),
                ttl: Duration::from_secs(3600),
            },
        )
        .await;
    let mounted = server.mount(target).await.unwrap();
    assert!(mounted.cache_hit);
    if std::env::var_os("REIFY_PROFILE").is_some() {
        // A separate diagnostic run quantifies work formerly performed for every
        // response. This scope intentionally excludes the route measurements.
        let start = Instant::now();
        for _ in 0..iterations {
            let mut definitions = vec![reify_server::handler::meta_tool().to_mcp_value().unwrap()];
            definitions.extend(
                mounted
                    .app
                    .tools
                    .iter()
                    .map(|tool| tool.definition.to_mcp_value().unwrap()),
            );
            black_box(serde_json::to_vec(&definitions).unwrap());
        }
        eprintln!(
            "definition_materialization_ns_per_request={}",
            start.elapsed().as_nanos() / iterations as u128
        );
    }
    let request=serde_json::to_vec(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":PROTOCOL_VERSION,"reify/mount":mounted.token}}})).unwrap();
    let router = server.router();
    let mut total = 0usize;
    for i in 0..iterations + 1 {
        let start = Instant::now();
        let response = router
            .clone()
            .oneshot(
                Request::post("/mcp")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .header("mcp-method", "tools/list")
                    .header("mcp-protocol-version", PROTOCOL_VERSION)
                    .body(Body::from(request.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_success());
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let elapsed = start.elapsed();
        if i == 0 || i == iterations {
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(value["result"]["tools"].as_array().unwrap().len(), 2);
            assert_eq!(
                value["result"]["tools"][1]["inputSchema"]["properties"]["body"]["properties"]
                    .as_object()
                    .unwrap()
                    .len(),
                64
            );
            if i == 0
                && let Some(path) = std::env::var_os("REIFY_DEFINITION_OUT")
            {
                std::fs::write(path, serde_json::to_vec(&value["result"]["tools"]).unwrap())
                    .unwrap();
            }
        }
        if i > 0 {
            total += elapsed.as_nanos() as usize;
        }
        black_box(bytes);
    }
    println!(
        "warm_tools_list_ns_per_request={} iterations={iterations} errors=0",
        total / iterations
    );
}
