//! Autonomous upstream canary checks and schema generation recovery.
use crate::{Color, commands::shutdown_signal, ui};
use chrono::{SecondsFormat, Utc};
use reify_core::{
    ReifyError,
    domain::{CapturedFrame, CompiledTool, Target},
};
use reify_probe::ProbeEngine;
use reify_synth::SynthEngine;
use serde_json::{Value, json};
use std::{collections::BTreeSet, future::Future, time::Duration};
use tokio::task::JoinSet;
use url::Url;

const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const HTTP_TIMEOUT: Duration = Duration::from_secs(2);

struct Generation {
    tools: Vec<CompiledTool>,
    frames: Vec<CapturedFrame>,
    fingerprint: String,
    endpoint_routes: Vec<String>,
    profile: Option<&'static str>,
}

#[derive(Default)]
struct Health {
    not_found: Vec<String>,
    unhealthy: Vec<String>,
    unavailable: Vec<String>,
}

/// Monitor a target until Ctrl+C/SIGTERM, or perform one canary with once.
pub async fn run_watch(
    url: Url,
    goal: Option<String>,
    interval: u64,
    once: bool,
    webhook: Option<Url>,
    fresh: bool,
    color: Color,
) -> Result<(), ReifyError> {
    run_watch_with_shutdown(
        url,
        goal,
        interval,
        once,
        webhook,
        fresh,
        color,
        shutdown_signal(),
    )
    .await
}

/// Run the canary with an externally managed cancellation future.
#[allow(clippy::too_many_arguments)]
pub async fn run_watch_with_shutdown<F>(
    url: Url,
    goal: Option<String>,
    interval: u64,
    once: bool,
    webhook: Option<Url>,
    fresh: bool,
    color: Color,
    shutdown: F,
) -> Result<(), ReifyError>
where
    F: Future<Output = ()>,
{
    run_watch_with_probe(
        url,
        goal,
        interval,
        once,
        webhook,
        fresh,
        color,
        ProbeEngine::new(),
        shutdown,
    )
    .await
}

/// Run the canary with the CLI's configured probe and cancellation future.
#[allow(clippy::too_many_arguments)]
pub async fn run_watch_with_probe<F>(
    url: Url,
    goal: Option<String>,
    interval: u64,
    once: bool,
    webhook: Option<Url>,
    fresh: bool,
    color: Color,
    probe: ProbeEngine,
    shutdown: F,
) -> Result<(), ReifyError>
where
    F: Future<Output = ()>,
{
    let color = ui::use_color(color);
    if !(5..=86_400).contains(&interval) {
        return Err(ReifyError::ExecutionError(
            "watch interval must be between 5 and 86400 seconds".into(),
        ));
    }
    let target = Target::from_url(url, goal)?;
    let synth = SynthEngine::new();
    let client = http_client()?;
    tokio::pin!(shutdown);

    let initial = tokio::select! {
        result = compile_generation(&target, fresh, &probe, &synth) => result?,
        _ = &mut shutdown => {
            ui::write_watch(&ui::watch_line("[STOPPED]", "Shutdown requested.", color))?;
            return Ok(());
        }
    };
    let mut active = initial;
    ui::write_watch(&ui::watch_baseline(
        &target.url,
        active.tools.len(),
        short_hash(&active.fingerprint),
        color,
    ))?;

    let mut timer = tokio::time::interval(Duration::from_secs(interval));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                ui::write_watch(&ui::watch_line("[STOPPED]", "Shutdown requested.", color))?;
                return Ok(());
            }
            _ = timer.tick() => {}
        }

        let refreshed = tokio::select! {
            result = compile_generation(&target, fresh, &probe, &synth) => result,
            _ = &mut shutdown => {
                ui::write_watch(&ui::watch_line("[STOPPED]", "Shutdown requested.", color))?;
                return Ok(());
            }
        };
        let candidate = match refreshed {
            Ok(generation) => generation,
            Err(error) => {
                ui::write_watch(&ui::watch_line(
                    "[CANARY FAILED]",
                    &format!("Could not refresh the target: {error}"),
                    color,
                ))?;
                if once {
                    return Ok(());
                }
                continue;
            }
        };

        let health = tokio::select! {
            health = inspect_health(&target, &candidate, &client) => health,
            _ = &mut shutdown => {
                ui::write_watch(&ui::watch_line("[STOPPED]", "Shutdown requested.", color))?;
                return Ok(());
            }
        };
        let schema_changed = candidate.fingerprint != active.fingerprint;
        let drift = schema_changed || !health.not_found.is_empty();

        if drift {
            ui::write_watch(&ui::watch_line(
                "[DRIFT DETECTED]",
                "Upstream signature changed! Re-synthesizing...",
                color,
            ))?;
            let old_hash = active.fingerprint.clone();
            let new_hash = candidate.fingerprint.clone();
            let tool_names = candidate
                .tools
                .iter()
                .map(|tool| tool.definition.name.clone())
                .collect::<Vec<_>>();
            let tool_count = candidate.tools.len();
            active = candidate;
            ui::write_watch(&ui::watch_line(
                "[SELF-HEALED]",
                &format!(
                    "Schema updated: {} -> {} ({tool_count} tools).",
                    short_hash(&old_hash),
                    short_hash(&new_hash),
                ),
                color,
            ))?;
            if let Some(webhook) = &webhook
                && let Err(error) = notify_webhook(
                    &client,
                    webhook,
                    &target.url,
                    &old_hash,
                    &new_hash,
                    &tool_names,
                )
                .await
            {
                ui::write_watch(&ui::watch_line(
                    "[WEBHOOK FAILED]",
                    &error.to_string(),
                    color,
                ))?;
            }
        } else if !health.unhealthy.is_empty() {
            ui::write_watch(&ui::watch_line(
                "[UNHEALTHY]",
                &health.unhealthy.join(", "),
                color,
            ))?;
        } else if !health.unavailable.is_empty() {
            ui::write_watch(&ui::watch_line(
                "[UNAVAILABLE]",
                &format!("Could not verify: {}", health.unavailable.join(", ")),
                color,
            ))?;
        } else {
            ui::write_watch(&ui::watch_heartbeat(&timestamp(), color))?;
        }

        if once {
            return Ok(());
        }
    }
}

