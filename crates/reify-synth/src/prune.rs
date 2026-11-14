//! Deterministic lexical relevance and response-quality ranking.
use crate::{
    cluster::{EndpointCluster, cluster_key},
    words::{singular, stem, stems, tokens},
};
use regex::Regex;
use serde_json::Value;
use std::{collections::BTreeSet, sync::OnceLock};

const ACTIONS: &[&str] = &[
    "search", "get", "list", "book", "create", "update", "delete", "cancel",
];
fn keywords(goal: &str) -> BTreeSet<String> {
    stems(goal)
        .into_iter()
        .filter(|t| {
            ![
                "a", "an", "the", "and", "or", "to", "for", "of", "in", "on", "with", "my", "me",
                "please", "want", "i",
            ]
            .contains(&t.as_str())
        })
        .collect()
}
fn fields<'a>(value: &'a Value, output: &mut BTreeSet<&'a str>, depth: usize) {
    if depth >= 64 {
        return;
    }
    match value {
        Value::Object(o) => {
            for (key, value) in o {
                output.insert(key);
                fields(value, output, depth + 1);
            }
        }
        Value::Array(a) => {
            for value in a {
                fields(value, output, depth + 1);
            }
        }
        _ => {}
    }
}
/// An endpoint's score, topical relevance, and exclusion decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointScore {
    /// Larger values rank first.
    pub score: i64,
    /// Matches a topical goal token, or any token for an action-only goal.
    pub relevant: bool,
    /// Operational noise should never become an agent tool.
    pub noise: bool,
}
/// Score path/query/body/response field names without reading scalar payload values.
pub fn score(cluster: &EndpointCluster<'_>, goal: Option<&str>) -> EndpointScore {
    let mut vocabulary = cluster.path_template.clone();
    for name in cluster.query_parameters.keys() {
        vocabulary.push(' ');
        vocabulary.push_str(name);
    }
    let mut names = BTreeSet::new();
    for frame in &cluster.frames {
        if let Some(body) = &frame.request_body {
            fields(body, &mut names, 0);
        }
        if let Some(body) = &frame.response_body {
            fields(body, &mut names, 0);
        }
    }
    for name in names {
        vocabulary.push(' ');
        vocabulary.push_str(name);
    }
    if let Ok(operation) = serde_json::from_str::<Value>(&cluster.operation) {
        if let Some(name) = operation["operationName"].as_str() {
            vocabulary.push(' ');
            vocabulary.push_str(name);
        }
        if let Some(query) = operation["query"].as_str() {
            // Only document identifiers matter; discard comments and string literals
            // so user-provided scalar text cannot steer relevance.
            static LITERALS: OnceLock<Regex> = OnceLock::new();
            let literals = LITERALS.get_or_init(|| {
                Regex::new(r#""""(?s:.*?)"""|"(?:\\.|[^"\\])*"|#[^\r\n]*"#)
                    .expect("static GraphQL lexical pattern")
            });
            vocabulary.push(' ');
            vocabulary.push_str(&literals.replace_all(query, " "));
        }
    }
    let observed = stems(&vocabulary);
    let path = stems(&cluster.path_template);
    let noise = [
        "config",
        "configuration",
        "bootstrap",
        "telemetry",
        "heartbeat",
        "health",
        "healthcheck",
        "analytics",
        "tracking",
        "metric",
        "ping",
    ]
    .iter()
    .any(|word| path.contains(*word));
    let goal = keywords(goal.unwrap_or(""));
    let topical: BTreeSet<_> = goal
        .iter()
        .filter(|t| !ACTIONS.contains(&t.as_str()))
        .cloned()
        .collect();
    let overlap = goal.intersection(&observed).count() as i64;
    let topical_overlap = topical.intersection(&observed).count() as i64;
    let rich = cluster.frames.iter().any(|f| {
        (200..300).contains(&f.status_code)
            && f.response_body
                .as_ref()
                .is_some_and(|b| b.is_array() || b.is_object())
    });
    EndpointScore {
        score: overlap * 100 + topical_overlap * 50 + if rich { 30 } else { 0 }
            - if noise { 1000 } else { 0 },
        relevant: goal.is_empty()
            || if topical.is_empty() {
                overlap > 0
            } else {
                topical_overlap > 0
            },
        noise,
    }
}
/// Return at most five relevant non-noise endpoints, ties resolved by canonical key.
/// Fewer than three are returned when the capture does not support three tools.
pub fn select<'a>(
    clusters: &'a [EndpointCluster<'a>],
    goal: Option<&str>,
) -> Vec<&'a EndpointCluster<'a>> {
    let mut scored: Vec<_> = clusters
        .iter()
        .map(|c| (c, score(c, goal), cluster_key(c)))
        .filter(|(_, s, _)| s.relevant && !s.noise)
        .collect();
    scored.sort_by(|(_, a, ka), (_, b, kb)| b.score.cmp(&a.score).then_with(|| ka.cmp(kb)));
    scored.into_iter().take(5).map(|(c, _, _)| c).collect()
}
/// Build a readable snake_case operation name from method and static resource path.
pub fn tool_name(cluster: &EndpointCluster<'_>) -> String {
    if let Ok(operation) = serde_json::from_str::<Value>(&cluster.operation)
        && let Some(name) = operation["operationName"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
    {
        let mut name = crate::words::snake(name);
        name.truncate(96);
        return name.trim_end_matches('_').to_owned();
    }
    let mut parts: Vec<String> = cluster
        .path_template
        .split('/')
        .filter(|s| !s.is_empty() && !s.starts_with('{'))
        .flat_map(tokens)
        .filter(|s| {
            s != "api" && !(s.starts_with('v') && s[1..].bytes().all(|c| c.is_ascii_digit()))
        })
        .collect();
    let explicit = parts
        .iter()
        .position(|p| ACTIONS.contains(&stem(p).as_str()));
    let action = explicit.map(|i| stem(&parts.remove(i))).unwrap_or_else(|| {
        match cluster.method.as_str() {
            "GET" | "HEAD" => "get",
            "DELETE" => "delete",
            "PUT" | "PATCH" => "update",
            _ => "create",
        }
        .into()
    });
    if parts.is_empty() {
        parts.push("endpoint".into());
    }
    let n = parts.len();
    for p in &mut parts[..n - 1] {
        *p = singular(p);
    }
    let mut name = format!("{action}_{}", parts.join("_"));
    name.truncate(96);
    name.trim_end_matches('_').to_owned()
}
/// Describe captured behavior and response shape without exposing observed values.
pub fn description(cluster: &EndpointCluster<'_>) -> String {
    let mut shapes = BTreeSet::new();
    for frame in &cluster.frames {
        if let Some(body) = &frame.response_body {
            shapes.insert(match body {
                Value::Object(o) => format!(
                    "a JSON object with fields {}",
                    o.keys().take(8).cloned().collect::<Vec<_>>().join(", ")
                ),
                Value::Array(_) => "a JSON array".into(),
                Value::Null => "JSON null".into(),
                _ => "a JSON scalar".into(),
            });
        }
    }
    let returns = if shapes.is_empty() {
        "the upstream response (body shape was not captured)".into()
    } else {
        shapes.into_iter().collect::<Vec<_>>().join(" or ")
    };
    let mut operation = tool_name(cluster).replace('_', " ");
    if let Some(initial) = operation.get_mut(..1) {
        initial.make_ascii_uppercase();
    }
    format!(
        "{operation} using {} {}. Returns {}. Inputs are inferred from observed requests; optional inputs may be omitted.",
        cluster.method, cluster.path_template, returns
    )
}
