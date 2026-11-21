//! Embedded, network-free API plans for familiar public services.
use crate::{
    ReifyError,
    domain::{CompiledTool, ExecutionSpec, Target, normalize_goal},
    mcp::ToolDefinition,
};
use serde_json::{Map, Value, json};
use std::{collections::HashMap, sync::OnceLock};
use url::Url;

/// One typed input exposed by an embedded endpoint plan.
#[derive(Clone, Copy, Debug)]
pub struct PrewarmedParameter {
    /// MCP argument name.
    pub name: &'static str,
    /// JSON Schema primitive type.
    pub kind: &'static str,
    /// Human-facing explanation.
    pub description: &'static str,
    /// Whether callers must supply the argument.
    pub required: bool,
}

/// One immutable request plan in a pre-warmed profile.
#[derive(Clone, Copy, Debug)]
pub struct PrewarmedTool {
    /// Stable MCP tool name.
    pub name: &'static str,
    /// Description shown to agents.
    pub description: &'static str,
    /// HTTP method.
    pub method: &'static str,
    /// Absolute HTTP(S) endpoint, with `{argument}` path placeholders.
    pub endpoint: &'static str,
    /// Typed arguments inserted into path segments.
    pub path_parameters: &'static [&'static str],
    /// Typed arguments encoded as query parameters, argument name to wire name.
    pub query_parameters: &'static [(&'static str, &'static str)],
    /// Inputs selected by goal text.
    pub keywords: &'static [&'static str],
    /// Tool inputs in displayed signature order.
    pub parameters: &'static [PrewarmedParameter],
    /// Optional JSON body template. Exact `{name}` strings read typed arguments.
    pub body_template: Option<&'static str>,
    /// Defaults for optional path arguments.
    pub path_defaults: &'static [(&'static str, &'static str)],
    /// Credential keys required for the API to accept a request.
    pub required_auth_keys: &'static [&'static str],
}

/// Domain-owned collection of precomputed typed API plans.
#[derive(Clone, Copy, Debug)]
pub struct PrewarmedProfile {
    /// Primary domain name.
    pub domain: &'static str,
    /// Additional domain families or explicit `*.` subdomain patterns.
    pub aliases: &'static [&'static str],
    /// Human-facing profile name.
    pub name: &'static str,
    /// Description of the API collection.
    pub description: &'static str,
    /// Candidate endpoint plans.
    pub tools: &'static [PrewarmedTool],
}

mod catalog;
pub use catalog::PROFILES;

/// Match a built-in profile by domain family at DNS label boundaries.
/// A `*.` alias matches subdomains only; other domains include their subdomains.
/// Goal text selects the highest-scoring useful tool(s); unrelated goals fall
/// through to live discovery instead of mounting a mismatched catalog.
pub fn lookup(url: &Url, goal: Option<&str>) -> Option<&'static PrewarmedProfile> {
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    PROFILES.iter().find(|profile| {
        let domain_match = host_matches(&host, profile.domain)
            || profile
                .aliases
                .iter()
                .any(|alias| host_matches(&host, alias));
        domain_match && selected_tools(profile, goal).is_some()
    })
}

/// Resolve one profile tool set, filtering to the strongest goal keyword match.
pub fn selected_tools<'a>(
    profile: &'a PrewarmedProfile,
    goal: Option<&str>,
) -> Option<Vec<&'a PrewarmedTool>> {
    if profile.tools.is_empty() {
        return None;
    }
    let Some(goal) = goal.filter(|goal| !goal.trim().is_empty()) else {
        return Some(profile.tools.iter().collect());
    };
    let goal = normalize_goal(goal);
    let tokens: Vec<_> = goal
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| token.len() > 1)
        .collect();
    if tokens.iter().any(|token| {
        matches!(
            *token,
            "delete" | "create" | "update" | "edit" | "remove" | "checkout" | "purchase"
        )
    }) {
        return None;
    }
    if tokens.is_empty() {
        return Some(profile.tools.iter().collect());
    }
    let scored: Vec<_> = profile
        .tools
        .iter()
        .map(|tool| {
            let score = tool
                .keywords
                .iter()
                .filter(|keyword| {
                    let keyword = keyword.to_ascii_lowercase();
                    tokens.iter().any(|token| {
                        keyword == *token
                            || keyword
                                .strip_suffix('s')
                                .is_some_and(|singular| singular == *token)
                            || token
                                .strip_suffix('s')
                                .is_some_and(|singular| singular == keyword)
                    })
                })
                .count();
            (tool, score)
        })
        .collect();
    let best = scored.iter().map(|(_, score)| *score).max().unwrap_or(0);
    (best > 0).then(|| {
        scored
            .into_iter()
            .filter_map(|(tool, score)| (score == best).then_some(tool))
            .collect()
    })
}