async fn compile_generation(
    target: &Target,
    fresh: bool,
    probe: &ProbeEngine,
    synth: &SynthEngine,
) -> Result<Generation, ReifyError> {
    if !fresh && let Some((profile, app)) = reify_server::prewarmed_app(target.clone())? {
        return generation(app.tools, Vec::new(), None, &target.url, Some(profile));
    }

    let output = probe.probe(target, PROBE_TIMEOUT).await?;
    let scope = Target::from_url(output.final_url.clone(), target.goal.clone())?;
    let page_title = output.page_title;
    let frames = output.frames;
    let auth = output.auth;
    let synth = *synth;
    let synth_frames = frames.clone();
    let synth_auth = auth.clone();
    let tools =
        tokio::task::spawn_blocking(move || synth.synthesize(&scope, &synth_frames, &synth_auth))
            .await
            .map_err(|_| ReifyError::ExecutionError("schema synthesis worker failed".into()))??
            .tools;
    generation(tools, frames, page_title, &target.url, None)
}

fn generation(
    tools: Vec<CompiledTool>,
    frames: Vec<CapturedFrame>,
    page_title: Option<String>,
    target_url: &Url,
    profile: Option<&'static str>,
) -> Result<Generation, ReifyError> {
    if tools.is_empty() {
        return Err(ReifyError::ExecutionError(
            "no tools were compiled for the watch target".into(),
        ));
    }
    let mut schemas = tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.definition.name,
                "method": tool.execution.http_method,
                "endpoint": tool.execution.upstream_url_template,
                "input_schema": tool.definition.input_schema,
            })
        })
        .collect::<Vec<_>>();
    schemas.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
    let endpoint_routes = endpoint_routes(&frames, target_url);
    let mut response_schemas = frames
        .iter()
        .filter_map(|frame| frame.response_body.as_ref().map(json_shape))
        .collect::<Vec<_>>();
    response_schemas.sort_by_key(Value::to_string);
    response_schemas.dedup();
    let representation = json!({
        "tools": schemas,
        "response_schemas": response_schemas,
        "endpoint_routes": endpoint_routes,
        "page_title": page_title,
    });
    let fingerprint = blake3::hash(&serde_json::to_vec(&representation)?)
        .to_hex()
        .to_string();
    Ok(Generation {
        tools,
        frames,
        fingerprint,
        endpoint_routes,
        profile,
    })
}

