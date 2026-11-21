#[path = "../../../test-support/environment.rs"]
mod environment;
use reify_core::domain::Target;
use reify_probe::{ProbeEngine, ProbeOutput, action, process};
use serde_json::json;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    task::JoinHandle,
};

/// Real DOM/event/network fixtures; each probe owns its browser and profile.
struct Fixture {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}

impl Fixture {
    async fn new(html: String) -> Option<Self> {
        let listener = environment::loopback_listener().await?;
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let html = Arc::new(html);
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let html = html.clone();
                let recorded = recorded.clone();
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        let Ok(byte) = socket.read_u8().await else {
                            return;
                        };
                        request.push(byte);
                        assert!(request.len() < 16384);
                    }
                    let request = String::from_utf8(request).unwrap();
                    let path = request.split_whitespace().nth(1).unwrap();
                    recorded.lock().unwrap().push(path.into());
                    let (mime, body) = if path.starts_with("/api/") {
                        if path.starts_with("/api/hang") {
                            tokio::time::sleep(Duration::from_secs(2)).await;
                        }
                        (
                            "application/json",
                            json!({"stories":[{"title":"Fixture", "id":1}]}).to_string(),
                        )
                    } else if path.starts_with("/results") {
                        (
                            "text/html",
                            "<script>fetch('/api/search'+location.search)</script>".into(),
                        )
                    } else {
                        ("text/html", html.to_string())
                    };
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(reply.as_bytes()).await;
                });
            }
        });
        Some(Self {
            url,
            requests,
            task,
        })
    }

    async fn probe(&self, goal: Option<&str>) -> Option<ProbeOutput> {
        static BROWSERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
        let _permit = BROWSERS.acquire().await.unwrap();
        let Ok(binary) = process::discover() else {
            eprintln!("skipping DOM integration test: Chromium not installed in this environment");
            return None;
        };
        let target = Target::new(&self.url, goal.map(str::to_owned)).unwrap();
        match ProbeEngine::with_chromium(binary)
            .probe(&target, Duration::from_secs(15))
            .await
        {
            Ok(output) => Some(output),
            Err(reify_core::ReifyError::ExecutionError(message)) => {
                eprintln!("skipping DOM integration test in restricted sandbox: {message}");
                None
            }
            Err(error) => panic!("DOM probe failed: {error}"),
        }
    }

    fn assert_no_unsafe_requests(&self) {
        assert!(
            !self.requests.lock().unwrap().iter().any(|path| path
                .split('?')
                .next()
                .unwrap()
                .contains("unsafe"))
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn empty_passive_capture_excites_debounced_search_and_reads_body() {
    let Some(fixture) = Fixture::new(r#"<input type="search" oninput="setTimeout(() => fetch('/api/search?q='+encodeURIComponent(this.value)), 200)">"#.into()).await else { return; };
    let Some(output) = fixture.probe(None).await else {
        return;
    };
    assert_eq!(output.frames.len(), 1);
    let frame = &output.frames[0];
    assert_eq!(frame.url.query(), Some("q=test"));
    assert_eq!(
        frame.response_body,
        Some(json!({"stories":[{"title":"Fixture", "id":1}]}))
    );
    let target = Target::new(&fixture.url, None).unwrap();
    let compiled = reify_synth::SynthEngine::new()
        .synthesize(&target, &output.frames, &output.auth)
        .unwrap();
    assert_eq!(compiled.tools[0].definition.name, "search_endpoint");
    assert_eq!(
        compiled.tools[0].definition.input_schema["properties"]["q"]["type"],
        "string"
    );
}

#[tokio::test]
async fn interactive_goal_excites_even_when_passive_frame_matches() {
    let Some(fixture) = Fixture::new(r#"<input name="query" oninput="fetch('/api/search?q='+encodeURIComponent(this.value))"><script>fetch('/api/stories')</script>"#.into()).await else { return; };
    let Some(output) = fixture.probe(Some("search stories")).await else {
        return;
    };
    assert_eq!(output.frames.len(), 2);
    assert_eq!(output.frames[0].url.path(), "/api/stories");
    assert_eq!(output.frames[1].url.query(), Some("q=stories"));
}

#[tokio::test]
async fn unmatched_noninteractive_goal_excites_safe_filter() {
    let Some(fixture) = Fixture::new(r#"<input placeholder="Filter catalog" onchange="fetch('/api/catalog?q='+this.value)"><script>fetch('/api/bootstrap')</script>"#.into()).await else { return; };
    let Some(output) = fixture.probe(Some("catalog")).await else {
        return;
    };
    assert_eq!(output.frames.len(), 2);
    assert_eq!(output.frames[1].url.path(), "/api/catalog");
}

#[tokio::test]
async fn enter_handlers_receive_legacy_key_code_and_can_cancel_submission() {
    let Some(fixture) = Fixture::new(r#"<form action="/unsafe-submit"><input type="search" onkeydown="if(event.key==='Enter' && event.keyCode===13){event.preventDefault();fetch('/api/search?q='+this.value)}" onkeyup="if(event.which===13)fetch('/api/keyup')"></form>"#.into()).await else { return; };
    let Some(output) = fixture.probe(None).await else {
        return;
    };
    assert_eq!(output.frames.len(), 2);
    fixture.assert_no_unsafe_requests();
}

#[tokio::test]
async fn safe_get_form_navigation_is_observed_without_a_second_action() {
    let Some(fixture) = Fixture::new(
        r#"<form action="/results"><label>Search <input name="q"></label></form>"#.into(),
    )
    .await
    else {
        return;
    };
    let Some(output) = fixture.probe(None).await else {
        return;
    };
    assert_eq!(output.frames.len(), 1);
    assert_eq!(output.frames[0].url.query(), Some("q=test"));
    assert_eq!(output.final_url.path(), "/results");
    assert!(output.frames[0].response_body.is_some());
}

#[tokio::test]
async fn explicit_cross_host_search_destination_is_captured() {
    let Some(destination) = Fixture::new(String::new()).await else {
        return;
    };
    let form_url = destination.url.replace("127.0.0.1", "localhost");
    let Some(fixture) = Fixture::new(format!(
        r#"<form action="{form_url}/results"><input name="q"></form>"#
    ))
    .await
    else {
        return;
    };
    let Some(output) = fixture.probe(None).await else {
        return;
    };
    assert_eq!(output.frames.len(), 1);
    assert_eq!(output.frames[0].url.host_str(), Some("localhost"));
    assert!(output.frames[0].response_body.is_some());
}

#[tokio::test]
async fn safe_search_button_is_clicked_once() {
    let Some(fixture) = Fixture::new(
        r#"<button type="button" onclick="fetch('/api/search')">Search</button>"#.into(),
    )
    .await
    else {
        return;
    };
    let Some(output) = fixture.probe(None).await else {
        return;
    };
    assert_eq!(output.frames.len(), 1);
}

#[tokio::test]
async fn unsafe_forms_external_controls_and_unrelated_inputs_are_never_touched() {
    let mut html = String::from("<title>Safety fixture</title>");
    for (index, marker) in [
        "password", "email", "tel", "payment", "card", "delete", "checkout",
    ]
    .iter()
    .enumerate()
    {
        html.push_str(&format!(
            r#"<form id="f{index}"><input type="search"><input name="{marker}"></form>"#
        ));
    }
    html.push_str(r#"
        <form id="external"><input type="search"></form><input form="external" type="password">
        <form action="/checkout"><input type="search"></form>
        <form method="post"><input type="search"></form>
        <input name="username"><input type="email" name="search"><input type="tel" name="query">
        <input type="search" hidden><input type="search" disabled><input type="search" readonly>
        <fieldset disabled><input type="search"></fieldset>
        <form><input type="search" style="display:none"><button formaction="/delete">Search</button></form>
        <form><button formmethod="post">Search</button></form>
        <button type="reset">Search</button>
        <button>Checkout search</button>
        <script>
            for(const kind of ['focusin','input','change','keydown','keyup','submit','click'])
                document.addEventListener(kind, event => {event.preventDefault();fetch('/api/unsafe?kind='+kind)},true);
        </script>"#);
    let Some(fixture) = Fixture::new(html).await else {
        return;
    };
    let Some(output) = fixture.probe(None).await else {
        return;
    };
    assert!(output.frames.is_empty());
    fixture.assert_no_unsafe_requests();
}

#[tokio::test]
async fn only_one_safe_control_is_excited_and_goal_cannot_inject_script() {
    let Some(fixture) = Fixture::new(r#"<input type="search" oninput="fetch('/api/search?q='+encodeURIComponent(this.value))"><input name="q" oninput="fetch('/api/unsafe-second')">"#.into()).await else { return; };
    let Some(output) = fixture
        .probe(Some("find cats\");fetch('/api/unsafe-injection');//"))
        .await
    else {
        return;
    };
    assert_eq!(output.frames.len(), 1);
    assert!(
        output.frames[0]
            .url
            .query_pairs()
            .any(|(key, value)| key == "q" && value.starts_with("cats"))
    );
    fixture.assert_no_unsafe_requests();
}

#[tokio::test]
async fn absent_controls_no_network_and_slow_responses_are_benign_and_bounded() {
    for html in [
        "<p>No discovery controls</p>",
        "<input type=search>",
        "<input type=search oninput=\"fetch('/api/hang')\">",
    ] {
        let Some(fixture) = Fixture::new(html.into()).await else {
            return;
        };
        let Some(output) = fixture.probe(None).await else {
            return;
        };
        assert!(output.frames.is_empty());
        assert!(
            output.duration_ms < 5000,
            "action did not respect its observation deadline"
        );
    }
}

#[test]
fn interactive_goal_detection_uses_words() {
    assert!(action::is_interactive(Some("Search stories")));
    assert!(action::is_interactive(Some("filtering a table")));
    assert!(!action::is_interactive(Some("research papers")));
    assert!(!action::is_interactive(None));
}

#[tokio::test]
async fn interactive_goal_uses_the_browser_even_when_ssr_state_exists() {
    let Some(fixture) = Fixture::new(
        r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"initial":true}}</script>
        <input type="search" oninput="fetch('/api/search?q='+this.value)">"#
            .into(),
    )
    .await
    else {
        return;
    };
    let Some(output) = fixture.probe(Some("search stories")).await else {
        return;
    };
    assert_eq!(output.frames.len(), 1);
    assert_eq!(output.frames[0].url.path(), "/api/search");
}

#[tokio::test]
async fn safety_is_rechecked_after_application_event_handlers_mutate_the_form() {
    let Some(fixture) = Fixture::new(r#"<form><input type="search"
        oninput="fetch('/api/search');this.form.insertAdjacentHTML('beforeend','<input type=password>')"
        onchange="fetch('/api/unsafe-change')" onkeydown="fetch('/api/unsafe-enter')"></form>"#.into()).await else { return; };
    let Some(output) = fixture.probe(None).await else {
        return;
    };
    assert_eq!(output.frames.len(), 1);
    assert_eq!(output.frames[0].url.path(), "/api/search");
    fixture.assert_no_unsafe_requests();
}
