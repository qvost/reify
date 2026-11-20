use axum::body::{Body, to_bytes};
use http::{Request, StatusCode};
use reify_bridge::{BridgeServer, ForwardRequest, ForwardResponse, ForwardTransport, ResponseBody};
use reify_core::{ReifyError, domain::AuthContext};
use serde_json::{Value, json};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tower::ServiceExt;
use url::Url;

const TOKEN: &str = "extension-test-token-123456789";
const ORIGIN: &str = "chrome-extension://abcdefghijklmnopabcdefghijklmnop";
const PAYLOAD: &str = include_str!("../../../extension/tests/fixtures/session.json");

#[derive(Default)]
struct Upstream(Mutex<Vec<ForwardRequest>>);
impl ForwardTransport for Upstream {
    fn forward(
        &self,
        request: ForwardRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ForwardResponse, ReifyError>> + Send + '_>> {
        self.0.lock().unwrap().push(request);
        Box::pin(async {
            Ok(ForwardResponse {
                status: 200,
                headers: Default::default(),
                body: ResponseBody::Json(json!({"private_repos":["fixture"]})),
            })
        })
    }
}

fn fixture() -> (BridgeServer, Arc<Upstream>) {
    let upstream = Arc::new(Upstream::default());
    (
        BridgeServer::with_transport(upstream.clone(), TOKEN.into()).unwrap(),
        upstream,
    )
}
async fn call(
    bridge: &BridgeServer,
    method: &str,
    path: &str,
    body: Option<Value>,
    origin: Option<&str>,
    authenticated: bool,
) -> (StatusCode, http::HeaderMap, Value) {
    let mut request = Request::builder().method(method).uri(path);
    if authenticated {
        request = request.header("authorization", format!("Bearer {TOKEN}"));
    }
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    let body = if let Some(body) = body {
        request = request.header("content-type", "application/json");
        Body::from(body.to_string())
    } else {
        Body::empty()
    };
    let response = bridge
        .router()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, value)
}