fn json_shape(value: &Value) -> Value {
    match value {
        Value::Null => json!({"type":"null"}),
        Value::Bool(_) => json!({"type":"boolean"}),
        Value::Number(number) => json!({
            "type": if number.is_i64() || number.is_u64() { "integer" } else { "number" }
        }),
        Value::String(_) => json!({"type":"string"}),
        Value::Array(values) => {
            let mut items = values.iter().map(json_shape).collect::<Vec<_>>();
            items.sort_by_key(Value::to_string);
            items.dedup();
            json!({"type":"array","items":items})
        }
        Value::Object(values) => {
            let properties = values
                .iter()
                .map(|(key, value)| (key.clone(), json_shape(value)))
                .collect::<serde_json::Map<_, _>>();
            json!({"type":"object","properties":properties})
        }
    }
}

async fn inspect_health(
    target: &Target,
    generation: &Generation,
    client: &reqwest::Client,
) -> Health {
    let mut checks = JoinSet::new();
    let page = target.url.clone();
    let page_client = client.clone();
    checks.spawn(async move {
        (
            "target page".to_owned(),
            request_status(&page_client, &page).await,
        )
    });

    if generation.profile.is_some() {
        for tool in &generation.tools {
            let endpoint = &tool.execution.upstream_url_template;
            let has_required_inputs = tool.definition.input_schema["required"]
                .as_array()
                .is_some_and(|required| !required.is_empty());
            if endpoint.contains('{')
                || endpoint.contains('}')
                || has_required_inputs
                || !tool.execution.http_method.eq_ignore_ascii_case("GET")
            {
                continue;
            }
            let Ok(endpoint) = Url::parse(endpoint) else {
                continue;
            };
            let label = tool.definition.name.clone();
            let endpoint_client = client.clone();
            checks.spawn(async move { (label, request_status(&endpoint_client, &endpoint).await) });
        }
    } else {
        for frame in &generation.frames {
            record_status(
                &mut checks,
                frame.url.clone(),
                frame.status_code,
                frame.method.clone(),
            );
        }
    }
    for route in &generation.endpoint_routes {
        let Ok(endpoint) = target.url.join(route) else {
            continue;
        };
        let route_client = client.clone();
        let label = format!("GET {route}");
        checks.spawn(async move { (label, request_status(&route_client, &endpoint).await) });
    }

    let mut health = Health::default();
    while let Some(result) = checks.join_next().await {
        let Ok((label, status)) = result else {
            health.unavailable.push("health check task failed".into());
            continue;
        };
        classify_status(&mut health, label, status);
    }
    health
}

fn endpoint_routes(frames: &[CapturedFrame], target_url: &Url) -> Vec<String> {
    fn visit(value: &serde_json::Value, target_url: &Url, routes: &mut BTreeSet<String>) {
        match value {
            serde_json::Value::String(value) => {
                let path = if value.starts_with("/api/") {
                    Some(value.split(['?', '#']).next().unwrap_or(value).to_owned())
                } else {
                    Url::parse(value).ok().and_then(|url| {
                        (url.origin() == target_url.origin() && url.path().starts_with("/api/"))
                            .then(|| url.path().to_owned())
                    })
                };
                if let Some(path) = path.filter(|path| !path.contains(['{', '}'])) {
                    routes.insert(path);
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    visit(value, target_url, routes);
                }
            }
            serde_json::Value::Object(values) => {
                for (key, value) in values {
                    if key.starts_with("/api/") {
                        let path = key.split(['?', '#']).next().unwrap_or(key);
                        if !path.contains(['{', '}']) {
                            routes.insert(path.to_owned());
                        }
                    }
                    visit(value, target_url, routes);
                }
            }
            _ => {}
        }
    }

    let mut routes = BTreeSet::new();
    for frame in frames {
        if frame.url.origin() == target_url.origin() && frame.url.path().starts_with("/api/") {
            routes.insert(frame.url.path().to_owned());
        }
        if let Some(body) = &frame.response_body {
            visit(body, target_url, &mut routes);
        }
    }
    routes.into_iter().collect()
}

