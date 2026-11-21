use serde_json::Value;
use std::{
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    process::Command,
    sync::oneshot,
};

struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn mock_target(change_endpoint_after_first_request: bool) -> Option<Server> {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!("skipping loopback watch test: {error}");
            return None;
        }
        Err(error) => panic!("could not bind mock target: {error}"),
    };
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let requests = requests.clone();
            tokio::spawn(async move {
                let Some(request) = read_request(&mut stream).await else {
                    return;
                };
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                let (status, content_type, body) = if path == "/" {
                    let request_number = requests.fetch_add(1, Ordering::SeqCst);
                    let version = if change_endpoint_after_first_request && request_number > 0 {
                        2
                    } else {
                        1
                    };
                    let body = format!(
                        r#"<!doctype html><html><head><title>Watch fixture</title></head><body><script id="__NEXT_DATA__">{{"props":{{"pageProps":{{"endpoint":"http://{address}/api/v{version}","items":[{{"id":1,"name":"sample"}}]}}}}}}</script></body></html>"#
                    );
                    (200, "text/html; charset=utf-8", body)
                } else if matches!(path, "/api/v1" | "/api/v2") {
                    (
                        200,
                        "application/json",
                        r#"{"items":[{"id":1}]}"#.to_owned(),
                    )
                } else {
                    (
                        404,
                        "application/json",
                        r#"{"error":"not found"}"#.to_owned(),
                    )
                };
                write_response(&mut stream, status, content_type, &body).await;
            });
        }
    });
    Some(Server {
        url: format!("http://{address}/"),
        task,
    })
}

async fn mock_webhook() -> Option<(Server, oneshot::Receiver<Value>)> {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!("skipping webhook watch test: {error}");
            return None;
        }
        Err(error) => panic!("could not bind mock webhook: {error}"),
    };
    let address = listener.local_addr().unwrap();
    let (sender, receiver) = oneshot::channel();
    let task = tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await
            && let Some(request) = read_request(&mut stream).await
        {
            let payload = request
                .split_once("\r\n\r\n")
                .map(|(_, body)| serde_json::from_str(body).unwrap());
            let _ = sender.send(payload.unwrap_or(Value::Null));
            write_response(&mut stream, 200, "application/json", "{}").await;
        }
    });
    Some((
        Server {
            url: format!("http://{address}/alert"),
            task,
        },
        receiver,
    ))
}

async fn read_request(stream: &mut TcpStream) -> Option<String> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let read = stream.read(&mut buffer).await.ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..read]);
        let text = String::from_utf8_lossy(&bytes);
        let Some((headers, body)) = text.split_once("\r\n\r\n") else {
            continue;
        };
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        if body.len() >= content_length {
            return Some(text.into_owned());
        }
    }
}

async fn write_response(stream: &mut TcpStream, status: u16, content_type: &str, body: &str) {
    let reason = if status == 200 { "OK" } else { "Not Found" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
}

async fn run_watch(args: &[&str]) -> std::process::Output {
    tokio::time::timeout(
        Duration::from_secs(12),
        Command::new(env!("CARGO_BIN_EXE_reify"))
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .expect("watch command timed out")
    .expect("could not run reify binary")
}

#[tokio::test]
async fn watch_once_succeeds_on_mock_target() {
    let Some(target) = mock_target(false).await else {
        return;
    };
    let output = run_watch(&["watch", &target.url, "--once", "--chromium", "/missing"]).await;

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("[REIFY WATCH] Monitoring:"));
    assert!(stdout.contains("[BASELINE] 1 tools active. Schema Hash:"));
    assert!(stdout.contains("[OK]"));
}

#[tokio::test]
async fn watch_detects_drift_and_triggers_webhook() {
    let Some(target) = mock_target(true).await else {
        return;
    };
    let Some((webhook, payload)) = mock_webhook().await else {
        return;
    };
    let output = run_watch(&[
        "watch",
        &target.url,
        "--once",
        "--chromium",
        "/missing",
        "--webhook",
        &webhook.url,
    ])
    .await;

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("[DRIFT DETECTED]"));
    assert!(stdout.contains("[SELF-HEALED]"));

    let payload = tokio::time::timeout(Duration::from_secs(3), payload)
        .await
        .expect("webhook was not called")
        .unwrap();
    assert_eq!(payload["event"], "schema_drift");
    assert_eq!(payload["target"], target.url);
    assert_ne!(payload["old_hash"], payload["new_hash"]);
    assert!(
        payload["tools"]
            .as_array()
            .is_some_and(|tools| !tools.is_empty())
    );
    assert!(payload["timestamp"].as_str().unwrap().contains('T'));
}

#[tokio::test]
async fn watch_honors_prewarmed_catalog() {
    let output = run_watch(&[
        "watch",
        "https://news.ycombinator.com",
        "--once",
        "--chromium",
        "/missing-chromium-for-profile-test",
    ])
    .await;

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("[REIFY WATCH] Monitoring: https://news.ycombinator.com/"));
    assert!(stdout.contains("[BASELINE] 2 tools active. Schema Hash:"));
}
