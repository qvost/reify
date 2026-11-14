use reify_core::domain::{CapturedFrame, ResourceType};
use reify_probe::{
    auth::{AuthExtractor, BrowserCookie},
    cdp::{CdpClient, Incoming, parse_message},
    filter, process,
};
use serde_json::{Value, json};
use std::{collections::HashMap, path::PathBuf, time::Duration};
use url::Url;

fn url(s: &str) -> Url {
    Url::parse(s).unwrap()
}
#[test]
fn static_noise_and_tracker_filtering() {
    for suffix in [
        "png", "jpg", "jpeg", "svg", "webp", "ico", "css", "woff", "woff2", "ttf", "mp4", "webm",
        "mp3",
    ] {
        assert!(filter::is_noise(
            &url(&format!("https://example.com/asset.{suffix}?v=1")),
            "Fetch"
        ));
    }
    for tracker in [
        "google-analytics",
        "doubleclick",
        "segment.io",
        "sentry",
        "datadog",
        "clarity.ms",
        "mixpanel",
        "intercom",
        "hotjar",
        "facebook.net",
        "launchdarkly",
        "telemetry",
    ] {
        assert!(filter::is_noise(
            &url(&format!("https://example.com/{tracker}/collect")),
            "XHR"
        ));
    }
    for kind in ["Image", "Stylesheet", "Font", "Media", "Script"] {
        assert!(filter::is_noise(
            &url("https://example.com/no-extension"),
            kind
        ));
    }
    for endpoint in [
        "https://example.com/api/items",
        "https://example.com/graphql?query=%7Bx%7D",
    ] {
        assert!(!filter::is_noise(&url(endpoint), "Fetch"));
    }
}
#[test]
fn first_party_scope_has_hostname_boundaries() {
    let target = url("https://example.com");
    for candidate in ["https://example.com/api", "https://api.example.com/v1"] {
        assert!(filter::is_first_party(&target, &url(candidate)));
    }
    for candidate in [
        "https://example.com.evil.org",
        "https://notexample.com",
        "https://other.org",
        "ftp://example.com",
    ] {
        assert!(!filter::is_first_party(&target, &url(candidate)));
    }
    assert!(!filter::is_first_party(
        &url("https://www.example.com"),
        &url("https://api.example.com")
    ));
    assert!(filter::is_first_party(
        &url("http://127.0.0.1"),
        &url("http://127.0.0.1:8888")
    ));
}
#[test]
fn api_mime_initiator_and_graphql_retained() {
    let mut frame = CapturedFrame {
        id: uuid::Uuid::new_v4(),
        url: url("https://example.com/api"),
        method: "POST".into(),
        request_headers: HashMap::new(),
        request_body: None,
        status_code: 200,
        response_headers: HashMap::new(),
        response_body: None,
        timestamp: chrono::Utc::now(),
        resource_type: ResourceType::Other,
    };
    assert!(!filter::is_api(&frame, "text/html"));
    for mime in [
        "application/json",
        "text/json; charset=utf-8",
        "application/graphql",
        "application/problem+json",
    ] {
        assert!(filter::is_api(&frame, mime));
    }
    frame.resource_type = ResourceType::Xhr;
    assert!(filter::is_api(&frame, "text/plain"));
    frame.resource_type = ResourceType::Fetch;
    assert!(filter::is_api(&frame, "text/plain"));
    frame.resource_type = ResourceType::Other;
    frame.request_body = Some(json!({"query":"{users{id}}"}));
    assert!(filter::is_api(&frame, "text/plain"));
}
#[test]
fn scoped_cookies_csrf_and_authorization() {
    let mut auth = AuthExtractor::default();
    auth.observe_headers(
        &HashMap::from([
            ("Authorization".into(), "Bearer secret".into()),
            ("X-API-Key".into(), "api-secret".into()),
            ("Cookie".into(), "unparsed=ignored".into()),
        ]),
        true,
    );
    auth.observe_headers(
        &HashMap::from([
            ("X-CSRF-Token".into(), "response-token".into()),
            ("Authorization".into(), "never-use-response-auth".into()),
        ]),
        false,
    );
    let cookies: Vec<BrowserCookie> = serde_json::from_value(json!([
        {"name":"session","value":"cookie-secret","domain":".example.com","expires":4102444800.0},
        {"name":"csrftoken","value":"csrf-secret","domain":"example.com","expires":-1},
        {"name":"xsrf-token","value":"xsrf-secret","domain":".example.com"},
        {"name":"_csrf","value":"csrf2","domain":"example.com"},
        {"name":"evil","value":"bad","domain":"example.com.evil"},
        {"name":"old","value":"bad","domain":"example.com","expires":1},
        {"name":"empty-domain","value":"bad","domain":""}
    ]))
    .unwrap();
    auth.cookies(&url("https://example.com"), cookies);
    let auth = auth.finish();
    assert_eq!(auth.authorization_header.as_deref(), Some("Bearer secret"));
    assert_eq!(auth.custom_headers["x-api-key"], "api-secret");
    assert!(!auth.custom_headers.contains_key("cookie"));
    assert_eq!(auth.csrf_tokens["x-csrf-token"], "response-token");
    assert_eq!(auth.csrf_tokens["csrftoken"], "csrf-secret");
    assert_eq!(auth.cookies.len(), 4);
    assert!(auth.expires_at.is_some());
}
#[test]
fn cdp_parsing_differentiates_replies_events_and_errors() {
    match parse_message(r#"{"id":7,"result":{"body":"{}","base64Encoded":false}}"#).unwrap() {
        Incoming::Reply(7, Ok(value)) => assert_eq!(value["body"], "{}"),
        other => panic!("unexpected {other:?}"),
    }
    match parse_message(
        r#"{"method":"Network.loadingFinished","sessionId":"page","params":{"requestId":"r"}}"#,
    )
    .unwrap()
    {
        Incoming::Event(event) => {
            assert_eq!(event.session_id.as_deref(), Some("page"));
            assert_eq!(event.params["requestId"], "r");
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(matches!(
        parse_message(r#"{"id":1,"error":{"code":-32000,"message":"secret"}}"#).unwrap(),
        Incoming::Reply(1, Err(_))
    ));
    for bad in ["{", "null", "{}", r#"{"id":2}"#] {
        assert!(
            parse_message(bad).is_err()
                || matches!(parse_message(bad).unwrap(), Incoming::Reply(_, Err(_)))
        );
    }
}
#[test]
fn endpoint_announcements_require_loopback() {
    assert_eq!(
        process::devtools_endpoint(
            "DevTools listening on ws://127.0.0.1:1234/devtools/browser/id\n"
        )
        .unwrap()
        .port(),
        Some(1234)
    );
    for bad in [
        "noise",
        "DevTools listening on ws://evil.org:9/x",
        "DevTools listening on http://localhost:9/x",
    ] {
        assert!(process::devtools_endpoint(bad).is_none());
    }
}
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("reify-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[cfg(unix)]
fn script(path: &std::path::Path, source: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, source).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}
#[cfg(unix)]
#[test]
fn chromium_discovery_override_playwright_and_system() {
    let dir = Temp::new();
    let system = dir.0.join("chromium");
    script(&system, "#!/bin/sh\nexit 0\n");
    let path = std::env::join_paths([&dir.0]).unwrap();
    assert_eq!(
        process::discover_in(None, None, Some(&path)).unwrap(),
        system
    );
    let chrome = dir
        .0
        .join(".cache/ms-playwright/chromium-123/chrome-linux64/chrome");
    std::fs::create_dir_all(chrome.parent().unwrap()).unwrap();
    script(&chrome, "#!/bin/sh\nexit 0\n");
    assert_eq!(
        process::discover_in(None, Some(&dir.0), Some(&path)).unwrap(),
        chrome
    );
    assert_eq!(
        process::discover_in(Some(&system), Some(&dir.0), Some(&path)).unwrap(),
        system
    );
    assert!(process::discover_in(Some(&dir.0.join("missing")), Some(&dir.0), Some(&path)).is_err());
}
#[tokio::test]
async fn socket_correlates_out_of_order_replies_and_reports_disconnect() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{accept_async, tungstenite::Message};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();
        let a: Value =
            serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
        let b: Value =
            serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
        ws.send(Message::Text(
            json!({"method":"Page.loadEventFired","params":{},"sessionId":"page"}).to_string(),
        ))
        .await
        .unwrap();
        for v in [b, a] {
            ws.send(Message::Text(
                json!({"id":v["id"],"result":{"method":v["method"]}}).to_string(),
            ))
            .await
            .unwrap();
        }
        ws.close(None).await.unwrap();
    });
    let mut client = CdpClient::connect(&format!("ws://{address}"))
        .await
        .unwrap();
    let mut events = client.take_events().unwrap();
    let (a, b) = tokio::join!(
        client.command("one", json!({}), None),
        client.command("two", json!({}), Some("page"))
    );
    assert_eq!(a.unwrap()["method"], "one");
    assert_eq!(b.unwrap()["method"], "two");
    assert_eq!(events.recv().await.unwrap().method, "Page.loadEventFired");
    server.await.unwrap();
    assert!(
        client
            .command("after-close", json!({}), None)
            .await
            .is_err()
    );
}
#[cfg(unix)]
#[tokio::test]
async fn process_drop_reaps_and_cleans_profile() {
    let dir = Temp::new();
    let binary = dir.0.join("chrome");
    let record = dir.0.join("record");
    script(
        &binary,
        &format!(
            "#!/bin/sh\nprofile=''\nfor arg in \"$@\"; do case \"$arg\" in --user-data-dir=*) profile=${{arg#*=}};; esac; done\nprintf '%s\\n%s\\n' \"$$\" \"$profile\" > '{}'\necho 'DevTools listening on ws://127.0.0.1:12345/devtools/browser/test' >&2\ntrap '' TERM\nwhile :; do sleep 1; done\n",
            record.display()
        ),
    );
    let process = process::ChromiumProcess::launch(&binary).await.unwrap();
    assert_eq!(process.endpoint().port(), Some(12345));
    let recorded = std::fs::read_to_string(record).unwrap();
    let mut lines = recorded.lines();
    let pid: u32 = lines.next().unwrap().parse().unwrap();
    let profile = PathBuf::from(lines.next().unwrap());
    assert!(profile.exists());
    drop(process);
    tokio::time::timeout(Duration::from_secs(3), async {
        while profile.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
}
#[tokio::test]
async fn zero_budget_returns_error() {
    let target = reify_core::domain::Target::new("https://example.com", None).unwrap();
    assert!(
        reify_probe::ProbeEngine::with_chromium("/does/not/exist")
            .probe(&target, Duration::ZERO)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn loopback_http_discovers_websocket() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let http = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = http.local_addr().unwrap().port();
    let ws = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_address = ws.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = http.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let byte = stream.read_u8().await.unwrap();
            request.push(byte);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
            assert!(request.len() < 8192);
        }
        assert!(String::from_utf8_lossy(&request).starts_with("GET /json/version "));
        let body =
            json!({"webSocketDebuggerUrl":format!("ws://{ws_address}/devtools/browser/test")})
                .to_string();
        stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        let (socket, _) = ws.accept().await.unwrap();
        let _socket = tokio_tungstenite::accept_async(socket).await.unwrap();
    });
    let _client = CdpClient::discover(port).await.unwrap();
    server.await.unwrap();
}
