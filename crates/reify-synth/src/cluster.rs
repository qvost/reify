//! Borrowed endpoint groups keyed by origin, method, path template, and GraphQL identity.
use crate::words::{singular, snake};
use chrono::NaiveDate;
use reify_core::domain::CapturedFrame;
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

/// An observed query argument; repeated keys are arrays, singleton keys strings.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryParameter {
    /// Present in every exchange in this group.
    pub required: bool,
    /// Values from exchanges where the key was present, in canonical order.
    pub samples: Vec<Value>,
}
/// One method/endpoint family. Captured frames are borrowed, not copied.
#[derive(Debug)]
pub struct EndpointCluster<'a> {
    /// Credential-free origin, including a nondefault port.
    pub origin: String,
    /// Case-sensitive observed HTTP method.
    pub method: String,
    /// URL path with named dynamic segments.
    pub path_template: String,
    /// GraphQL operation/query identity, empty for ordinary REST.
    pub operation: String,
    /// Canonically ordered exchanges.
    pub frames: Vec<&'a CapturedFrame>,
    /// Named path variables and their observed string values.
    pub path_parameters: BTreeMap<String, Vec<Value>>,
    /// Unified decoded query arguments with presence statistics.
    pub query_parameters: BTreeMap<String, QueryParameter>,
}
/// Generate a stable key suitable for score tie breaking and deterministic naming.
pub fn cluster_key(cluster: &EndpointCluster<'_>) -> String {
    format!(
        "{} {}{} {}",
        cluster.method, cluster.origin, cluster.path_template, cluster.operation
    )
}
fn dynamic(segment: &str) -> Option<&'static str> {
    if segment.len() == 10 && NaiveDate::parse_from_str(segment, "%Y-%m-%d").is_ok() {
        Some("date")
    } else if (!segment.is_empty()
        && segment
            .strip_prefix('-')
            .unwrap_or(segment)
            .bytes()
            .all(|c| c.is_ascii_digit())
        && segment != "-")
        || Uuid::parse_str(segment).is_ok()
        || (segment.len() >= 16 && segment.bytes().all(|c| c.is_ascii_hexdigit()))
    {
        Some("id")
    } else {
        None
    }
}
/// Replace ID/date segments, deriving names from the preceding static resource.
/// Version labels like `v1` and percent-encoded static segments remain unchanged.
pub fn parameterize_path(path: &str) -> (String, BTreeMap<String, Value>) {
    let mut params = BTreeMap::new();
    let mut segments = Vec::new();
    let mut resource = String::new();
    for segment in path.split('/') {
        if let Some(kind) = dynamic(segment) {
            let base = if resource.is_empty() {
                kind.to_owned()
            } else {
                format!("{}_{}", singular(&snake(&resource)), kind)
            };
            let mut name = base.clone();
            let mut index = 2;
            while params.contains_key(&name) {
                name = format!("{base}_{index}");
                index += 1;
            }
            params.insert(name.clone(), Value::String(segment.to_owned()));
            segments.push(format!("{{{name}}}"));
        } else {
            segments.push(segment.to_owned());
            if !segment.is_empty() {
                resource = segment.to_owned();
            }
        }
    }
    (segments.join("/"), params)
}
fn operation(frame: &CapturedFrame) -> String {
    if !frame.is_graphql() {
        return String::new();
    }
    let body = frame.request_body.as_ref();
    let query: BTreeMap<_, _> = frame
        .url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    // Keep different query documents separate even when their operationName matches.
    serde_json::json!({"operationName":body.and_then(|b| b.get("operationName")).cloned().or_else(|| query.get("operationName").map(|v| Value::String(v.clone()))),"query":body.and_then(|b| b.get("query")).cloned().or_else(|| query.get("query").map(|v| Value::String(v.clone()))),"persistedQuery":body.and_then(|b| b.pointer("/extensions/persistedQuery")).cloned()}).to_string()
}
/// Group HTTP(S) exchanges. Ports/methods/origins and trailing slash semantics stay
/// separate. Frame ordering and resulting schemas are independent of input order.
pub fn cluster_frames(frames: &[CapturedFrame]) -> Vec<EndpointCluster<'_>> {
    cluster_refs(frames.iter())
}
/// Group a filtered iterator without copying captured payloads.
pub fn cluster_refs<'a>(
    frames: impl IntoIterator<Item = &'a CapturedFrame>,
) -> Vec<EndpointCluster<'a>> {
    let mut groups: BTreeMap<(String, String, String, String), Vec<&CapturedFrame>> =
        BTreeMap::new();
    for frame in frames {
        if !matches!(frame.url.scheme(), "http" | "https") || frame.url.host_str().is_none() {
            continue;
        }
        let (path, _) = parameterize_path(frame.url.path());
        groups
            .entry((
                frame.url.origin().ascii_serialization(),
                frame.method.clone(),
                path,
                operation(frame),
            ))
            .or_default()
            .push(frame);
    }
    groups
        .into_iter()
        .map(|((origin, method, path_template, operation), mut frames)| {
            frames.sort_by_cached_key(|f| {
                (
                    f.url.as_str(),
                    f.request_body
                        .as_ref()
                        .map(Value::to_string)
                        .unwrap_or_default(),
                    f.status_code.to_string(),
                    f.response_body
                        .as_ref()
                        .map(Value::to_string)
                        .unwrap_or_default(),
                )
            });
            let mut path_parameters: BTreeMap<String, Vec<Value>> = BTreeMap::new();
            let mut queries: BTreeMap<String, Vec<Value>> = BTreeMap::new();
            for frame in &frames {
                for (name, value) in parameterize_path(frame.url.path()).1 {
                    path_parameters.entry(name).or_default().push(value);
                }
                let mut values: BTreeMap<String, Vec<String>> = BTreeMap::new();
                for (k, v) in frame.url.query_pairs() {
                    values
                        .entry(k.into_owned())
                        .or_default()
                        .push(v.into_owned());
                }
                for (key, mut values) in values {
                    let value = if values.len() == 1 {
                        Value::String(values.pop().expect("one query value"))
                    } else {
                        Value::Array(values.into_iter().map(Value::String).collect())
                    };
                    queries.entry(key).or_default().push(value);
                }
            }
            let query_parameters = queries
                .into_iter()
                .map(|(key, samples)| {
                    (
                        key,
                        QueryParameter {
                            required: samples.len() == frames.len(),
                            samples,
                        },
                    )
                })
                .collect();
            EndpointCluster {
                origin,
                method,
                path_template,
                operation,
                frames,
                path_parameters,
                query_parameters,
            }
        })
        .collect()
}
