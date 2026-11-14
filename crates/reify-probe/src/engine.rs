//! Per-probe orchestration with bounded capture and concurrent response body reads.
use crate::{
    auth::{AuthExtractor, BrowserCookie},
    cdp::{CdpClient, CdpEvent},
    failure, filter,
    process::{self, ChromiumProcess},
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
    /// Completed, filtered exchanges in request-start order.
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
/// Stateless probe configuration. Clone freely; every call owns separate resources.
#[derive(Default, Clone)]
pub struct ProbeEngine {
    binary: Option<PathBuf>,
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
        }
    }
    /// Observe until page load plus 750ms network quiet, bounded by `timeout`.
    /// Timeout is a hard budget for startup/navigation/extraction; it returns an
    /// error rather than silently returning an incomplete credential snapshot.
    pub async fn probe(
        &self,
        target: &Target,
        timeout: Duration,
    ) -> Result<ProbeOutput, ReifyError> {
        if timeout.is_zero() {
            return Err(failure("probe", "deadline exceeded"));
        }
        Target::new(target.url.as_str(), target.goal.clone())?;
        tokio::time::timeout(timeout, self.run(target))
            .await
            .map_err(|_| failure("probe", "deadline exceeded"))?
    }
    async fn run(&self, target: &Target) -> Result<ProbeOutput, ReifyError> {
        let started = Instant::now();
        let binary = match &self.binary {
            Some(p) => p.clone(),
            None => process::discover()?,
        };
        let process = ChromiumProcess::launch(&binary).await?;
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
        let mut bodies = FuturesUnordered::new();
        let mut loaded = false;
        let mut last_activity = tokio::time::Instant::now();
        loop {
            let quiet = last_activity + Duration::from_millis(750);
            tokio::select! {
                event = events.recv() => {
                    let event = event.ok_or_else(|| failure("CDP events", "socket closed or event queue overflowed"))?;
                    if event.session_id.as_deref() != Some(session) { continue; }
                    if event.method == "Page.loadEventFired" { loaded = true; last_activity = tokio::time::Instant::now(); }
                    if let Some((sequence, frame)) = capture.event(&event, &target.url) {
                        if bodies.len() >= 16 { return Err(failure("capture", "response concurrency limit exceeded")); }
                        let cdp = &cdp;
                        bodies.push(async move {
                            let response = cdp.command("Network.getResponseBody", json!({"requestId":sequence.1}), Some(session)).await;
                            (sequence.0, frame, response)
                        });
                    }
                    if !capture.pending.is_empty() { last_activity = tokio::time::Instant::now(); }
                }
                Some((sequence, mut frame, response)) = bodies.next(), if !bodies.is_empty() => {
                    if let Ok(response) = response { frame.response_body = decode_body(&response); }
                    // Bodies can be unavailable after eviction or for 204 responses;
                    // retain the completed request and response metadata in that case.
                    capture.completed.push((sequence, frame));
                    last_activity = tokio::time::Instant::now();
                }
                _ = tokio::time::sleep_until(quiet), if loaded && capture.pending.is_empty() && bodies.is_empty() => break,
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
fn headers(value: &Value) -> HashMap<String, String> {
    value
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.to_ascii_lowercase(), s.to_owned())))
                .collect()
        })
        .unwrap_or_default()
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
}
impl Capture {
    fn event(&mut self, event: &CdpEvent, target: &Url) -> Option<((u64, String), CapturedFrame)> {
        let p = &event.params;
        let id = p["requestId"].as_str()?;
        match event.method.as_str() {
            "Network.requestWillBeSent" => {
                // Redirects reuse request IDs; never associate an old body or auth
                // header with a new URL. The final hop becomes the captured exchange.
                self.pending.remove(id);
                if self.next >= MAX_FRAMES as u64 {
                    return None;
                }
                if filter::is_static_type(p["type"].as_str().unwrap_or("")) {
                    return None;
                }
                let r = &p["request"];
                let url = Url::parse(r["url"].as_str()?).ok()?;
                if !filter::is_first_party(target, &url)
                    || filter::is_noise(&url, p["type"].as_str().unwrap_or(""))
                {
                    return None;
                }
                let resource_type = match p["type"].as_str() {
                    Some("XHR") => ResourceType::Xhr,
                    Some("Fetch") => ResourceType::Fetch,
                    _ => ResourceType::Other,
                };
                let request_headers = headers(&r["headers"]);
                self.auth.observe_headers(&request_headers, true);
                let post = r["postData"].as_str().filter(|s| s.len() <= MAX_BODY);
                let request_body = post.and_then(|s| serde_json::from_str(s).ok()).or_else(|| {
                    let graphql = request_headers
                        .get("content-type")
                        .is_some_and(|m| m.starts_with("application/graphql"));
                    post.filter(|_| graphql).map(|s| json!({"query":s}))
                });
                let frame = CapturedFrame {
                    id: Uuid::new_v4(),
                    url,
                    method: r["method"].as_str()?.to_owned(),
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
                self.pending.insert(id.to_owned(), (sequence, frame));
            }
            "Network.responseReceived" => {
                let (_, frame) = self.pending.get_mut(id)?;
                let response = &p["response"];
                frame.response_headers = headers(&response["headers"]);
                let status = response["status"].as_f64()?;
                if !(100.0..=599.0).contains(&status) {
                    self.pending.remove(id);
                    return None;
                }
                frame.status_code = status as u16;
                self.auth.observe_headers(&frame.response_headers, false);
                if !filter::is_api(frame, response["mimeType"].as_str().unwrap_or("")) {
                    self.pending.remove(id);
                }
            }
            "Network.loadingFailed" => {
                self.pending.remove(id);
            }
            "Network.loadingFinished" => {
                let (sequence, frame) = self.pending.remove(id)?;
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
                return Some(((sequence, id.to_owned()), frame));
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
        capture.event(&request("https://example.com/graphql"), &target);
        assert_eq!(capture.pending.len(), 1);
        capture.event(&event("Network.responseReceived",json!({"requestId":"1","response":{"status":200,"headers":{"Content-Type":"application/json"},"mimeType":"application/json"}})),&target);
        let (_, frame) = capture
            .event(
                &event(
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
        capture.event(&request("https://example.com/graphql"), &target);
        capture.event(&request("https://evil.org/graphql"), &target);
        assert!(capture.pending.is_empty());
        capture.event(&request("https://example.com/api"), &target);
        capture.event(
            &event("Network.loadingFailed", json!({"requestId":"1"})),
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
}
