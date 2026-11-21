use reify_cli::{Cli, Command, args::parse_arguments, ui};
use serde_json::json;

#[test]
fn diagnostic_report_includes_synthesized_query_schema_without_credential_bundle() {
    let target = reify_core::domain::Target::new("https://example.com", None).unwrap();
    let frame = serde_json::from_value(json!({
        "id":"00000000-0000-4000-8000-000000000001",
        "url":"https://example.com/api/search?q=test", "method":"GET",
        "request_headers":{}, "request_body":null, "status_code":200,
        "response_headers":{"content-type":"application/json"},
        "response_body":{"stories":[{"title":"Fixture"}]},
        "timestamp":"2026-01-01T00:00:00Z", "resource_type":"Fetch"
    }))
    .unwrap();
    let mut auth = reify_core::domain::AuthContext::default();
    auth.cookies
        .insert("session".into(), "private-cookie-value".into());
    let output = reify_probe::ProbeOutput {
        frames: vec![frame],
        auth,
        final_url: target.url.clone(),
        page_title: Some("Fixture".into()),
        duration_ms: 1,
    };
    let compiled = reify_synth::SynthEngine::new()
        .synthesize(&target, &output.frames, &output.auth)
        .unwrap();
    let report = serde_json::to_value(reify_cli::commands::probe::report_with_tools(
        &output,
        &compiled.tools,
    ))
    .unwrap();
    assert_eq!(report["frames"].as_array().unwrap().len(), 1);
    assert_eq!(
        report["tools"][0]["inputSchema"]["properties"]["q"]["type"],
        "string"
    );
    assert!(report.get("auth").is_none());
    assert!(!report.to_string().contains("private-cookie-value"));
}

