//! Authenticated loopback service, bounded forwarding, and injectable transport.
use crate::store::SessionStore;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use reify_core::{ReifyError, mcp::PROTOCOL_VERSION};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap, future::Future, net::SocketAddr, pin::Pin, sync::Arc, time::Duration,
};
use tokio::{net::TcpListener, sync::Semaphore};
use url::Url;
fn error(message: &str) -> ReifyError {
    ReifyError::ExecutionError(message.into())
}
fn present_body<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}
/// A developer-authored HTTP request. Debug is omitted because headers contain secrets.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForwardRequest {
    /// Absolute HTTP(S) destination without URL credentials.
    pub target_url: Url,
    /// Case-sensitive HTTP method.
    pub method: String,
    /// Explicit headers; these override stored credential headers by name.
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Optional JSON document; explicit JSON null remains a body.
    #[serde(
        default,
        deserialize_with = "present_body",
        skip_serializing_if = "Option::is_none"
    )]
    pub body: Option<Value>,
}
/// Lossless response-body representation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ResponseBody {
    /// Parsed JSON, including null.
    Json(Value),
    /// Valid UTF-8 text (empty for a bodyless response).
    Text(String),
    /// Base64 for binary data.
    Base64(String),
}
/// An upstream response carried inside the bridge's JSON HTTP 200 envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForwardResponse {
    /// Actual upstream HTTP status.
    pub status: u16,
    /// Lowercase names with repeated values preserved. Non-ASCII values use the
    /// `=?base64?...?=` sentinel to retain their exact bytes.
    pub headers: HashMap<String, Vec<String>>,
    /// Typed content, without silent binary-to-text conversion.
    pub body: ResponseBody,
}
/// Execution seam for a real HTTP client or in-process developer transport.
pub trait ForwardTransport: Send + Sync + 'static {
    /// Execute an already validated, credential-merged request.
    fn forward(
        &self,
        request: ForwardRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ForwardResponse, ReifyError>> + Send + '_>>;
}
/// Pooled HTTPS-capable transport with redirects disabled and a 4 MiB body bound.
pub struct HttpTransport {
    client: reqwest::Client,
}
impl HttpTransport {
    /// Build a 5s-connect/20s-total client with no inherited proxy configuration.
    pub fn new() -> Result<Self, ReifyError> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|_| error("cannot construct bridge HTTP client"))?;
        Ok(Self { client })
    }
}
impl ForwardTransport for HttpTransport {
    fn forward(
        &self,
        request: ForwardRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ForwardResponse, ReifyError>> + Send + '_>> {
        Box::pin(async move {
            validate(&request)?;
            let mut outgoing = self.client.request(
                Method::from_bytes(request.method.as_bytes())
                    .map_err(|_| error("invalid method"))?,
                request.target_url,
            );
            for (name, value) in request.headers {
                let name = HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| error("invalid header name"))?;
                let mut value =
                    HeaderValue::from_str(&value).map_err(|_| error("invalid header value"))?;
                value.set_sensitive(true);
                outgoing = outgoing.header(name, value);
            }
            if let Some(body) = request.body {
                outgoing = outgoing.json(&body);
            }
            let mut response = outgoing
                .send()
                .await
                .map_err(|_| error("bridge upstream request failed"))?;
            let status = response.status().as_u16();
            let mut headers: HashMap<String, Vec<String>> = HashMap::new();
            for (name, value) in response.headers() {
                let value = value
                    .to_str()
                    .ok()
                    .filter(|text| !(text.starts_with("=?base64?") && text.ends_with("?=")))
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("=?base64?{}?=", STANDARD.encode(value.as_bytes())));
                headers.entry(name.to_string()).or_default().push(value);
            }
            if response
                .content_length()
                .is_some_and(|n| n > 4 * 1024 * 1024)
            {
                return Err(error("bridge upstream response exceeds 4 MiB"));
            }
            let mut bytes = Vec::with_capacity(
                response.content_length().unwrap_or(0).min(4 * 1024 * 1024) as usize,
            );
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| error("bridge response read failed"))?
            {
                if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
                    return Err(error("bridge upstream response exceeds 4 MiB"));
                }
                bytes.extend_from_slice(&chunk);
            }
            let body = match serde_json::from_slice(&bytes) {
                Ok(value) => ResponseBody::Json(value),
                Err(_) => match String::from_utf8(bytes) {
                    Ok(text) => ResponseBody::Text(text),
                    Err(error) => ResponseBody::Base64(STANDARD.encode(error.into_bytes())),
                },
            };
            Ok(ForwardResponse {
                status,
                headers,
                body,
            })
        })
    }
}
fn validate(request: &ForwardRequest) -> Result<(), ReifyError> {
    if !matches!(request.target_url.scheme(), "http" | "https")
        || request.target_url.host_str().is_none()
        || !request.target_url.username().is_empty()
        || request.target_url.password().is_some()
    {
        return Err(error(
            "forward target must be HTTP(S) without URL credentials",
        ));
    }
    let method =
        Method::from_bytes(request.method.as_bytes()).map_err(|_| error("invalid HTTP method"))?;
    if method.as_str().eq_ignore_ascii_case("CONNECT")
        || method.as_str().eq_ignore_ascii_case("TRACE")
    {
        return Err(error("CONNECT and TRACE are not supported"));
    }
    let mut names = std::collections::HashSet::new();
    for (name, value) in &request.headers {
        let name =
            HeaderName::from_bytes(name.as_bytes()).map_err(|_| error("invalid header name"))?;
        if !names.insert(name.clone()) {
            return Err(error("duplicate case-insensitive header"));
        }
        if matches!(
            name.as_str(),
            "host"
                | "content-length"
                | "transfer-encoding"
                | "connection"
                | "upgrade"
                | "te"
                | "trailer"
                | "keep-alive"
                | "proxy-connection"
                | "proxy-authorization"
        ) {
            return Err(error("forward header is managed by the transport"));
        }
        HeaderValue::from_str(value).map_err(|_| error("invalid header value"))?;
    }
    Ok(())
}
pub(super) struct BridgeState {
    pub(super) store: SessionStore,
    transport: Arc<dyn ForwardTransport>,
    token: String,
    slots: Semaphore,
}
/// One loopback bridge. The token authorizes forwarding, never read via /health.
#[derive(Clone)]
pub struct BridgeServer {
    state: Arc<BridgeState>,
}
impl BridgeServer {
    /// Create an HTTP bridge with an unguessable per-process bearer token.
    pub fn new() -> Result<Self, ReifyError> {
        Self::with_transport(
            Arc::new(HttpTransport::new()?),
            uuid::Uuid::new_v4().to_string(),
        )
    }
    /// Supply a transport and local authorization token. Intended for embedding
    /// and in-process HTTP tests; production can use HttpTransport with a fixed token.
    pub fn with_transport(
        transport: Arc<dyn ForwardTransport>,
        token: String,
    ) -> Result<Self, ReifyError> {
        if token.len() < 16
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
        {
            return Err(error(
                "bridge token requires at least 16 safe ASCII characters",
            ));
        }
        Ok(Self {
            state: Arc::new(BridgeState {
                store: SessionStore::new(),
                transport,
                token,
                slots: Semaphore::new(32),
            }),
        })
    }
    /// Credential-store handle for explicit developer configuration.
    pub fn store(&self) -> SessionStore {
        self.state.store.clone()
    }
    /// Secret for configuring an authorized local client. Never printed by health.
    pub fn token(&self) -> &str {
        &self.state.token
    }
    /// Router for in-process testing. Embedders must preserve loopback binding.
    pub fn router(&self) -> Router {
        Router::new()
            .route("/health", get(health))
            .route("/forward", post(forward))
            .merge(crate::session::routes())
            .layer(DefaultBodyLimit::max(1024 * 1024))
            .layer(axum::middleware::from_fn(crate::session::cors))
            .with_state(self.state.clone())
    }
    /// Serve a configurable loopback address until canceled or the listener fails.
    pub async fn serve(&self, address: SocketAddr) -> Result<(), ReifyError> {
        self.serve_with_shutdown(address, std::future::pending())
            .await
    }
    /// Serve with graceful shutdown. Non-loopback addresses are rejected.
    pub async fn serve_with_shutdown<F>(
        &self,
        address: SocketAddr,
        shutdown: F,
    ) -> Result<(), ReifyError>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        if !address.ip().is_loopback() {
            return Err(error("bridge must bind a loopback address"));
        }
        let listener = TcpListener::bind(address)
            .await
            .map_err(|_| error("cannot bind bridge listener"))?;
        self.serve_listener(listener, shutdown).await
    }
    /// Serve a pre-bound loopback listener, allowing race-free ephemeral-port tests.
    pub async fn serve_listener<F>(
        &self,
        listener: TcpListener,
        shutdown: F,
    ) -> Result<(), ReifyError>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let address = listener
            .local_addr()
            .map_err(|_| error("cannot inspect bridge listener"))?;
        if !address.ip().is_loopback() {
            return Err(error("bridge listener must be loopback"));
        }
        axum::serve(listener, self.router())
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(|_| error("bridge HTTP server failed"))
    }
}
pub(super) fn api_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error":message}))).into_response()
}
async fn health(headers: HeaderMap) -> Response {
    if headers.contains_key("origin") && crate::session::extension_origin(&headers).is_none() {
        return api_error(StatusCode::FORBIDDEN, "browser origins are not allowed");
    }
    Json(json!({"status":"ok","protocol_version":PROTOCOL_VERSION})).into_response()
}
async fn forward(
    State(state): State<Arc<BridgeState>>,
    headers: HeaderMap,
    Json(mut request): Json<ForwardRequest>,
) -> Response {
    if headers.contains_key("origin") {
        return api_error(StatusCode::FORBIDDEN, "browser origins are not allowed");
    }
    if !authorized(&state, &headers) {
        return api_error(StatusCode::UNAUTHORIZED, "bridge bearer token required");
    }
    if validate(&request).is_err() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid forward URL, method, or headers",
        );
    }
    let Ok(_permit) = state.slots.try_acquire() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "bridge concurrency limit reached",
        );
    };
    if let Some(stored) = state.store.headers_for(&request.target_url).await {
        let mut merged = HashMap::with_capacity(stored.len() + request.headers.len());
        for (mut name, value) in request.headers {
            name.make_ascii_lowercase();
            merged.insert(name, value);
        }
        for (name, value) in stored.iter() {
            if !merged.contains_key(name) {
                merged.insert(name.clone(), value.clone());
            }
        }
        request.headers = merged;
    }
    match tokio::time::timeout(Duration::from_secs(25), state.transport.forward(request)).await {
        Ok(Ok(response)) => Json(response).into_response(),
        _ => api_error(
            StatusCode::BAD_GATEWAY,
            "bridge upstream forwarding failed or timed out",
        ),
    }
}
pub(super) fn authorized(state: &BridgeState, headers: &HeaderMap) -> bool {
    headers.get_all("authorization").iter().count() == 1
        && !headers
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            .and_then(|value| value.split_once(' '))
            .is_none_or(|(scheme, token)| {
                !scheme.eq_ignore_ascii_case("Bearer") || token != state.token
            })
}