/// Construct validated MCP tools from a profile's static request plans.
pub fn compile_tools(
    profile: &PrewarmedProfile,
    goal: Option<&str>,
) -> Result<Option<Vec<CompiledTool>>, ReifyError> {
    let Some(selected) = selected_tools(profile, goal) else {
        return Ok(None);
    };
    let compiled = compiled_profiles()?;
    let all = compiled
        .get(profile.name)
        .ok_or_else(|| ReifyError::SchemaValidationError("missing embedded profile".into()))?;
    selected
        .into_iter()
        .map(|selected| {
            all.iter()
                .find(|tool| tool.definition.name == selected.name)
                .cloned()
                .ok_or_else(|| {
                    ReifyError::SchemaValidationError("missing embedded endpoint plan".into())
                })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

static COMPILED_PROFILES: OnceLock<Result<HashMap<&'static str, Vec<CompiledTool>>, String>> =
    OnceLock::new();

/// Compile all immutable endpoint plans once during process startup.
pub fn warm() -> Result<(), ReifyError> {
    compiled_profiles().map(|_| ())
}

fn compiled_profiles() -> Result<&'static HashMap<&'static str, Vec<CompiledTool>>, ReifyError> {
    COMPILED_PROFILES
        .get_or_init(|| {
            PROFILES
                .iter()
                .map(|profile| {
                    let tools = profile
                        .tools
                        .iter()
                        .map(compile_tool)
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|error| error.to_string())?;
                    Ok((profile.name, tools))
                })
                .collect()
        })
        .as_ref()
        .map_err(|error| ReifyError::SchemaValidationError(error.clone()))
}

fn compile_tool(tool: &PrewarmedTool) -> Result<CompiledTool, ReifyError> {
    let endpoint = Url::parse(tool.endpoint)
        .map_err(|error| ReifyError::InvalidTargetUrl(error.to_string()))?;
    if !matches!(endpoint.scheme(), "http" | "https") || endpoint.host_str().is_none() {
        return Err(ReifyError::InvalidTargetUrl(
            "pre-warmed endpoint must use HTTP(S)".into(),
        ));
    }
    let mut properties = Map::new();
    let mut required = Vec::new();
    for parameter in tool.parameters {
        if !["string", "integer", "number", "boolean"].contains(&parameter.kind) {
            return Err(ReifyError::SchemaValidationError(
                "unsupported pre-warmed parameter type".into(),
            ));
        }
        properties.insert(
            parameter.name.into(),
            json!({"type":parameter.kind,"description":parameter.description}),
        );
        if parameter.required {
            required.push(Value::String(parameter.name.into()));
        }
    }
    let input_schema = json!({
        "$schema":"https://json-schema.org/draft/2020-12/schema",
        "type":"object",
        "properties":properties,
        "required":required,
        "additionalProperties":false
    });
    let body_template = tool
        .body_template
        .map(serde_json::from_str::<Value>)
        .transpose()?;
    let compiled = CompiledTool {
        definition: ToolDefinition {
            name: tool.name.into(),
            description: tool.description.into(),
            input_schema,
            metadata: Some(crate::mcp::ToolMetadata {
                tags: vec!["prewarmed".into(), tool.method.to_ascii_lowercase()],
                source_endpoint: Some(tool.endpoint.into()),
            }),
        },
        execution: ExecutionSpec {
            upstream_url_template: tool.endpoint.into(),
            http_method: tool.method.into(),
            query_param_mappings: tool
                .query_parameters
                .iter()
                .map(|(argument, wire)| ((*argument).into(), (*wire).into()))
                .collect::<HashMap<_, _>>(),
            path_parameter_defaults: tool
                .path_defaults
                .iter()
                .map(|(name, value)| ((*name).into(), (*value).into()))
                .collect(),
            body_template,
            required_auth_keys: tool
                .required_auth_keys
                .iter()
                .map(|key| (*key).into())
                .collect(),
        },
    };
    compiled.definition.validate()?;
    Ok(compiled)
}

fn host_matches(host: &str, pattern: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(suffix) => host
            .strip_suffix(suffix)
            .is_some_and(|prefix| prefix.ends_with('.') && !prefix[..prefix.len() - 1].is_empty()),
        None => {
            host == pattern
                || host
                    .strip_suffix(pattern)
                    .is_some_and(|prefix| prefix.ends_with('.'))
        }
    }
}

/// Resolve a target to its matching profile and already compiled relevant tools.
pub fn for_target(
    target: &Target,
) -> Result<Option<(&'static PrewarmedProfile, Vec<CompiledTool>)>, ReifyError> {
    let Some(profile) = lookup(&target.url, target.goal.as_deref()) else {
        return Ok(None);
    };
    let tools = compile_tools(profile, target.goal.as_deref())?
        .ok_or_else(|| ReifyError::SchemaValidationError("matched profile has no tools".into()))?;
    Ok(Some((profile, tools)))
}

/// Format all static signatures in declared parameter order.
pub fn signature(tool: &PrewarmedTool) -> String {
    let parameters = tool
        .parameters
        .iter()
        .map(|parameter| {
            format!(
                "{}{}: {}",
                parameter.name,
                if parameter.required { "" } else { "?" },
                parameter.kind
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{}({parameters})", tool.name)
}

/// Match a profile from an HTTP(S) URL string for lightweight callers.
pub fn lookup_str(url: &str, goal: Option<&str>) -> Option<&'static PrewarmedProfile> {
    Url::parse(url).ok().and_then(|url| lookup(&url, goal))
}
