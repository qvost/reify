//! Authenticated browser-session API and narrowly scoped extension CORS.
use crate::server::{BridgeState, api_error, authorized};
use axum::{
    Json, Router,
    extract::{Query, Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
};
use http::{HeaderMap, HeaderValue, Method, StatusCode};
use reify_core::domain::AuthContext;
use serde::Deserialize;
use serde_json::json;
use std::{collections::HashMap, sync::Arc};
use url::Url;

#[derive(Deserialize)]
#[serde(try_from = "String")]
struct BrowserOrigin(Url);
impl TryFrom<String> for BrowserOrigin {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let url = Url::parse(&value).map_err(|_| "invalid browser origin")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("expected an HTTP(S) origin without a path or credentials");
        }
        Ok(Self(url))
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionRequest {
    origin: BrowserOrigin,
    headers: HashMap<String, String>,
    ttl_seconds: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OriginRequest {
    origin: BrowserOrigin,
}

pub(super) fn routes() -> Router<Arc<BridgeState>> {
    Router::new()
        .route("/session", axum::routing::post(share).delete(revoke))
        .route("/sessions", get(list))
        .route("/session/auth", get(credentials))
}

pub(super) fn extension_origin(headers: &HeaderMap) -> Option<&HeaderValue> {
    if headers.get_all("origin").iter().count() != 1 {
        return None;
    }
    let origin = headers.get("origin")?;
    let id = origin.to_str().ok()?.strip_prefix("chrome-extension://")?;
    (id.len() == 32 && id.bytes().all(|byte| (b'a'..=b'p').contains(&byte))).then_some(origin)
}

pub(super) async fn cors(request: Request, next: Next) -> Response {
    let origin = if request.headers().contains_key("origin") {
        let Some(origin) = extension_origin(request.headers()) else {
            return api_error(StatusCode::FORBIDDEN, "browser origins are not allowed");
        };
        if !matches!(request.uri().path(), "/health" | "/session" | "/sessions") {
            return api_error(
                StatusCode::FORBIDDEN,
                "extension access is limited to session management",
            );
        }
        Some(origin.clone())
    } else {
        None
    };
    let private_network = request
        .headers()
        .get("access-control-request-private-network")
        .is_some_and(|value| value == "true");
    let mut response = if request.method() == Method::OPTIONS && origin.is_some() {
        StatusCode::NO_CONTENT.into_response()
    } else {
        next.run(request).await
    };
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    if let Some(origin) = origin {
        let headers = response.headers_mut();
        headers.insert("access-control-allow-origin", origin);
        headers.insert("vary", HeaderValue::from_static("Origin"));
        headers.insert(
            "access-control-allow-methods",
            HeaderValue::from_static("GET, POST, DELETE, OPTIONS"),
        );
        headers.insert(
            "access-control-allow-headers",
            HeaderValue::from_static("Authorization, Content-Type"),
        );
        if private_network {
            headers.insert(
                "access-control-allow-private-network",
                HeaderValue::from_static("true"),
            );
        }
    }
    response
}

fn session_auth(headers: HashMap<String, String>, ttl: u32) -> Option<AuthContext> {
    if !(1..=3600).contains(&ttl) {
        return None;
    }
    let mut normalized = HashMap::new();
    for (name, value) in headers {
        let name = name.to_ascii_lowercase();
        if ![
            "cookie",
            "user-agent",
            "authorization",
            "x-csrf-token",
            "x-xsrf-token",
        ]
        .contains(&name.as_str())
            || normalized.insert(name, value).is_some()
        {
            return None;
        }
    }
    let cookie = normalized.get("cookie")?;
    if cookie.is_empty() {
        return None;
    }
    for pair in cookie.split(';') {
        let (name, value) = pair.trim().split_once('=')?;
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .unwrap_or(value);
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
            || !value.bytes().all(|byte| {
                byte == 0x21
                    || (0x23..=0x2b).contains(&byte)
                    || (0x2d..=0x3a).contains(&byte)
                    || (0x3c..=0x5b).contains(&byte)
                    || (0x5d..=0x7e).contains(&byte)
            })
        {
            return None;
        }
    }
    Some(AuthContext {
        custom_headers: normalized,
        expires_at: Some(chrono::Utc::now() + chrono::Duration::seconds(ttl.into())),
        ..Default::default()
    })
}

async fn share(
    State(state): State<Arc<BridgeState>>,
    headers: HeaderMap,
    Json(request): Json<SessionRequest>,
) -> Response {
    if !authorized(&state, &headers) {
        return api_error(StatusCode::UNAUTHORIZED, "bridge bearer token required");
    }
    let Some(auth) = session_auth(request.headers, request.ttl_seconds) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid session headers or TTL (1–3600 seconds)",
        );
    };
    let expires_at = auth.expires_at;
    if state
        .store
        .insert_origin(&request.origin.0, auth)
        .await
        .is_err()
    {
        return api_error(StatusCode::BAD_REQUEST, "invalid session credentials");
    }
    Json(json!({"origin":request.origin.0.origin().ascii_serialization(), "expires_at":expires_at}))
        .into_response()
}
async fn list(State(state): State<Arc<BridgeState>>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return api_error(StatusCode::UNAUTHORIZED, "bridge bearer token required");
    }
    Json(json!({"sessions":state.store.origins().await})).into_response()
}
async fn revoke(
    State(state): State<Arc<BridgeState>>,
    headers: HeaderMap,
    Json(request): Json<OriginRequest>,
) -> Response {
    if !authorized(&state, &headers) {
        return api_error(StatusCode::UNAUTHORIZED, "bridge bearer token required");
    }
    state.store.remove_origin(&request.origin.0).await;
    StatusCode::NO_CONTENT.into_response()
}
async fn credentials(
    State(state): State<Arc<BridgeState>>,
    headers: HeaderMap,
    Query(request): Query<OriginRequest>,
) -> Response {
    if !authorized(&state, &headers) {
        return api_error(StatusCode::UNAUTHORIZED, "bridge bearer token required");
    }
    Json(json!({"auth":state.store.auth_for_origin(&request.origin.0).await})).into_response()
}
