//! Per-probe orchestration with bounded capture and concurrent response body reads.
use crate::{
    action,
    auth::{AuthExtractor, BrowserCookie},
    cdp::{CdpClient, CdpEvent},
    failure, filter,
    process::ChromiumProcess,
    ssr::SsrProbe,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::Utc;
use futures_util::{StreamExt, stream::FuturesUnordered};
use reify_core::{
    ReifyError,
    domain::{AuthContext, CapturedFrame, ResourceType, Target},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant},
};
use url::Url;
use uuid::Uuid;

const MAX_FRAMES: usize = 512;
const MAX_BODY: usize = 1024 * 1024;
/// Result of one isolated navigation and network observation window.
pub struct ProbeOutput {
    /// Completed API exchanges or derived SSR snapshots, in source order.
    pub frames: Vec<CapturedFrame>,
    /// Scoped credentials, never logged.
    pub auth: AuthContext,
    /// URL obtained from the page after navigation/redirects.
    pub final_url: Url,
    /// Document title if nonempty.
    pub page_title: Option<String>,
    /// Elapsed wall time, including startup and extraction.
    pub duration_ms: u64,
}
/// Cloneable configuration with a cookie-free HTTP pool. Browser fallback owns
/// separate process/profile resources for each call.
#[derive(Default, Clone)]
pub struct ProbeEngine {
    binary: Option<PathBuf>,
    ssr: SsrProbe,
    initial_auth: Option<AuthContext>,
}
impl ProbeEngine {
    /// Use automatic Chromium discovery.
    pub fn new() -> Self {
        Self::default()
    }
    /// Use an explicit executable, useful for deployment and integration tests.
    pub fn with_chromium(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: Some(binary.into()),
            ssr: SsrProbe::default(),
            initial_auth: None,
        }
    }
    /// Interactive goals use Chromium directly; other goals try SSR JSON first.
    /// Observe Chromium until page load
    /// plus 750ms network quiet. One deadline covers both paths. SSR gets at most
    /// 500ms or a quarter of the budget, leaving time for browser fallback.
    pub async fn probe(
        &self,
        target: &Target,
        timeout: Duration,
    ) -> Result<ProbeOutput, ReifyError> {
        if let Some(bridge) = reify_bridge::BridgeClient::from_env()? {
            let started = Instant::now();
            let initial_auth = tokio::time::timeout(timeout, bridge.auth_for(&target.url))
                .await
                .map_err(|_| failure("probe", "deadline exceeded"))??;
            let configured = Self {
                initial_auth,
                ..self.clone()
            };
            let remaining = timeout
                .checked_sub(started.elapsed())
                .ok_or_else(|| failure("probe", "deadline exceeded"))?;
            let mut output = configured
                .probe_with_ssr(target, remaining, configured.ssr.probe(target))
                .await?;
            output.duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            return Ok(output);
        }
        self.probe_with_ssr(target, timeout, self.ssr.probe(target))
            .await
    }
    async fn probe_with_ssr(
        &self,
        target: &Target,
        timeout: Duration,
        fast_path: impl std::future::Future<Output = Option<ProbeOutput>>,
    ) -> Result<ProbeOutput, ReifyError> {
        if timeout.is_zero() {
            return Err(failure("probe", "deadline exceeded"));
        }
        Target::new(target.url.as_str(), target.goal.clone())?;
        let started = Instant::now();
        tokio::time::timeout(timeout, async {
            let budget = (timeout / 4).min(Duration::from_millis(500));
            let mut output =
                if self.initial_auth.is_some() || action::is_interactive(target.goal.as_deref()) {
                    self.run(target).await?
                } else {
                    match tokio::time::timeout(budget, fast_path).await {
                        Ok(Some(output)) => output,
                        _ => self.run(target).await?,
                    }
                };
            output.duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            Ok(output)
        })
        .await
        .map_err(|_| failure("probe", "deadline exceeded"))?
    }
    async fn run(&self, target: &Target) -> Result<ProbeOutput, ReifyError> {
        let started = Instant::now();
        let process = match &self.binary {
            Some(binary) => ChromiumProcess::launch(binary).await?,
            None => ChromiumProcess::discover_and_launch().await?,
        };
        let mut cdp = CdpClient::discover_for_probe(
            process
                .endpoint()
                .port()
                .ok_or_else(|| failure("DevTools", "missing port"))?,
        )
        .await?;
        let mut events = cdp
            .take_events()
            .ok_or_else(|| failure("CDP events", "already consumed"))?;
        let created = cdp
            .command("Target.createTarget", json!({"url":"about:blank"}), None)
            .await?;
        let target_id = created["targetId"]
            .as_str()
            .ok_or_else(|| failure("create page", "missing targetId"))?;
        let attached = cdp
            .command(
                "Target.attachToTarget",
                json!({"targetId":target_id,"flatten":true}),
                None,
            )
            .await?;
        let session = attached["sessionId"]
            .as_str()
            .ok_or_else(|| failure("attach page", "missing sessionId"))?;
        tokio::try_join!(
            cdp.command("Page.enable",json!({}),Some(session)),
            cdp.command("Runtime.enable",json!({}),Some(session)),
            cdp.command("Network.enable",json!({"maxPostDataSize":MAX_BODY,"maxResourceBufferSize":MAX_BODY,"maxTotalBufferSize":16*MAX_BODY}),Some(session))
        )?;
        if let Some(auth) = &self.initial_auth {
            let headers = auth.to_headers()?;
            if let Some(cookie) = headers.get("cookie").and_then(|value| value.to_str().ok()) {
                let mut seen = std::collections::HashSet::new();
                let cookies: Vec<_> = cookie
                    .split(';')
                    .filter_map(|pair| {
                        let (name, value) = pair.trim().split_once('=')?;
                        if !seen.insert(name) {
                            return None;
                        }
                        let mut cookie = json!({"name":name,"value":value,
                        "url":format!("{}/", target.url.origin().ascii_serialization()),
                        "secure":target.url.scheme() == "https"});
                        if let Some(expiry) = auth.expires_at {
                            cookie["expires"] = json!(expiry.timestamp());
                        }
                        Some(cookie)
                    })
                    .collect();
                cdp.command(
                    "Network.setCookies",
                    json!({"cookies":cookies}),
                    Some(session),
                )
                .await?;
            }
            if let Some(agent) = headers
                .get("user-agent")
                .and_then(|value| value.to_str().ok())
            {
                cdp.command(
                    "Network.setUserAgentOverride",
                    json!({"userAgent":agent}),
                    Some(session),
                )
                .await?;
            }
        }
        let navigation = cdp
            .command(
                "Page.navigate",
                json!({"url":target.url.as_str()}),
                Some(session),
            )
            .await?;
        if navigation.get("errorText").is_some() {
            return Err(failure(
                "page navigation",
                "Chromium reported a navigation error",
            ));
        }
        let mut capture = Capture::default();
        capture
            .observe(&cdp, session, &target.url, &mut events, None)
            .await?;
        let needs_action = capture.completed.is_empty()
            || action::is_interactive(target.goal.as_deref())
            || target
                .goal
                .as_deref()
                .is_some_and(|goal| !capture.matches_goal(goal));
        if needs_action
            && let Some(action) = action::excite(&cdp, session, target.goal.as_deref()).await?
        {
            capture.discovery_scope = action.destination;
            let previous = capture.completed.len();
            capture
                .observe(
                    &cdp,
                    session,
                    &target.url,
                    &mut events,
                    Some(tokio::time::Instant::now() + Duration::from_millis(750)),
                )
                .await?;
            if capture.completed.len() == previous {
                tracing::debug!("search excitation produced no additional API frames");
            }
        }
        let page = cdp.command("Runtime.evaluate", json!({"expression":"({url:location.href,title:document.title})","returnByValue":true}), Some(session)).await?;
        if page.get("exceptionDetails").is_some() {
            return Err(failure("page inspection", "script evaluation failed"));
        }
        let value = &page["result"]["value"];
        let final_url = value["url"]
            .as_str()
            .and_then(|u| Url::parse(u).ok())
            .ok_or_else(|| failure("page inspection", "missing final URL"))?;
        let page_title = value["title"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let mut cookie_urls = vec![target.url.as_str()];
        cookie_urls.extend(
            capture
                .completed
                .iter()
                .map(|(_, frame)| frame.url.as_str()),
        );
        let mut cookies = cdp
            .command(
                "Network.getCookies",
                json!({"urls":cookie_urls}),
                Some(session),
            )
            .await?;
        let cookies: Vec<BrowserCookie> = serde_json::from_value(
            cookies
                .get_mut("cookies")
                .map(Value::take)
                .ok_or_else(|| failure("cookie extraction", "missing cookies"))?,
        )?;
        capture.auth.cookies(&target.url, cookies);
        capture.completed.sort_by_key(|(sequence, _)| *sequence);
        Ok(ProbeOutput {
            frames: capture.completed.into_iter().map(|(_, f)| f).collect(),
            auth: capture.auth.finish(),
            final_url,
            page_title,
            duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        })
    }
}
fn headers(value: &mut Value) -> HashMap<String, String> {
    let Value::Object(map) = value.take() else {
        return HashMap::new();
    };
    let mut result = HashMap::with_capacity(map.len());
    for (mut name, value) in map {
        if let Value::String(value) = value {
            name.make_ascii_lowercase();
            result.insert(name, value);
        }
    }
    result
}
fn decode_body(response: &Value) -> Option<Value> {
    let body = response["body"].as_str()?;
    if body.len() > MAX_BODY * 4 / 3 + 4 {
        return None;
    }
    if response["base64Encoded"].as_bool().unwrap_or(false) {
        let bytes = STANDARD.decode(body).ok()?;
        if bytes.len() > MAX_BODY {
            return None;
        }
        serde_json::from_slice(&bytes).ok()
    } else {
        if body.len() > MAX_BODY {
            return None;
        }
        serde_json::from_str(body).ok()
    }
}
#[derive(Default)]
struct Capture {
    pending: HashMap<String, (u64, CapturedFrame)>,
    completed: Vec<(u64, CapturedFrame)>,
    auth: AuthExtractor,
    next: u64,
    discovery_scope: Option<Url>,
}
impl Capture {
    fn matches_goal(&self, goal: &str) -> bool {
        fn fields_match(value: &Value, keywords: &[&str], depth: usize) -> bool {
            if depth >= 64 {
                return false;
            }
            match value {
                Value::Object(fields) => fields.iter().any(|(name, value)| {
                    let name = name.to_lowercase();
                    keywords.iter().any(|keyword| name.contains(keyword))
                        || fields_match(value, keywords, depth + 1)
                }),
                Value::Array(values) => values
                    .iter()
                    .any(|value| fields_match(value, keywords, depth + 1)),
                _ => false,
            }
        }
        let keywords: Vec<_> = goal
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| {
                word.len() > 2 && !["the", "for", "and", "with", "please"].contains(word)
            })
            .collect();
        keywords.is_empty()
            || self.completed.iter().any(|(_, frame)| {
                let endpoint = frame.url.as_str().to_lowercase();
                keywords.iter().any(|keyword| endpoint.contains(keyword))
                    || [&frame.request_body, &frame.response_body]
                        .iter()
                        .any(|body| {
                            body.as_ref()
                                .is_some_and(|body| fields_match(body, &keywords, 0))
                        })
            })
    }
    async fn observe(
        &mut self,
        cdp: &CdpClient,
        session: &str,
        target: &Url,
        events: &mut tokio::sync::mpsc::Receiver<CdpEvent>,
        deadline: Option<tokio::time::Instant>,
    ) -> Result<(), ReifyError> {
        let mut bodies = FuturesUnordered::new();
        let mut loaded = deadline.is_some();
        let mut last_activity = tokio::time::Instant::now();
        loop {
            let quiet = last_activity + Duration::from_millis(750);
            tokio::select! {
                biased;
                _ = async {
                    match deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline).await,
                        None => std::future::pending().await,
                    }
                } => break,
                Some((sequence, frame, response)) = bodies.next(), if !bodies.is_empty() => {
                    let mut frame: CapturedFrame = frame;
                    if let Ok(response) = response { frame.response_body = decode_body(&response); }
                    // Bodies can be unavailable after eviction or for 204 responses;
                    // retain the completed request and response metadata in that case.
                    self.completed.push((sequence, frame));
                    last_activity = tokio::time::Instant::now();
                }
                event = events.recv() => {
                    let mut event = event.ok_or_else(|| failure("CDP events", "socket closed or event queue overflowed"))?;
                    if event.session_id.as_deref() != Some(session) { continue; }
                    if event.method == "Page.loadEventFired" { loaded = true; last_activity = tokio::time::Instant::now(); }
                    let was_pending=!self.pending.is_empty();
                    if let Some((sequence, frame)) = self.event(&mut event, target) {
                        if bodies.len() >= 16 { return Err(failure("capture", "response concurrency limit exceeded")); }
                        bodies.push(async move {
                            let response = cdp.command("Network.getResponseBody", json!({"requestId":sequence.1}), Some(session)).await;
                            (sequence.0, frame, response)
                        });
                    }
                    if was_pending || !self.pending.is_empty() { last_activity = tokio::time::Instant::now(); }
                }
                _ = tokio::time::sleep_until(quiet), if deadline.is_none() && loaded && self.pending.is_empty() && bodies.is_empty() => break,
            }
        }
        Ok(())
    }
    fn event(
        &mut self,
        event: &mut CdpEvent,
        target: &Url,
    ) -> Option<((u64, String), CapturedFrame)> {
        let p = &mut event.params;
        let id = match p.get_mut("requestId")?.take() {
            Value::String(id) => id,
            _ => return None,
        };
        match event.method.as_str() {
            "Network.requestWillBeSent" => {
                // Redirects reuse request IDs; never associate an old body or auth
                // header with a new URL. The final hop becomes the captured exchange.
                self.pending.remove(&id);
                if self.next >= MAX_FRAMES as u64 {
                    return None;
                }
                if filter::is_static_type(p["type"].as_str().unwrap_or("")) {
                    return None;
                }
                let resource_type = match p["type"].as_str() {
                    Some("XHR") => ResourceType::Xhr,
                    Some("Fetch") => ResourceType::Fetch,
                    _ => ResourceType::Other,
                };
                let r = &mut p["request"];
                let url = Url::parse(r["url"].as_str()?).ok()?;
                if !(filter::is_first_party(target, &url)
                    || self
                        .discovery_scope
                        .as_ref()
                        .is_some_and(|scope| filter::is_first_party(scope, &url)))
                    || filter::is_noise(&url, "")
                {
                    return None;
                }
                let request_headers = headers(&mut r["headers"]);
                self.auth.observe_headers(&request_headers, true);
                let post = match r.get_mut("postData").map(Value::take) {
                    Some(Value::String(post)) if post.len() <= MAX_BODY => Some(post),
                    _ => None,
                };
                let request_body = post
                    .as_deref()
                    .and_then(|post| serde_json::from_str(post).ok())
                    .or_else(|| {
                        let graphql = request_headers
                            .get("content-type")
                            .is_some_and(|mime| mime.starts_with("application/graphql"));
                        if graphql {
                            post.map(|post| {
                                let mut object = serde_json::Map::new();
                                object.insert("query".into(), Value::String(post));
                                Value::Object(object)
                            })
                        } else {
                            None
                        }
                    });
                let method = match r.get_mut("method")?.take() {
                    Value::String(method) => method,
                    _ => return None,
                };
                let frame = CapturedFrame {
                    id: Uuid::new_v4(),
                    url,
                    method,
                    request_headers,
                    request_body,
                    status_code: 0,
                    response_headers: HashMap::new(),
                    response_body: None,
                    timestamp: Utc::now(),
                    resource_type,
                };
                let sequence = self.next;
                self.next += 1;
                self.pending.insert(id, (sequence, frame));
            }
            "Network.responseReceived" => {
                let (_, frame) = self.pending.get_mut(&id)?;
                let response = &mut p["response"];
                frame.response_headers = headers(&mut response["headers"]);
                let status = response["status"].as_f64()?;
                if !(100.0..=599.0).contains(&status) {
                    self.pending.remove(&id);
                    return None;
                }
                frame.status_code = status as u16;
                self.auth.observe_headers(&frame.response_headers, false);
                if !filter::is_api(frame, response["mimeType"].as_str().unwrap_or("")) {
                    self.pending.remove(&id);
                }
            }
            "Network.loadingFailed" => {
                self.pending.remove(&id);
            }
            "Network.loadingFinished" => {
                let (sequence, frame) = self.pending.remove(&id)?;
                if frame.status_code == 0 {
                    return None;
                }
                if p["encodedDataLength"]
                    .as_u64()
                    .is_some_and(|n| n > MAX_BODY as u64)
                {
                    self.completed.push((sequence, frame));
                    return None;
                }
                return Some(((sequence, id), frame));
            }
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(method: &str, params: Value) -> CdpEvent {
        CdpEvent {
            method: method.into(),
            params,
            session_id: Some("page".into()),
        }
    }
    #[test]
    fn capture_joins_events_and_drops_redirected_or_failed_requests() {
        let target = Url::parse("https://example.com").unwrap();
        let mut capture = Capture::default();
        let request = |url: &str| {
            event(
                "Network.requestWillBeSent",
                json!({"requestId":"1","type":"Fetch","request":{"url":url,"method":"POST","headers":{"Authorization":"Bearer test"},"postData":"{\"query\":\"{x}\"}"}}),
            )
        };
        capture.event(&mut request("https://example.com/graphql"), &target);
        assert_eq!(capture.pending.len(), 1);
        capture.event(&mut event("Network.responseReceived",json!({"requestId":"1","response":{"status":200,"headers":{"Content-Type":"application/json"},"mimeType":"application/json"}})),&target);
        let (_, frame) = capture
            .event(
                &mut event(
                    "Network.loadingFinished",
                    json!({"requestId":"1","encodedDataLength":20}),
                ),
                &target,
            )
            .unwrap();
        assert_eq!(frame.status_code, 200);
        assert!(frame.is_graphql());
        assert_eq!(
            capture.auth.finish().authorization_header.as_deref(),
            Some("Bearer test")
        );
        let mut capture = Capture::default();
        capture.event(&mut request("https://example.com/graphql"), &target);
        capture.event(&mut request("https://evil.org/graphql"), &target);
        assert!(capture.pending.is_empty());
        capture.event(&mut request("https://example.com/api"), &target);
        capture.event(
            &mut event("Network.loadingFailed", json!({"requestId":"1"})),
            &target,
        );
        assert!(capture.pending.is_empty());
    }
    #[test]
    fn body_decodes_base64_and_enforces_limit() {
        assert_eq!(
            decode_body(&json!({"body":"eyJ4IjoxfQ==","base64Encoded":true})),
            Some(json!({"x":1}))
        );
        assert_eq!(
            decode_body(&json!({"body":"null","base64Encoded":false})),
            Some(Value::Null)
        );
        assert!(decode_body(&json!({"body":"%%%","base64Encoded":true})).is_none());
        assert!(decode_body(&json!({"body":"not json"})).is_none());
        assert!(decode_body(&json!({"body":" ".repeat(MAX_BODY+1)})).is_none());
    }

    #[test]
    fn goal_matching_checks_endpoint_and_json_field_names() {
        let target = Url::parse("https://example.com").unwrap();
        let mut capture = Capture::default();
        capture.event(
            &mut event(
                "Network.requestWillBeSent",
                json!({
                    "requestId":"r", "type":"Fetch", "request":{
                        "url":"https://example.com/api/items", "method":"GET", "headers":{}
                    }
                }),
            ),
            &target,
        );
        let (sequence, mut frame) = capture.pending.remove("r").unwrap();
        frame.response_body = Some(json!({"data":[{"stories":[], "title":"catalog"}]}));
        capture.completed.push((sequence, frame));
        assert!(capture.matches_goal("items"));
        assert!(capture.matches_goal("stories"));
        assert!(!capture.matches_goal("catalog"));
    }

    #[test]
    fn capture_accepts_only_original_or_declared_search_hosts() {
        let target = Url::parse("https://example.com").unwrap();
        let mut capture = Capture::default();
        let request = |url: &str| {
            event(
                "Network.requestWillBeSent",
                json!({
                    "requestId":"r", "type":"Fetch", "request":{
                        "url":url, "method":"GET", "headers":{}
                    }
                }),
            )
        };
        capture.event(
            &mut request("https://search.example.org/api/search"),
            &target,
        );
        assert!(capture.pending.is_empty());
        capture.discovery_scope = Some(Url::parse("https://search.example.org").unwrap());
        capture.event(
            &mut request("https://search.example.org/api/search"),
            &target,
        );
        assert_eq!(capture.pending.len(), 1);
        capture.event(
            &mut request("https://search.example.org.evil.com/api/search"),
            &target,
        );
        assert!(capture.pending.is_empty());
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    #[test]
    fn intercepted_headers_methods_and_raw_graphql_move_into_the_frame() {
        let target = Url::parse("https://example.com").unwrap();
        let mut event = CdpEvent {
            method: "Network.requestWillBeSent".into(),
            session_id: None,
            params: json!({"requestId":"r","type":"Fetch","request":{"url":"https://example.com/graphql","method":"POST","headers":{"Authorization":"Bearer fixture","Content-Type":"application/graphql"},"postData":"query { users { id } }"}}),
        };
        let auth = event.params["request"]["headers"]["Authorization"]
            .as_str()
            .unwrap()
            .as_ptr();
        let body = event.params["request"]["postData"]
            .as_str()
            .unwrap()
            .as_ptr();
        let method = event.params["request"]["method"].as_str().unwrap().as_ptr();
        let mut capture = Capture::default();
        capture.event(&mut event, &target);
        let frame = &capture.pending["r"].1;
        assert_eq!(frame.request_headers["authorization"], "Bearer fixture");
        assert!(frame.is_graphql());
        assert_eq!(frame.request_headers["authorization"].as_ptr(), auth);
        assert_eq!(frame.method.as_ptr(), method);
        assert_eq!(
            frame.request_body.as_ref().unwrap()["query"]
                .as_str()
                .unwrap()
                .as_ptr(),
            body
        );
    }
}

#[cfg(test)]
mod ssr_tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[tokio::test]
    async fn ssr_hit_returns_state_without_ever_starting_chromium() {
        let engine = ProbeEngine::with_chromium("/missing/reify-test-chromium");
        let target = Target::new("https://example.com/items", None).unwrap();
        let frame = CapturedFrame {
            id: Uuid::new_v4(),
            url: target.url.clone(),
            method: "GET".into(),
            request_headers: HashMap::new(),
            request_body: None,
            status_code: 200,
            response_headers: HashMap::from([("content-type".into(), "application/json".into())]),
            response_body: Some(json!({"items":[1]})),
            timestamp: Utc::now(),
            resource_type: ResourceType::Other,
        };
        let ready = std::future::ready(Some(ProbeOutput {
            frames: vec![frame],
            auth: AuthContext::default(),
            final_url: target.url.clone(),
            page_title: Some("Items".into()),
            duration_ms: u64::MAX,
        }));
        let output = engine
            .probe_with_ssr(&target, Duration::from_secs(1), ready)
            .await
            .unwrap();
        assert_eq!(output.frames[0].response_body, Some(json!({"items":[1]})));
        assert_eq!(output.final_url, target.url);
        assert_eq!(output.page_title.as_deref(), Some("Items"));
        assert!(output.duration_ms < u64::MAX);
    }
    #[tokio::test]
    async fn ssr_misses_and_timeouts_fall_back_and_cancel_the_http_future() {
        struct Cancelled(Arc<AtomicBool>);
        impl Drop for Cancelled {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let engine = ProbeEngine::with_chromium("/missing/reify-test-chromium");
        let target = Target::new("https://example.com", None).unwrap();
        let miss = engine
            .probe_with_ssr(&target, Duration::from_secs(1), std::future::ready(None))
            .await
            .err()
            .unwrap();
        assert!(miss.to_string().contains("spawn Chromium"));
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancellation = cancelled.clone();
        let slow = async move {
            let _guard = Cancelled(cancellation);
            std::future::pending::<Option<ProbeOutput>>().await
        };
        let timeout = engine
            .probe_with_ssr(&target, Duration::from_millis(100), slow)
            .await
            .err()
            .unwrap();
        assert!(timeout.to_string().contains("spawn Chromium"));
        assert!(cancelled.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn zero_deadline_does_not_poll_the_http_path() {
        let engine = ProbeEngine::new();
        let target = Target::new("https://example.com", None).unwrap();
        let result = engine
            .probe_with_ssr(&target, Duration::ZERO, async {
                panic!("zero-budget HTTP future was polled")
            })
            .await;
        assert!(result.is_err());
    }
}
