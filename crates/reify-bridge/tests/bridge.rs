use axum::{
    Json, Router,
    body::{Body, to_bytes},
    response::IntoResponse,
    routing::any,
};
use http::{HeaderMap, Request, StatusCode};
use reify_bridge::{
    BridgeServer, ForwardRequest, ForwardResponse, ForwardTransport, ResponseBody,
    store::SessionStore,
};
use reify_core::{ReifyError, domain::AuthContext, mcp::PROTOCOL_VERSION};
use serde_json::{Value, json};
use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc};
use tower::ServiceExt;
const TOKEN: &str = "development-test-token-123456789";
struct InProcessUpstream;
impl ForwardTransport for InProcessUpstream {
    fn forward(
        &self,
        request: ForwardRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ForwardResponse, ReifyError>> + Send + '_>> {
        Box::pin(async move {
            async fn endpoint(
                headers: HeaderMap,
                Json(body): Json<Value>,
            ) -> axum::response::Response {
                let mut response=(StatusCode::IM_A_TEAPOT,Json(json!({"custom":headers["x-custom"].to_str().unwrap(),"authorization":headers["authorization"].to_str().unwrap(),"cookie":headers["cookie"].to_str().unwrap(),"body":body}))).into_response();
                response
                    .headers_mut()
                    .append("set-cookie", http::HeaderValue::from_static("a=1; Path=/"));
                response
                    .headers_mut()
                    .append("set-cookie", http::HeaderValue::from_static("b=2; Path=/"));
                response
            }
            let router = Router::new().route("/api", any(endpoint));
            let mut outgoing = Request::builder()
                .method(request.method.as_str())
                .uri(request.target_url.path())
                .header("content-type", "application/json");
            for (key, value) in request.headers {
                outgoing = outgoing.header(key, value);
            }
            let response = router
                .oneshot(
                    outgoing
                        .body(Body::from(serde_json::to_vec(
                            &request.body.unwrap_or(Value::Null),
                        )?))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status().as_u16();
            let mut headers: HashMap<String, Vec<String>> = HashMap::new();
            for (key, value) in response.headers() {
                headers
                    .entry(key.to_string())
                    .or_default()
                    .push(value.to_str().unwrap().to_owned());
            }
            let body = serde_json::from_slice(
                &to_bytes(response.into_body(), 1024 * 1024).await.unwrap(),
            )?;
            Ok(ForwardResponse {
                status,
                headers,
                body: ResponseBody::Json(body),
            })
        })
    }
}
fn bridge() -> BridgeServer {
    BridgeServer::with_transport(Arc::new(InProcessUpstream), TOKEN.into()).unwrap()
}
#[tokio::test]
async fn test_bridge_health_endpoint() {
    let response = bridge()
        .router()
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["protocol_version"], PROTOCOL_VERSION);
    assert!(!body.to_string().contains(TOKEN));
}
#[tokio::test]
async fn test_bridge_forward_preserves_headers_and_status() {
    let bridge = bridge();
    bridge
        .store()
        .insert(
            "127.0.0.1",
            AuthContext {
                authorization_header: Some("Bearer developer-target".into()),
                cookies: HashMap::from([("session".into(), "explicit".into())]),
                custom_headers: HashMap::from([("x-custom".into(), "stored".into())]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let payload = json!({"target_url":"http://127.0.0.1:54321/api","method":"POST","headers":{"X-Custom":"request-wins"},"body":{"nested":[1,true,null]}});
    let response = bridge
        .router()
        .oneshot(
            Request::post("/forward")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response: ForwardResponse =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(response.status, 418);
    assert_eq!(response.headers["set-cookie"].len(), 2);
    assert_eq!(
        response.body,
        ResponseBody::Json(
            json!({"custom":"request-wins","authorization":"Bearer developer-target","cookie":"session=explicit","body":{"nested":[1,true,null]}})
        )
    );
}
#[tokio::test]
async fn test_bridge_store_roundtrip() {
    let store = SessionStore::new();
    let auth = AuthContext {
        authorization_header: Some("Bearer test".into()),
        ..Default::default()
    };
    store.insert("EXAMPLE.com.", auth.clone()).await.unwrap();
    assert!(store.get("example.com").await.unwrap() == auth);
    assert!(store.get("api.example.com").await.is_none());
    store.insert("bücher.example", auth).await.unwrap();
    assert!(store.get("xn--bcher-kva.example").await.is_some());
    assert!(
        store
            .insert("https://example.com", AuthContext::default())
            .await
            .is_err()
    );
    store.remove("example.com").await.unwrap();
    assert!(store.get("example.com").await.is_none());
    let expired = AuthContext {
        expires_at: Some(chrono::Utc::now() - chrono::Duration::seconds(1)),
        ..Default::default()
    };
    store.insert("expired.test", expired).await.unwrap();
    assert!(store.get("expired.test").await.is_none());
}
#[tokio::test]
async fn bridge_denies_unauthorized_origins_and_transport_headers() {
    let bridge = bridge();
    let payload = json!({"target_url":"https://example.com","method":"GET","headers":{}});
    let req = |token: bool, origin: bool, value: Value| {
        let mut request = Request::post("/forward").header("content-type", "application/json");
        if token {
            request = request.header("authorization", format!("Bearer {TOKEN}"));
        }
        if origin {
            request = request.header("origin", "https://evil.test");
        }
        request.body(Body::from(value.to_string())).unwrap()
    };
    assert_eq!(
        bridge
            .router()
            .oneshot(req(false, false, payload.clone()))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        bridge
            .router()
            .oneshot(req(true, true, payload.clone()))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let mut value = payload;
    value["headers"] = json!({"Host":"evil.test"});
    assert_eq!(
        bridge
            .router()
            .oneshot(req(true, false, value))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert!(bridge.serve("0.0.0.0:9099".parse().unwrap()).await.is_err());
}
#[test]
fn explicit_null_body_survives_typed_roundtrip() {
    let request: ForwardRequest = serde_json::from_value(
        json!({"target_url":"https://example.com","method":"POST","body":null}),
    )
    .unwrap();
    assert_eq!(request.body, Some(Value::Null));
    let value = serde_json::to_value(request).unwrap();
    assert!(value.get("body").is_some());
}
