//! Typed argument layout and a reference renderer for execution-plan interpolation.
use crate::{cluster::EndpointCluster, prune, schema, words::snake};
use reify_core::{
    ReifyError,
    domain::{AuthContext, CompiledTool, ExecutionSpec},
    mcp::{ToolDefinition, ToolMetadata},
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeSet, HashMap};
use url::Url;

fn invalid(message: impl Into<String>) -> ReifyError {
    ReifyError::SchemaValidationError(message.into())
}
fn auth_keys(auth: &AuthContext) -> Vec<String> {
    let mut keys = BTreeSet::new();
    if !auth.cookies.is_empty() {
        keys.insert("cookie".to_owned());
    }
    if auth.authorization_header.is_some() {
        keys.insert("authorization".to_owned());
    }
    keys.extend(
        auth.csrf_tokens
            .keys()
            .chain(auth.custom_headers.keys())
            .map(|s| s.to_ascii_lowercase()),
    );
    keys.into_iter().collect()
}
/// Compile one cluster. Path/query inputs are flat; the optional `body` input is
/// the complete typed JSON document. A whole-string `{body}` preserves JSON types.
pub fn compile(
    cluster: &EndpointCluster<'_>,
    auth: &AuthContext,
    name: String,
) -> Result<CompiledTool, ReifyError> {
    if cluster.frames.is_empty() {
        return Err(invalid("cannot compile an empty endpoint cluster"));
    }
    if !cluster
        .method
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        || cluster.method.is_empty()
    {
        return Err(invalid("invalid HTTP method"));
    }
    let mut properties = Map::new();
    let mut required = BTreeSet::new();
    let mut used: BTreeSet<String> = cluster.path_parameters.keys().cloned().collect();
    used.insert("body".into());
    for (key, values) in &cluster.path_parameters {
        properties.insert(key.clone(), schema::induce(values));
        required.insert(key.clone());
    }
    let mut query_param_mappings = HashMap::new();
    for (upstream, parameter) in &cluster.query_parameters {
        let base = snake(upstream);
        let mut argument = base.clone();
        let mut index = 2;
        if used.contains(&argument) {
            argument = format!("query_{base}");
        }
        while used.contains(&argument) {
            argument = format!("query_{base}_{index}");
            index += 1;
        }
        used.insert(argument.clone());
        properties.insert(argument.clone(), schema::induce(&parameter.samples));
        if parameter.required {
            required.insert(argument.clone());
        }
        query_param_mappings.insert(argument, upstream.clone());
    }
    let bodies: Vec<_> = cluster
        .frames
        .iter()
        .filter_map(|f| f.request_body.as_ref())
        .collect();
    let body_template = if bodies.is_empty() {
        None
    } else {
        properties.insert("body".into(), schema::induce_refs(&bodies));
        if bodies.len() == cluster.frames.len() {
            required.insert("body".into());
        }
        Some(json!("{body}"))
    };
    let mut input_schema = json!({"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","additionalProperties":false});
    input_schema["properties"] = Value::Object(properties);
    input_schema["required"] = Value::Array(required.into_iter().map(Value::String).collect());
    let definition = ToolDefinition {
        name,
        description: prune::description(cluster),
        input_schema,
        metadata: Some(ToolMetadata {
            tags: vec!["observed".into(), cluster.method.to_ascii_lowercase()],
            source_endpoint: Some(format!("{}{}", cluster.origin, cluster.path_template)),
        }),
    };
    definition.validate()?;
    Ok(CompiledTool {
        definition,
        execution: ExecutionSpec {
            upstream_url_template: format!("{}{}", cluster.origin, cluster.path_template),
            http_method: cluster.method.clone(),
            query_param_mappings,
            body_template,
            required_auth_keys: auth_keys(auth),
        },
    })
}
/// Rendered request data, with no network operation or credential material attached.
#[derive(Debug, PartialEq)]
pub struct RenderedRequest {
    /// Encoded URL, including query arguments.
    pub url: Url,
    /// Observed HTTP method.
    pub method: String,
    /// Typed JSON request body, if supplied.
    pub body: Option<Value>,
}
/// Render a compiled plan. Requires named JSON arguments, preserves repeated query
/// values and JSON body types, and percent-encodes path values as single segments.
/// Arguments are validated against the generated schema before interpolation.
/// Prepared immutable validation and query ordering for repeated invocations.
/// Rebuild this object when the tool definition changes; it never caches arguments.
pub struct PreparedTool {
    validator: jsonschema::Validator,
    properties: BTreeSet<String>,
    required: Vec<String>,
    template: String,
    method: String,
    queries: Vec<(String, String)>,
    body: BodyTemplate,
}
enum BodyTemplate {
    None,
    Argument,
    Unsupported,
}
/// Borrowed JSON body/method with an owned URL, ready for request serialization.
pub struct RenderedRequestRef<'a> {
    /// Percent-encoded destination and ordered query arguments.
    pub url: Url,
    /// Original HTTP method.
    pub method: &'a str,
    /// Complete typed body, borrowing the invocation's arguments.
    pub body: Option<&'a Value>,
}
impl PreparedTool {
    /// Compile schema validation once, retaining only execution configuration.
    pub fn new(tool: &CompiledTool) -> Result<Self, ReifyError> {
        let validator = jsonschema::validator_for(&tool.definition.input_schema)
            .map_err(|e| invalid(format!("invalid input schema: {e}")))?;
        let properties = tool.definition.input_schema["properties"]
            .as_object()
            .ok_or_else(|| invalid("missing input properties"))?
            .keys()
            .cloned()
            .collect();
        let required = tool.definition.input_schema["required"]
            .as_array()
            .map(|keys| {
                keys.iter()
                    .map(|key| {
                        key.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| invalid("invalid required argument name"))
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default();
        let mut queries: Vec<_> = tool
            .execution
            .query_param_mappings
            .iter()
            .map(|(a, b)| (a.clone(), b.clone()))
            .collect();
        queries.sort();
        let body = match &tool.execution.body_template {
            None => BodyTemplate::None,
            Some(Value::String(s)) if s == "{body}" => BodyTemplate::Argument,
            _ => BodyTemplate::Unsupported,
        };
        Ok(Self {
            validator,
            properties,
            required,
            template: tool.execution.upstream_url_template.clone(),
            method: tool.execution.http_method.clone(),
            queries,
            body,
        })
    }
    /// Validate and render without cloning the request body's JSON tree.
    pub fn render<'a>(
        &'a self,
        arguments: &'a Value,
    ) -> Result<RenderedRequestRef<'a>, ReifyError> {
        self.validator
            .validate(arguments)
            .map_err(|e| invalid(format!("invalid tool arguments: {e}")))?;
        let args = arguments
            .as_object()
            .ok_or_else(|| invalid("tool arguments must be an object"))?;
        for key in args.keys() {
            if !self.properties.contains(key) {
                return Err(invalid(format!("unknown argument: {key}")));
            }
        }
        for key in &self.required {
            if !args.contains_key(key) {
                return Err(invalid(format!("missing required argument: {key}")));
            }
        }
        let mut template = self.template.clone();
        let mut encoder = None;
        while let Some(start) = template.find('{') {
            let end = template[start..]
                .find('}')
                .map(|n| n + start)
                .ok_or_else(|| invalid("unclosed path argument"))?;
            let key = &template[start + 1..end];
            let value = args
                .get(key)
                .and_then(Value::as_str)
                .ok_or_else(|| invalid(format!("path argument {key} must be a string")))?;
            if value.is_empty() || matches!(value, "." | "..") {
                return Err(invalid("path argument cannot be empty or a dot segment"));
            }
            let encoder = encoder
                .get_or_insert_with(|| Url::parse("https://encode.invalid/").expect("literal URL"));
            encoder.set_path("/");
            encoder
                .path_segments_mut()
                .expect("hierarchical URL")
                .push(value);
            template.replace_range(start..=end, encoder.path().trim_start_matches('/'));
        }
        let mut url =
            Url::parse(&template).map_err(|e| invalid(format!("invalid rendered URL: {e}")))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(invalid("rendered URL must be HTTP(S)"));
        }
        let mut pairs = self
            .queries
            .iter()
            .filter_map(|(input, upstream)| args.get(input).map(|value| (input, upstream, value)))
            .flat_map(|(input, upstream, value)| {
                value
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_else(|| std::slice::from_ref(value))
                    .iter()
                    .map(move |value| (input, upstream, value))
            })
            .peekable();
        if pairs.peek().is_some() {
            let mut query = url.query_pairs_mut();
            for (input, upstream, value) in pairs {
                query.append_pair(
                    upstream,
                    value.as_str().ok_or_else(|| {
                        invalid(format!("query argument {input} must contain strings"))
                    })?,
                );
            }
        }
        let body = match self.body {
            BodyTemplate::None => None,
            BodyTemplate::Argument => args.get("body"),
            BodyTemplate::Unsupported => {
                return Err(invalid(
                    "unsupported body template; expected whole-document {body}",
                ));
            }
        };
        Ok(RenderedRequestRef {
            url,
            method: &self.method,
            body,
        })
    }
}
/// One-shot compatibility renderer. Repeated executors should retain PreparedTool.
pub fn render(tool: &CompiledTool, arguments: &Value) -> Result<RenderedRequest, ReifyError> {
    let prepared = PreparedTool::new(tool)?;
    let result = prepared.render(arguments)?;
    Ok(RenderedRequest {
        url: result.url,
        method: result.method.to_owned(),
        body: result.body.cloned(),
    })
}