fn record_status(
    checks: &mut JoinSet<(String, Option<u16>)>,
    url: Url,
    status: u16,
    method: String,
) {
    let label = format!("{method} {url}");
    checks.spawn(async move { (label, Some(status)) });
}

async fn request_status(client: &reqwest::Client, url: &Url) -> Option<u16> {
    if url.as_str().contains('{') || url.as_str().contains('}') {
        return None;
    }
    match client.head(url.clone()).send().await {
        Ok(response) if matches!(response.status().as_u16(), 405 | 501) => client
            .get(url.clone())
            .header(reqwest::header::RANGE, "bytes=0-0")
            .send()
            .await
            .ok()
            .map(|response| response.status().as_u16()),
        Ok(response) => Some(response.status().as_u16()),
        Err(_) => None,
    }
}

fn classify_status(health: &mut Health, label: String, status: Option<u16>) {
    match status {
        Some(404) => health.not_found.push(label),
        Some(code) if !(200..400).contains(&code) => {
            health
                .unhealthy
                .push(format!("{label} returned HTTP {code}"));
        }
        None => health.unavailable.push(label),
        Some(_) => {}
    }
}

async fn notify_webhook(
    client: &reqwest::Client,
    webhook: &Url,
    target: &Url,
    old_hash: &str,
    new_hash: &str,
    tools: &[String],
) -> Result<(), ReifyError> {
    let payload = json!({
        "event": "schema_drift",
        "target": target.as_str(),
        "old_hash": old_hash,
        "new_hash": new_hash,
        "tools": tools,
        "timestamp": timestamp(),
    });
    let response = client
        .post(webhook.clone())
        .json(&payload)
        .send()
        .await
        .map_err(|error| ReifyError::ExecutionError(format!("webhook request failed: {error}")))?;
    if !response.status().is_success() {
        return Err(ReifyError::ExecutionError(format!(
            "webhook returned HTTP {}",
            response.status()
        )));
    }
    Ok(())
}

fn http_client() -> Result<reqwest::Client, ReifyError> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(HTTP_TIMEOUT)
        .timeout(HTTP_TIMEOUT)
        .user_agent(concat!("reify-watch/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| {
            ReifyError::ExecutionError(format!("cannot create canary client: {error}"))
        })
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn short_hash(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use reify_core::domain::ResourceType;
    use serde_json::json;
    use std::collections::HashMap;
    use uuid::Uuid;

    fn captured_state(target: &Url, endpoint: &str) -> CapturedFrame {
        CapturedFrame {
            id: Uuid::new_v4(),
            url: target.clone(),
            method: "GET".into(),
            request_headers: HashMap::new(),
            request_body: None,
            status_code: 200,
            response_headers: HashMap::from([("content-type".into(), "application/json".into())]),
            response_body: Some(json!({"props":{"endpoint":endpoint,"items":[{"id":1}]}})),
            timestamp: Utc::now(),
            resource_type: ResourceType::Other,
        }
    }

    #[test]
    fn endpoint_reference_changes_are_part_of_the_schema_fingerprint() {
        let target = Url::parse("https://example.com/").unwrap();
        let profile = reify_core::profiles::PROFILES
            .iter()
            .find(|profile| profile.name == "Hacker News")
            .unwrap();
        let tools = reify_core::profiles::compile_tools(profile, None)
            .unwrap()
            .unwrap();
        let before = generation(
            tools.clone(),
            vec![captured_state(&target, "https://example.com/api/v1")],
            None,
            &target,
            None,
        )
        .unwrap();
        let after = generation(
            tools,
            vec![captured_state(&target, "https://example.com/api/v2")],
            None,
            &target,
            None,
        )
        .unwrap();

        assert_eq!(before.endpoint_routes, ["/api/v1"]);
        assert_eq!(after.endpoint_routes, ["/api/v2"]);
        assert_ne!(before.fingerprint, after.fingerprint);
    }
}