#[test]
fn test_cli_argument_parsing() {
    let parsed = Cli::parse_checked_from([
        "reify",
        "probe",
        "https://news.ycombinator.com",
        "search stories",
    ])
    .unwrap();
    assert!(
        matches!(parsed.command, Some(Command::Probe { goal, .. }) if goal == ["search stories"])
    );
    let parsed =
        Cli::parse_checked_from(["reify", "https://Example.com/", "search", "flights"]).unwrap();
    assert_eq!(parsed.url.unwrap().as_str(), "https://example.com/");
    assert_eq!(parsed.goal, vec!["search", "flights"]);
    let parsed = Cli::parse_checked_from(["reify", "serve"]).unwrap();
    assert!(matches!(parsed.command,Some(Command::Serve{host,port:8080}) if host=="127.0.0.1"));
    let parsed =
        Cli::parse_checked_from(["reify", "serve", "--host", "::1", "--port", "9000"]).unwrap();
    assert!(matches!(
        parsed.command,
        Some(Command::Serve { port: 9000, .. })
    ));
    assert!(matches!(
        Cli::parse_checked_from(["reify", "probe", "https://example.com"])
            .unwrap()
            .command,
        Some(Command::Probe { .. })
    ));
    let parsed = Cli::parse_checked_from([
        "reify",
        "call",
        "https://example.com",
        "search_flights",
        r#"{"body":{"active":true}}"#,
    ])
    .unwrap();
    assert!(
        matches!(parsed.command,Some(Command::Call{json_arguments,..}) if json_arguments==json!({"body":{"active":true}}))
    );
    assert!(matches!(
        Cli::parse_checked_from(["reify", "bridge", "--port", "9099"])
            .unwrap()
            .command,
        Some(Command::Bridge { port: 9099, .. })
    ));
    let parsed = Cli::parse_checked_from([
        "reify",
        "--timeout",
        "10",
        "--color",
        "never",
        "probe",
        "http://localhost",
    ])
    .unwrap();
    assert_eq!(parsed.timeout, 10);
    let parsed = Cli::parse_checked_from([
        "reify",
        "--fresh",
        "https://news.ycombinator.com",
        "search stories",
    ])
    .unwrap();
    assert!(parsed.fresh);
    let parsed = Cli::parse_checked_from([
        "reify",
        "https://news.ycombinator.com",
        "search stories",
        "--fresh",
    ])
    .unwrap();
    assert!(parsed.fresh);
    assert_eq!(parsed.goal, ["search stories"]);
    assert!(matches!(
        Cli::parse_checked_from(["reify", "profiles"])
            .unwrap()
            .command,
        Some(Command::Profiles)
    ));
    assert!(matches!(
        Cli::parse_checked_from([
            "reify",
            "watch",
            "https://example.com",
            "--goal",
            "search items",
            "--interval",
            "5",
            "--once",
            "--webhook",
            "https://hooks.example.com/alert",
        ])
        .unwrap()
        .command,
        Some(Command::Watch {
            interval: 5,
            once: true,
            goal: Some(goal),
            webhook: Some(_),
            ..
        }) if goal == "search items"
    ));
    assert_eq!(
        Cli::parse_checked_from(["reify", "watch", "https://example.com", "--interval", "4",])
            .unwrap_err()
            .exit_code(),
        2
    );
    assert!(
        Cli::parse_checked_from(["reify", "--no-profile", "https://github.com"])
            .unwrap()
            .fresh
    );
}
#[test]
fn test_cli_error_reporting() {
    for argv in [
        vec!["reify"],
        vec!["reify", "not-a-url"],
        vec!["reify", "ftp://example.com"],
        vec!["reify", "probe"],
        vec!["reify", "call", "https://example.com"],
        vec!["reify", "call", "https://example.com", "tool", "[]"],
        vec!["reify", "call", "https://example.com", "tool", "{"],
        vec!["reify", "serve", "--port", "99999"],
        vec!["reify", "--timeout", "0", "probe", "https://example.com"],
    ] {
        let error = Cli::parse_checked_from(argv).unwrap_err();
        let text = error.to_string();
        assert!(!text.contains("panicked"));
        assert!(!text.contains("stack backtrace"));
        assert_eq!(error.exit_code(), 2);
    }
    assert!(parse_arguments("false").is_err());
}
#[test]
fn binary_reports_invalid_url_without_panicking() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_reify"))
        .args(["probe", "ftp://example.com"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("HTTP"));
    assert!(!stderr.contains("panicked"));
}
#[test]
fn terminal_rendering_is_safe_and_configuration_is_current() {
    assert_eq!(
        ui::safe_terminal("hello\x1b[31m\nworld\u{202e}"),
        "hello [31m world "
    );
    let config = ui::configuration("http://127.0.0.1:8080/mcp");
    assert!(config.contains("mcpServers"));
    assert!(config.contains("servers"));
    assert!(config.contains("2026-07-28"));
    assert!(!config.contains('\x1b'));
    assert_eq!(
        ui::parameter_type(&json!({"anyOf":[{"type":"null"},{"type":"string"}]})),
        "null | string"
    );
}
#[tokio::test]
async fn shutdown_waiter_completes_for_signals_and_dropped_senders() {
    let (sender, receiver) = tokio::sync::watch::channel(false);
    sender.send(true).unwrap();
    reify_cli::commands::wait_for_shutdown(receiver).await;
    let (sender, receiver) = tokio::sync::watch::channel(false);
    drop(sender);
    reify_cli::commands::wait_for_shutdown(receiver).await;
}

#[test]
fn goals_can_contain_subcommand_words_and_scoped_configs_have_headers() {
    let parsed = Cli::parse_checked_from([
        "reify",
        "--color",
        "never",
        "https://example.com",
        "serve",
        "content",
    ])
    .unwrap();
    assert!(parsed.command.is_none());
    assert_eq!(parsed.goal, vec!["serve", "content"]);
    let config = ui::configuration_for_mount("http://127.0.0.1:8080/mcp", Some("mount-token"));
    assert_eq!(config.matches("Reify-Mount").count(), 3);
}

#[cfg(unix)]
#[test]
fn failing_diagnostic_probe_reaps_the_browser_before_binary_exit() {
    use std::os::unix::fs::PermissionsExt;
    let directory = std::env::temp_dir().join(format!(
        "reify-cli-cleanup-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let binary = directory.join("chrome");
    let record = directory.join("record");
    std::fs::write(&binary,format!("#!/bin/sh\nfor arg in \"$@\"; do case \"$arg\" in --user-data-dir=*) printf '%s' \"${{arg#*=}}\" > '{}' ;; esac; done\necho 'mock browser intentionally exits before CDP discovery' >&2\nexit 1\n",record.display())).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_reify"))
        .args(["probe", "http://127.0.0.1:1", "--chromium"])
        .arg(&binary)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&result.stderr).contains("panicked"));
    let profile = std::fs::read_to_string(record).unwrap();
    assert!(!std::path::Path::new(&profile).exists());
    std::fs::remove_dir_all(directory).unwrap();
}