#[tokio::test]
async fn extension_payload_is_stored_and_applied_to_subsequent_tool_requests() {
    let (bridge, upstream) = fixture();
    let payload: Value = serde_json::from_str(PAYLOAD).unwrap();
    let (status, headers, response) = call(
        &bridge,
        "POST",
        "/session",
        Some(payload.clone()),
        Some(ORIGIN),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["access-control-allow-origin"], ORIGIN);
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(response["origin"], "https://github.com");
    assert!(!response.to_string().contains("fixture-session"));
    let auth = bridge
        .store()
        .auth_for_origin(&Url::parse("https://github.com").unwrap())
        .await
        .unwrap();
    assert_eq!(auth.custom_headers["cookie"], payload["headers"]["cookie"]);
    let (status, _, _) = call(
        &bridge,
        "POST",
        "/forward",
        Some(json!({
            "target_url":"https://github.com/api/private/repos", "method":"GET", "headers":{}
        })),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let records = upstream.0.lock().unwrap();
    assert_eq!(records[0].headers["cookie"], payload["headers"]["cookie"]);
    assert_eq!(
        records[0].headers["user-agent"],
        payload["headers"]["user-agent"]
    );
    assert!(!records[0].headers.contains_key("authorization"));
}

#[tokio::test]
async fn origin_scope_explicit_headers_revocation_and_expiry_are_enforced() {
    let (bridge, upstream) = fixture();
    call(
        &bridge,
        "POST",
        "/session",
        Some(serde_json::from_str(PAYLOAD).unwrap()),
        Some(ORIGIN),
        true,
    )
    .await;
    for url in [
        "http://github.com/api",
        "https://api.github.com/api",
        "https://github.com:8443/api",
        "https://github.com.evil.test/api",
    ] {
        call(
            &bridge,
            "POST",
            "/forward",
            Some(json!({"target_url":url,"method":"GET"})),
            None,
            true,
        )
        .await;
    }
    assert!(
        upstream
            .0
            .lock()
            .unwrap()
            .iter()
            .all(|request| !request.headers.contains_key("cookie"))
    );
    call(&bridge, "POST", "/forward", Some(json!({"target_url":"https://github.com/api","method":"GET","headers":{"Cookie":"override=explicit"}})), None, true).await;
    assert_eq!(
        upstream.0.lock().unwrap().last().unwrap().headers["cookie"],
        "override=explicit"
    );
    let (status, _, listed) = call(&bridge, "GET", "/sessions", None, Some(ORIGIN), true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 1);
    assert!(!listed.to_string().contains("fixture-session"));
    assert_eq!(
        call(
            &bridge,
            "DELETE",
            "/session",
            Some(json!({"origin":"https://github.com"})),
            Some(ORIGIN),
            true
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert!(
        bridge
            .store()
            .auth_for_origin(&Url::parse("https://github.com").unwrap())
            .await
            .is_none()
    );
    assert_eq!(
        call(
            &bridge,
            "DELETE",
            "/session",
            Some(json!({"origin":"https://github.com"})),
            Some(ORIGIN),
            true
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    bridge
        .store()
        .insert_origin(
            &Url::parse("https://github.com").unwrap(),
            AuthContext {
                custom_headers: std::collections::HashMap::from([(
                    "cookie".into(),
                    "expired=1".into(),
                )]),
                expires_at: Some(chrono::Utc::now() - chrono::Duration::seconds(1)),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    call(
        &bridge,
        "POST",
        "/forward",
        Some(json!({"target_url":"https://github.com/api","method":"GET"})),
        None,
        true,
    )
    .await;
    assert!(
        !upstream
            .0
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .headers
            .contains_key("cookie")
    );
    assert!(bridge.store().origins().await.is_empty());
}

#[tokio::test]
async fn extension_cors_does_not_open_forwarding_or_credential_reads_to_pages() {
    let (bridge, _) = fixture();
    for path in ["/health", "/sessions", "/session"] {
        assert_eq!(
            call(&bridge, "GET", path, None, Some("https://evil.test"), true)
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            call(&bridge, "GET", path, None, Some("null"), true).await.0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        call(&bridge, "GET", "/health", None, Some(ORIGIN), false)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&bridge, "GET", "/sessions", None, Some(ORIGIN), false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &bridge,
            "POST",
            "/session",
            Some(serde_json::from_str(PAYLOAD).unwrap()),
            Some(ORIGIN),
            false
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &bridge,
            "DELETE",
            "/session",
            Some(json!({"origin":"https://github.com"})),
            Some(ORIGIN),
            false
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &bridge,
            "POST",
            "/forward",
            Some(json!({"target_url":"https://github.com", "method":"GET"})),
            Some(ORIGIN),
            true
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &bridge,
            "GET",
            "/session/auth?origin=https%3A%2F%2Fgithub.com",
            None,
            Some(ORIGIN),
            true
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let response = bridge
        .router()
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/session")
                .header("origin", ORIGIN)
                .header("access-control-request-method", "POST")
                .header(
                    "access-control-request-headers",
                    "authorization,content-type",
                )
                .header("access-control-request-private-network", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(response.headers()["access-control-allow-origin"], ORIGIN);
    assert_eq!(
        response.headers()["access-control-allow-private-network"],
        "true"
    );
}

#[tokio::test]
async fn malformed_origins_ttls_and_header_injection_are_rejected_without_storing() {
    let (bridge, _) = fixture();
    let base: Value = serde_json::from_str(PAYLOAD).unwrap();
    for (key, value) in [
        ("origin", json!("https://github.com/private")),
        ("origin", json!("https://user:secret@github.com")),
        ("origin", json!("file:///tmp")),
        ("ttl_seconds", json!(0)),
        ("ttl_seconds", json!(3601)),
        ("headers", json!({"cookie":"a=1\r\nInjected: yes"})),
        ("headers", json!({"cookie":"a=1", "host":"evil"})),
        ("headers", json!({"cookie":"a=1","Cookie":"b=2"})),
        ("headers", json!({"user-agent":"test"})),
    ] {
        let mut payload = base.clone();
        payload[key] = value;
        assert!(
            call(
                &bridge,
                "POST",
                "/session",
                Some(payload),
                Some(ORIGIN),
                true
            )
            .await
            .0
            .is_client_error()
        );
    }
    assert!(bridge.store().origins().await.is_empty());
}

#[tokio::test]
async fn native_credential_lookup_is_authenticated_and_preserves_cookie_order() {
    let (bridge, _) = fixture();
    let mut payload: Value = serde_json::from_str(PAYLOAD).unwrap();
    payload["headers"]["cookie"] = json!("session=scoped; session=root; quoted=\"value\"");
    call(
        &bridge,
        "POST",
        "/session",
        Some(payload),
        Some(ORIGIN),
        true,
    )
    .await;
    let path = "/session/auth?origin=https%3A%2F%2Fgithub.com";
    assert_eq!(
        call(&bridge, "GET", path, None, None, false).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, headers, value) = call(&bridge, "GET", path, None, None, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(
        value["auth"]["custom_headers"]["cookie"],
        "session=scoped; session=root; quoted=\"value\""
    );
}
