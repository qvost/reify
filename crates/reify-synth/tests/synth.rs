use reify_core::domain::{AuthContext, CapturedFrame, ResourceType, Target};
use reify_synth::{
    SynthEngine,
    cluster::{cluster_frames, parameterize_path},
    compiler::{compile, render},
    prune, schema,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use url::Url;

fn frame(url: &str, method: &str, body: Option<Value>, response: Value) -> CapturedFrame {
    CapturedFrame {
        id: uuid::Uuid::new_v4(),
        url: Url::parse(url).unwrap(),
        method: method.into(),
        request_headers: HashMap::new(),
        request_body: body,
        status_code: 200,
        response_headers: HashMap::from([("content-type".into(), "application/json".into())]),
        response_body: Some(response),
        timestamp: chrono::Utc::now(),
        resource_type: ResourceType::Fetch,
    }
}
fn assert_accepts(schema: &Value, samples: &[Value]) {
    jsonschema::meta::validate(schema).unwrap();
    let validator = jsonschema::options()
        .should_validate_formats(true)
        .build(schema)
        .unwrap();
    for sample in samples {
        assert!(
            validator.is_valid(sample),
            "schema {schema} rejected sample {sample}"
        );
    }
}
#[test]
fn test_json_schema_induction() {
    let samples = vec![
        json!({"name":"A","active":true,"score":1,"nullable":null,"items":[1,"x"],"profile":{"age":30,"nickname":"a"}}),
        json!({"name":"B","active":false,"score":1.25,"items":[],"profile":{"age":31}}),
    ];
    let schema = schema::induce(&samples);
    assert_accepts(&schema, &samples);
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["properties"]["score"]["type"], "number");
    assert_eq!(schema["properties"]["nullable"]["type"], "null");
    assert!(
        !schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("nullable"))
    );
    assert_eq!(schema["properties"]["profile"]["required"], json!(["age"]));
    assert_eq!(
        schema["properties"]["items"]["items"]["anyOf"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let mut reversed = samples.clone();
    reversed.reverse();
    assert_eq!(schema, schema::induce(&reversed));
}
#[test]
fn schema_joins_accept_every_observed_type_and_are_order_independent() {
    let values = vec![
        Value::Null,
        json!(true),
        json!(1),
        json!(1.0),
        json!(1.25),
        json!(u64::MAX),
        json!("text"),
        json!([]),
        json!([null, {}, 1]),
        json!({"x":null}),
        json!({"x":{"a":true}}),
    ];
    for a in &values {
        for b in &values {
            let samples = vec![a.clone(), b.clone()];
            let inferred = schema::induce(&samples);
            assert_accepts(&inferred, &samples);
            assert_eq!(inferred, schema::induce(&[b.clone(), a.clone()]));
        }
    }
    assert_eq!(schema::induce(&[]), json!({}));
    assert_eq!(
        schema::induce(&[json!([])]),
        json!({"type":"array","items":{}})
    );
    assert_eq!(schema::induce(&[json!(1), json!(2.0)])["type"], "integer");
    let nullable = vec![json!(null), json!({"a":1}), json!({"b":true})];
    assert_accepts(&schema::induce(&nullable), &nullable);
}
#[test]
fn formats_require_unanimous_evidence() {
    for (sample, format) in [
        ("2026-11-14", "date"),
        ("2026-11-14T12:30:00Z", "date-time"),
        ("550e8400-e29b-41d4-a716-446655440000", "uuid"),
        ("https://example.com/x", "uri"),
        ("a.b+tag@example.com", "email"),
    ] {
        let samples = vec![json!(sample), json!(sample)];
        let inferred = schema::induce(&samples);
        assert_eq!(inferred["format"], format);
        assert_accepts(&inferred, &samples);
        assert!(
            schema::induce(&[json!(sample), json!("ordinary")])
                .get("format")
                .is_none()
        );
    }
    for invalid in ["2026-02-30", "a..b@example.com", "a@-example.com"] {
        assert!(schema::induce(&[json!(invalid)]).get("format").is_none());
    }
}
#[test]
fn test_path_parameter_clustering() {
    let frames = vec![
        frame(
            "https://example.com/api/users/123/profile?expand=all&limit=10",
            "GET",
            None,
            json!({}),
        ),
        frame(
            "https://example.com/api/users/456/profile?expand=none",
            "GET",
            None,
            json!({}),
        ),
    ];
    let clusters = cluster_frames(&frames);
    assert_eq!(clusters.len(), 1);
    let cluster = &clusters[0];
    assert_eq!(cluster.path_template, "/api/users/{user_id}/profile");
    assert_eq!(
        cluster.path_parameters["user_id"],
        vec![json!("123"), json!("456")]
    );
    assert!(cluster.query_parameters["expand"].required);
    assert!(!cluster.query_parameters["limit"].required);
    let (path, params) = parameterize_path(
        "/api/v1/users/550e8400-e29b-41d4-a716-446655440000/files/0123456789abcdef/on/2026-11-14",
    );
    assert_eq!(path, "/api/v1/users/{user_id}/files/{file_id}/on/{on_date}");
    assert_eq!(params.len(), 3);
    assert_eq!(
        parameterize_path("/users/1/users/2/").0,
        "/users/{user_id}/users/{user_id_2}/"
    );
}
#[test]
fn clustering_preserves_origins_methods_trailing_slashes_and_repeated_queries() {
    let frames = vec![
        frame(
            "https://example.com/x?tag=one&tag=two",
            "GET",
            None,
            json!({}),
        ),
        frame("https://example.com/x?tag=one", "GET", None, json!({})),
        frame("https://example.com/x/", "GET", None, json!({})),
        frame("https://example.com:8443/x", "GET", None, json!({})),
        frame("https://example.com/x", "POST", None, json!({})),
        frame("https://other.org/x", "GET", None, json!({})),
    ];
    let clusters = cluster_frames(&frames);
    assert_eq!(clusters.len(), 5);
    let c = clusters.iter().find(|c| c.frames.len() == 2).unwrap();
    assert_eq!(
        c.query_parameters["tag"].samples,
        vec![json!("one"), json!(["one", "two"])]
    );
    assert_accepts(
        &schema::induce(&c.query_parameters["tag"].samples),
        &c.query_parameters["tag"].samples,
    );
}
#[test]
fn test_goal_directed_pruning() {
    let mut frames = Vec::new();
    for path in [
        "flights/search",
        "flights/fares",
        "flights/seats",
        "flights/availability",
        "flights/rules",
        "telemetry/flights",
        "heartbeat",
        "config/flights",
        "bootstrap",
        "analytics",
        "hotels/search",
        "hotels/book",
        "users/profile",
        "cars/search",
        "weather/current",
    ] {
        frames.push(frame(
            &format!("https://example.com/api/{path}"),
            "GET",
            None,
            json!({"data":[{"id":1}]}),
        ));
    }
    assert_eq!(frames.len(), 15);
    let target = Target::new("https://example.com", Some("search flights".into())).unwrap();
    let output = SynthEngine::new()
        .synthesize(&target, &frames, &AuthContext::default())
        .unwrap();
    assert!((3..=5).contains(&output.tools.len()));
    assert_eq!(output.tools.len(), 5);
    assert_eq!(output.primary_tool_name, "search_flights");
    for tool in &output.tools {
        assert!(tool.definition.name.contains("flight"));
        assert!(
            tool.definition
                .name
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        );
        tool.definition.validate().unwrap();
        assert!(!tool.definition.description.is_empty());
    }
    frames.reverse();
    let reversed = SynthEngine::new()
        .synthesize(&target, &frames, &AuthContext::default())
        .unwrap();
    assert_eq!(
        serde_json::to_value(output.tools).unwrap(),
        serde_json::to_value(reversed.tools).unwrap()
    );
}
#[test]
fn test_execution_spec_interpolation() {
    let frames = vec![
        frame(
            "https://example.com/api/users/123/profile?user_id=001&tag=a&tag=b&optional=yes",
            "POST",
            Some(json!({"count":1,"nested":{"active":true},"list":[1,2]})),
            json!({}),
        ),
        frame(
            "https://example.com/api/users/456/profile?user_id=002&tag=c",
            "POST",
            Some(json!({"count":2,"nested":{"active":false}})),
            json!({}),
        ),
    ];
    let auth = AuthContext {
        cookies: HashMap::from([("session".into(), "secret".into())]),
        authorization_header: Some("Bearer hidden".into()),
        csrf_tokens: HashMap::from([("X-CSRF-Token".into(), "hidden".into())]),
        ..Default::default()
    };
    let clusters = cluster_frames(&frames);
    let tool = compile(&clusters[0], &auth, "update_user_profile".into()).unwrap();
    assert_eq!(
        tool.execution.upstream_url_template,
        "https://example.com/api/users/{user_id}/profile"
    );
    assert_eq!(
        tool.execution.query_param_mappings["query_user_id"],
        "user_id"
    );
    assert_eq!(tool.execution.body_template, Some(json!("{body}")));
    assert_eq!(
        tool.execution.required_auth_keys,
        vec!["authorization", "cookie", "x-csrf-token"]
    );
    let body = json!({"count":3,"nested":{"active":true},"list":[4,5]});
    let args = json!({"user_id":"789/a?b#c","query_user_id":"001","tag":["x y","a&b"],"body":body});
    let rendered = render(&tool, &args).unwrap();
    assert_eq!(rendered.url.path(), "/api/users/789%2Fa%3Fb%23c/profile");
    assert_eq!(
        rendered
            .url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect::<Vec<_>>(),
        vec![
            ("user_id".into(), "001".into()),
            ("tag".into(), "x y".into()),
            ("tag".into(), "a&b".into())
        ]
    );
    assert_eq!(rendered.body, Some(body));
    assert_eq!(rendered.method, "POST");
    assert!(!serde_json::to_string(&tool).unwrap().contains("hidden"));
    assert!(!serde_json::to_string(&tool).unwrap().contains("secret"));
    for bad in [
        json!({}),
        json!({"user_id":"..","query_user_id":"1","tag":"a","body":{"count":1,"nested":{"active":true}}}),
        json!({"user_id":"1","query_user_id":1,"tag":"a","body":{}}),
    ] {
        assert!(render(&tool, &bad).is_err());
    }
}
#[test]
fn sparse_capture_optional_body_and_empty_goals() {
    let frames = vec![
        frame(
            "https://example.com/api/items?x=1",
            "POST",
            Some(json!({"a":null})),
            json!([]),
        ),
        frame("https://example.com/api/items", "POST", None, json!([])),
    ];
    let target = Target::new("https://example.com", None).unwrap();
    let output = SynthEngine::new()
        .synthesize(&target, &frames, &AuthContext::default())
        .unwrap();
    assert_eq!(output.tools.len(), 1);
    assert_eq!(render(&output.tools[0], &json!({})).unwrap().body, None);
    assert_eq!(
        render(&output.tools[0], &json!({"body":{"a":null}}))
            .unwrap()
            .body,
        Some(json!({"a":null}))
    );
    assert!(
        SynthEngine::new()
            .synthesize(&target, &[], &AuthContext::default())
            .is_err()
    );
    let target = Target::new("https://example.com", Some("search galaxies".into())).unwrap();
    assert!(
        SynthEngine::new()
            .synthesize(&target, &frames, &AuthContext::default())
            .is_err()
    );
}
#[test]
fn query_name_collisions_and_graphql_operations_are_deterministic() {
    let frames = vec![
        frame(
            "https://example.com/graphql?body=x&a-b=1&a_b=2",
            "POST",
            Some(
                json!({"operationName":"SearchFlights","query":"query SearchFlights { flights { id } }","variables":{"from":"KTM"}}),
            ),
            json!({"flights":[]}),
        ),
        frame(
            "https://example.com/graphql",
            "POST",
            Some(
                json!({"operationName":"BookFlight","query":"mutation BookFlight { bookFlight { id } }"}),
            ),
            json!({"bookFlight":{}}),
        ),
    ];
    let clusters = cluster_frames(&frames);
    assert_eq!(clusters.len(), 2);
    let c = clusters
        .iter()
        .find(|c| !c.query_parameters.is_empty())
        .unwrap();
    let tool = compile(c, &AuthContext::default(), prune::tool_name(c)).unwrap();
    assert_eq!(tool.execution.query_param_mappings.len(), 3);
    assert_eq!(tool.execution.query_param_mappings["query_body"], "body");
    assert_ne!(
        tool.execution.query_param_mappings.get("a_b"),
        tool.execution.query_param_mappings.get("query_a_b")
    );
}
#[test]
fn unrelated_domains_and_non_json_traffic_do_not_compile() {
    let mut bad = frame(
        "https://evil.org/flights/search",
        "GET",
        None,
        json!({"flights":[]}),
    );
    let target = Target::new("https://example.com", None).unwrap();
    assert!(
        SynthEngine::new()
            .synthesize(&target, &[bad.clone()], &AuthContext::default())
            .is_err()
    );
    bad.url = Url::parse("https://example.com/index.html").unwrap();
    bad.response_body = None;
    bad.response_headers.clear();
    assert!(
        SynthEngine::new()
            .synthesize(&target, &[bad], &AuthContext::default())
            .is_err()
    );
}

#[test]
fn graphql_identifiers_drive_relevance_without_literal_text() {
    let frames = vec![
        frame(
            "https://example.com/graphql",
            "POST",
            Some(json!({"query":"query SearchFlights { flights { id } }"})),
            json!({}),
        ),
        frame(
            "https://example.com/graphql",
            "POST",
            Some(
                json!({"query":"query HotelSearch { hotels(search: \"flights\") { id } } # flights"}),
            ),
            json!({}),
        ),
    ];
    let target = Target::new("https://example.com", Some("search flights".into())).unwrap();
    let output = SynthEngine::new()
        .synthesize(&target, &frames, &AuthContext::default())
        .unwrap();
    assert_eq!(output.tools.len(), 1);
    let clusters = cluster_frames(&frames);
    for cluster in &clusters {
        if cluster.operation.contains("HotelSearch") {
            assert!(!prune::score(cluster, Some("search flights")).relevant);
        }
    }
}
#[test]
fn methods_case_and_percent_encoded_path_arguments_are_preserved() {
    let frames = vec![
        frame("https://example.com/items/-123", "get", None, json!({})),
        frame("https://example.com/items/123", "GET", None, json!({})),
    ];
    let clusters = cluster_frames(&frames);
    assert_eq!(clusters.len(), 2);
    assert_eq!(parameterize_path("/items/-123").0, "/items/{item_id}");
    let cluster = clusters.iter().find(|c| c.method == "get").unwrap();
    let tool = compile(cluster, &AuthContext::default(), "get_item".into()).unwrap();
    assert_eq!(tool.execution.http_method, "get");
    let request = render(&tool, &json!({"item_id":"%2e%2e/{x}"})).unwrap();
    assert_eq!(request.url.path(), "/items/%252e%252e%2F%7Bx%7D");
}
