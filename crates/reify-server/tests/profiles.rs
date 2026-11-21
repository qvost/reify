use reify_core::domain::Target;
use reify_probe::ProbeEngine;
use reify_server::{AppBuilder, BrowserAppBuilder, ReifyServer};
use reify_synth::SynthEngine;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn profile_server(profiles: bool) -> ReifyServer {
    let probe = Arc::new(ProbeEngine::with_chromium(
        "/missing/prewarmed-test-chromium",
    ));
    let builder =
        BrowserAppBuilder::new(probe, Arc::new(SynthEngine::new())).with_profiles(profiles);
    ReifyServer::with_builder(Arc::new(builder))
}

#[tokio::test]
async fn mounts_a_goal_matched_profile_without_chromium_in_under_one_millisecond() {
    reify_server::warm_profiles().unwrap();
    let server = profile_server(true);
    let target = Target::new(
        "https://news.ycombinator.com",
        Some("search stories".into()),
    )
    .unwrap();
    let start = Instant::now();
    let mounted = server.mount(target).await.unwrap();
    let elapsed = start.elapsed();
    eprintln!(
        "pre-warmed Hacker News mount: {:.3}ms",
        elapsed.as_secs_f64() * 1000.0
    );
    let threshold = if cfg!(debug_assertions) {
        Duration::from_millis(10)
    } else {
        Duration::from_millis(1)
    };
    assert!(
        elapsed < threshold,
        "profile mount took {elapsed:?}"
    );
    assert_eq!(mounted.app.tools.len(), 1);
    assert_eq!(mounted.app.tools[0].definition.name, "search_stories");
    assert_eq!(
        mounted.app.tools[0].definition.input_schema["properties"]["q"]["type"],
        "string"
    );
    let prepared = server
        .store()
        .prepared(&mounted.key)
        .await
        .unwrap()
        .unwrap();
    let arguments = serde_json::json!({"q":"rust", "page":1});
    let request = prepared.renderer(0).unwrap().render(&arguments).unwrap();
    assert!(request.url.as_str().contains("query=rust"));
    assert!(request.url.as_str().contains("page=1"));
}

#[tokio::test]
async fn disabling_profiles_forces_the_live_probe_path() {
    let probe = Arc::new(ProbeEngine::with_chromium(
        "/missing/prewarmed-test-chromium",
    ));
    let builder = BrowserAppBuilder::new(probe, Arc::new(SynthEngine::new())).with_profiles(false);
    let target = Target::new(
        "https://news.ycombinator.com",
        Some("search stories".into()),
    )
    .unwrap();
    assert!(builder.build(target).await.is_err());
}

#[tokio::test]
async fn fresh_mount_bypasses_an_existing_cached_profile() {
    reify_server::warm_profiles().unwrap();
    let server = profile_server(true);
    let target = Target::new(
        "https://news.ycombinator.com",
        Some("search stories".into()),
    )
    .unwrap();
    let initial = server.mount(target.clone()).await.unwrap();
    assert!(!initial.cache_hit);
    let cached = server.mount(target.clone()).await.unwrap();
    assert!(cached.cache_hit);
    assert_eq!(cached.token, initial.token);
    // An invalid executable proves a forced call reached live discovery even
    // though a usable profile generation already exists in the cache.
    assert!(server.mount_fresh(target.clone()).await.is_err());
    let preserved = server.mount(target).await.unwrap();
    assert!(preserved.cache_hit);
    assert_eq!(preserved.token, initial.token);
}

#[tokio::test]
async fn mcp_bootstrap_honors_fresh_and_returns_prepared_wire_tools() {
    use axum::body::{Body, to_bytes};
    use reify_core::mcp::PROTOCOL_VERSION;
    use serde_json::{Value, json};
    use tower::ServiceExt;
    reify_server::warm_profiles().unwrap();
    let server = profile_server(true);
    for fresh in [false, true] {
        let body = json!({
            "jsonrpc":"2.0", "id":1, "method":"tools/call",
            "params":{
                "name":"reify",
                "arguments":{
                    "url":"https://news.ycombinator.com", "goal":"search stories", "fresh":fresh
                },
                "_meta":{
                    "io.modelcontextprotocol/protocolVersion":PROTOCOL_VERSION,
                    "io.modelcontextprotocol/clientCapabilities":{}
                }
            }
        });
        let response = server
            .router()
            .oneshot(
                http::Request::post("/mcp")
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json, text/event-stream")
                    .header("MCP-Protocol-Version", PROTOCOL_VERSION)
                    .header("Mcp-Method", "tools/call")
                    .header("Mcp-Name", "reify")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), http::StatusCode::OK);
        let response: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(response["result"]["isError"], fresh, "{response}");
        if !fresh {
            assert!(response.to_string().contains("search_stories"));
            assert!(!response.to_string().contains("get_item"));
        }
    }
}
