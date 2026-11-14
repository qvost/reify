use chrono::{TimeZone, Utc};
use http::{HeaderMap, HeaderValue};
use reify_core::{ReifyError, cache::CacheKey, domain::*, mcp::*};
use serde_json::{Value, json};
use std::{borrow::Cow, collections::HashMap};
use url::Url;
use uuid::Uuid;

fn target(url: &str, goal: Option<&str>) -> Target {
    Target::new(url, goal.map(str::to_owned)).unwrap()
}
fn request(method: &str, name: Option<&str>) -> McpRequest {
    let mut params =
        json!({"_meta": {"io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION}});
    if let Some(name) = name {
        params["name"] = json!(name);
        params["arguments"] = json!({"city":"Kathmandu"});
    }
    McpRequest::new(1_i64.into(), method, Some(params)).unwrap()
}
fn headers(method: &str, name: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "MCP-Protocol-Version",
        HeaderValue::from_static(PROTOCOL_VERSION),
    );
    headers.insert("Mcp-Method", HeaderValue::from_str(method).unwrap());
    if let Some(name) = name {
        headers.insert("Mcp-Name", encode_header_value(name));
    }
    headers
}
fn frame() -> CapturedFrame {
    CapturedFrame {
        id: Uuid::new_v4(),
        url: Url::parse("https://example.com/api").unwrap(),
        method: "POST".into(),
        request_headers: HashMap::new(),
        request_body: None,
        status_code: 200,
        response_headers: HashMap::new(),
        response_body: None,
        timestamp: Utc.with_ymd_and_hms(2026, 7, 28, 0, 0, 0).unwrap(),
        resource_type: ResourceType::Fetch,
    }
}

#[test]
fn test_mcp_streamable_http_roundtrip() {
    for (method, name) in [("tools/list", None), ("tools/call", Some("weather"))] {
        let original = request(method, name);
        let mut map = headers(method, name);
        for (name, value) in [
            (MCP_STREAM, "false"),
            (MCP_CACHE_SCOPE, "domain"),
            (MCP_TTL_MS, "60000"),
            (MCP_SESSION_BRIDGE, "bridge-123"),
        ] {
            map.insert(name, HeaderValue::from_static(value));
        }
        let bytes = serde_json::to_vec(&original).unwrap();
        let decoded: McpRequest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(original, decoded);
        let parsed = McpHeaders::parse(&map).unwrap();
        parsed.validate_request(&decoded).unwrap();
        assert_eq!(parsed.stream, Some(false));
        assert_eq!(parsed.cache_scope, Some(CacheScope::Domain));
        assert_eq!(parsed.ttl_ms, Some(60000));
        assert_eq!(parsed.session_bridge, Some("bridge-123"));
        if let Some(name) = name {
            assert_eq!(decoded.extract_tool_call().unwrap().name, name);
        }
        let response =
            McpResponse::success(decoded.id, json!({"resultType":"complete", "tools":[]}));
        let value = serde_json::to_value(&response).unwrap();
        assert!(value.get("error").is_none());
        assert_eq!(
            serde_json::from_value::<McpResponse>(value).unwrap(),
            response
        );
    }
}

#[test]
fn test_target_normalization() {
    let t = target(
        "https://Example.COM:443/api/?#fragment",
        Some("  Find\tALL\n  Items  "),
    );
    assert_eq!(t.url.as_str(), "https://example.com/api");
    assert_eq!(t.goal.as_deref(), Some("find all items"));
    assert_eq!(
        target("https://example.com/", Some("\u{2003} \n")).goal,
        None
    );
    assert_eq!(
        target("https://example.com/api%2F/?q=a%2Fb#x", None)
            .url
            .as_str(),
        "https://example.com/api%2F?q=a%2Fb"
    );
    assert_eq!(
        target("http://EXAMPLE.com:80/", None).url.as_str(),
        "http://example.com/"
    );
    assert_eq!(
        target("https://example.com:8443/api///", None).url.as_str(),
        "https://example.com:8443/api"
    );
}
#[test]
fn invalid_targets_and_deserialization() {
    for url in [
        "",
        "/api",
        "file:///tmp/x",
        "ftp://example.com",
        "data:text/plain,x",
        "https://[broken",
    ] {
        assert!(matches!(
            Target::new(url, None),
            Err(ReifyError::InvalidTargetUrl(_))
        ));
        assert!(serde_json::from_value::<Target>(json!({"url":url})).is_err());
    }
    let decoded: Target =
        serde_json::from_value(json!({"url":"https://EXAMPLE.com/a/#x", "goal":" A  B "})).unwrap();
    assert_eq!(decoded, target("https://example.com/a", Some("a b")));
    assert_eq!(
        serde_json::from_value::<Target>(serde_json::to_value(&decoded).unwrap()).unwrap(),
        decoded
    );
}
#[test]
fn test_blake3_cache_key_determinism() {
    let key = CacheKey::from_target(&target(
        "https://EXAMPLE.com:443/api/",
        Some(" Find  ITEMS "),
    ));
    assert_eq!(
        key,
        CacheKey::from_target(&target("https://example.com/api", Some("find items")))
    );
    assert_eq!(
        key,
        CacheKey::from_target(&target(
            "http://example.com/another?x=1",
            Some("find items")
        ))
    );
    assert_ne!(
        key,
        CacheKey::from_target(&target("https://example.com", Some("find people")))
    );
    assert_ne!(
        key,
        CacheKey::from_target(&target("https://other.example", Some("find items")))
    );
    assert_ne!(
        key,
        CacheKey::from_target(&target("https://example.com:8443", Some("find items")))
    );
    assert_eq!(key.to_hex().len(), 64);
    assert_eq!(key.to_string(), key.to_hex());
    assert!(
        key.to_hex()
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert_eq!(std::mem::size_of::<CacheKey>(), 32);
    // Independent preimage construction pins the documented v1 hash format.
    let mut preimage = b"reify:target:v1\0".to_vec();
    preimage.extend_from_slice(&11_u64.to_le_bytes());
    preimage.extend_from_slice(b"example.com");
    preimage.extend_from_slice(&10_u64.to_le_bytes());
    preimage.extend_from_slice(b"find items");
    assert_eq!(key.as_bytes(), blake3::hash(&preimage).as_bytes());
    assert_eq!(
        CacheKey::from_target(&target("https://example.com", None)),
        CacheKey::from_target(&target("https://example.com/", Some("  ")))
    );
}
#[test]
fn cache_authorities_ipv6_idna_and_unambiguous_fields() {
    assert_eq!(
        CacheKey::from_target(&target("https://[::1]:443/", None)),
        CacheKey::from_target(&target("http://[::1]/", None))
    );
    assert_ne!(
        CacheKey::from_target(&target("https://[::1]:444/", None)),
        CacheKey::from_target(&target("https://[::1]/", None))
    );
    assert_eq!(
        CacheKey::from_target(&target("https://bücher.example/", None)),
        CacheKey::from_target(&target("https://xn--bcher-kva.example", None))
    );
    assert_ne!(
        CacheKey::from_target(&target("https://ab", Some("c"))),
        CacheKey::from_target(&target("https://a", Some("bc")))
    );
}
#[test]
fn test_captured_frame_json_helpers() {
    let mut f = frame();
    assert!(!f.is_json_response());
    assert!(!f.is_graphql());
    for mime in [
        "application/json",
        "Application/JSON; charset=utf-8",
        "application/problem+json",
        "application/graphql-response+json",
    ] {
        f.response_headers
            .insert("CoNtEnT-TyPe".into(), mime.into());
        assert!(f.is_json_response());
    }
    for mime in [
        "text/html",
        "application/jsonp",
        "application/notjson",
        "json",
        "application/json/more",
    ] {
        f.response_headers
            .insert("CoNtEnT-TyPe".into(), mime.into());
        assert!(!f.is_json_response());
    }
    f.response_body = Some(Value::Null);
    assert!(f.is_json_response());
    f.request_body = Some(json!({"query":"query { users { id } }"}));
    assert!(f.is_graphql());
    f.request_body = None;
    f.url.set_path("/v1/graphql");
    assert!(f.is_graphql());
    f.url.set_path("/notgraphql");
    assert!(!f.is_graphql());
}
#[test]
fn graphql_batches_persisted_and_get_queries() {
    let mut f = frame();
    for body in [
        json!({"extensions":{"persistedQuery":{"version":1,"sha256Hash":"abc"}}}),
        json!([{"query":"{x}"},{"query":"{y}"}]),
    ] {
        f.request_body = Some(body);
        assert!(f.is_graphql());
    }
    for body in [
        json!([]),
        json!({"query":4}),
        json!({"operationName":"search"}),
        json!([{"query":"{x}"},{}]),
    ] {
        f.request_body = Some(body);
        assert!(!f.is_graphql());
    }
    f.request_body = None;
    f.url.set_query(Some("query=%7Busers%7Bid%7D%7D"));
    assert!(f.is_graphql());
    f.url.set_query(Some("query="));
    assert!(!f.is_graphql());
    f.url.set_query(Some(
        "extensions=%7B%22persistedQuery%22:%7B%22version%22:1%7D%7D",
    ));
    assert!(f.is_graphql());
    let serialized = serde_json::to_vec(&f).unwrap();
    assert_eq!(
        serde_json::from_slice::<CapturedFrame>(&serialized).unwrap(),
        f
    );
}
#[test]
fn envelope_rejects_invalid_versions_ids_params_and_batches() {
    for value in [
        json!({"jsonrpc":"1.0","id":1,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":null,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":true,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":null}),
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":5}),
        json!([]),
    ] {
        assert!(serde_json::from_value::<McpRequest>(value).is_err());
    }
    for id in [
        json!("abc"),
        json!(0),
        json!(-1),
        json!(1.5),
        json!(u64::MAX),
    ] {
        let value = json!({"jsonrpc":"2.0","id":id,"method":"custom","params":[]});
        assert_eq!(
            serde_json::to_value(serde_json::from_value::<McpRequest>(value.clone()).unwrap())
                .unwrap(),
            value
        );
    }
    assert!(
        serde_json::from_str::<McpRequest>(r#"{"jsonrpc":"2.0","id":1,"id":2,"method":"x"}"#)
            .is_err()
    );
    let mut req = request("tools/list", None);
    req.jsonrpc = "1.0".into();
    assert!(serde_json::to_value(req).is_err());
}
#[test]
fn response_null_success_errors_and_exclusivity() {
    let response = McpResponse::success("null-success".into(), Value::Null);
    let value = serde_json::to_value(&response).unwrap();
    assert_eq!(value["result"], Value::Null);
    assert!(value.get("result").is_some());
    assert!(value.get("error").is_none());
    assert_eq!(
        serde_json::from_value::<McpResponse>(value).unwrap(),
        response
    );
    let error = McpResponse::error(
        1_i64.into(),
        -32602,
        "invalid arguments",
        Some(json!({"field":"q"})),
    );
    let value = serde_json::to_value(&error).unwrap();
    assert!(value.get("result").is_none());
    assert_eq!(serde_json::from_value::<McpResponse>(value).unwrap(), error);
    for extra in [
        json!({}),
        json!({"result":null,"error":{"code":-1,"message":"x"}}),
        json!({"error":null}),
        json!({"error":{"code":"bad","message":"x"}}),
    ] {
        let mut value = json!({"jsonrpc":"2.0","id":1});
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert!(serde_json::from_value::<McpResponse>(value).is_err());
    }
    let mut invalid = response;
    invalid.error = error.error;
    assert!(serde_json::to_value(invalid).is_err());
}
#[test]
fn request_method_helpers_and_call_validation() {
    assert!(request("initialize", None).is_initialize());
    assert!(request("tools/list", None).is_tools_list());
    assert!(request("tools/call", Some("weather")).is_tools_call());
    assert!(!request("Tools/Call", None).is_tools_call());
    assert!(request("tools/list", None).extract_tool_call().is_err());
    for params in [
        json!({}),
        json!({"name":1}),
        json!({"name":"a","arguments":null}),
        json!({"name":"a","arguments":[]}),
    ] {
        let req = McpRequest::new(1_i64.into(), "tools/call", Some(params)).unwrap();
        assert!(req.extract_tool_call().is_err());
    }
    let req = McpRequest::new(1_i64.into(), "tools/call", Some(json!({"name":"a"}))).unwrap();
    assert!(req.extract_tool_call().unwrap().arguments.is_empty());
}
#[test]
fn headers_are_borrowed_and_missing_optional_values_stay_absent() {
    let map = headers("tools/list", None);
    let parsed = McpHeaders::parse(&map).unwrap();
    assert_eq!(
        parsed.method.unwrap().as_ptr(),
        map.get(MCP_METHOD).unwrap().as_bytes().as_ptr()
    );
    assert_eq!(parsed.stream, None);
    assert_eq!(parsed.ttl_ms, None);
    assert!(matches!(
        decode_header_value("weather").unwrap(),
        Cow::Borrowed(_)
    ));
    assert_eq!(McpHeaders::parse(&HeaderMap::new()).unwrap().method, None);
}
#[test]
fn headers_reject_duplicate_unsafe_or_malformed_values() {
    for (name, values) in [
        (
            MCP_STREAM,
            vec!["TRUE", "1", " false", "false ", "true,false"],
        ),
        (MCP_CACHE_SCOPE, vec!["public", "Domain", ""]),
        (
            MCP_TTL_MS,
            vec!["+1", "-1", "1.0", "1e3", "18446744073709551616", " 1"],
        ),
        (MCP_METHOD, vec!["", " tools/list", "tools/list "]),
    ] {
        for value in values {
            let mut map = headers("tools/list", None);
            map.insert(name, HeaderValue::from_str(value).unwrap());
            assert!(McpHeaders::parse(&map).is_err(), "{name}: {value}");
        }
    }
    for name in [
        MCP_PROTOCOL_VERSION,
        MCP_METHOD,
        MCP_NAME,
        MCP_STREAM,
        MCP_CACHE_SCOPE,
        MCP_TTL_MS,
        MCP_SESSION_BRIDGE,
    ] {
        let mut map = HeaderMap::new();
        map.append(name, HeaderValue::from_static("x"));
        map.append(name, HeaderValue::from_static("x"));
        assert!(McpHeaders::parse(&map).is_err());
    }
    let mut map = HeaderMap::new();
    map.insert(MCP_NAME, HeaderValue::from_bytes(&[0xff]).unwrap());
    assert!(McpHeaders::parse(&map).is_err());
    for ttl in [0_u64, u64::MAX] {
        let mut map = HeaderMap::new();
        map.insert(MCP_TTL_MS, HeaderValue::from_str(&ttl.to_string()).unwrap());
        assert_eq!(McpHeaders::parse(&map).unwrap().ttl_ms, Some(ttl));
    }
}
#[test]
fn encoded_header_names_roundtrip_and_validate() {
    for name in [
        "weather",
        "Hello, 世界",
        " padded ",
        "line1\nline2",
        "=?base64?literal?=",
        "",
    ] {
        let encoded = encode_header_value(name);
        assert_eq!(
            decode_header_value(encoded.to_str().unwrap()).unwrap(),
            name
        );
        let map = headers("tools/call", Some(name));
        McpHeaders::parse(&map)
            .unwrap()
            .validate_request(&request("tools/call", Some(name)))
            .unwrap();
    }
    assert_eq!(
        encode_header_value("Hello, 世界").to_str().unwrap(),
        "=?base64?SGVsbG8sIOS4lueVjA==?="
    );
    for value in [
        "=?base64?%%%?=",
        "=?base64?/w==?=",
        " leading",
        "trailing ",
        "\r\n",
    ] {
        assert!(decode_header_value(value).is_err());
    }
}
#[test]
fn routing_headers_must_match_body() {
    let req = request("tools/call", Some("weather"));
    for name in [MCP_PROTOCOL_VERSION, MCP_METHOD, MCP_NAME] {
        let mut map = headers("tools/call", Some("weather"));
        map.remove(name);
        assert!(
            McpHeaders::parse(&map)
                .unwrap()
                .validate_request(&req)
                .is_err()
        );
        map.insert(name, HeaderValue::from_static("wrong"));
        assert!(
            McpHeaders::parse(&map)
                .unwrap()
                .validate_request(&req)
                .is_err()
        );
    }
    let mut no_meta = req.clone();
    no_meta
        .params
        .as_mut()
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("_meta");
    assert!(
        McpHeaders::parse(&headers("tools/call", Some("weather")))
            .unwrap()
            .validate_request(&no_meta)
            .is_err()
    );
    let mut read = request("resources/read", None);
    read.params.as_mut().unwrap()["uri"] = json!("file:///a");
    McpHeaders::parse(&headers("resources/read", Some("file:///a")))
        .unwrap()
        .validate_request(&read)
        .unwrap();
}
#[test]
fn schema_validation_and_wire_field_names() {
    let value = json!({"name":"search","description":"Search","inputSchema":{"type":"object","properties":{"q":{"type":"string"}},"required":["q"],"additionalProperties":false}});
    let tool: ToolDefinition = serde_json::from_value(value.clone()).unwrap();
    tool.validate().unwrap();
    assert_eq!(serde_json::to_value(&tool).unwrap(), value);
    for schema in [
        json!(null),
        json!(false),
        json!([]),
        json!({"type":"invalid"}),
        json!({"properties":[]}),
        json!({"required":"q"}),
        json!({"$schema":"https://untrusted.example/schema"}),
        json!({"$schema":42}),
    ] {
        let mut invalid = value.clone();
        invalid["inputSchema"] = schema;
        assert!(serde_json::from_value::<ToolDefinition>(invalid).is_err());
    }
}
#[test]
fn content_and_cache_hint_roundtrip_and_standard_projection() {
    let result = ToolCallResult {
        content: vec![
            ContentBlock::Text {
                text: "hello".into(),
            },
            ContentBlock::Json {
                json: json!({"count":2}),
            },
            ContentBlock::Resource {
                resource: ResourceContent {
                    uri: "file:///a".into(),
                    mime_type: Some("text/plain".into()),
                    text: "contents".into(),
                },
            },
        ],
        is_error: false,
        cache_hint: Some(CacheHint {
            ttl_ms: 500,
            cache_scope: CacheScope::Task,
            revalidate_in_background: true,
        }),
    };
    assert_eq!(
        serde_json::from_value::<ToolCallResult>(serde_json::to_value(&result).unwrap()).unwrap(),
        result
    );
    let wire = result.to_mcp_value().unwrap();
    assert_eq!(wire["resultType"], "complete");
    assert_eq!(wire["isError"], false);
    assert_eq!(
        wire["content"][1],
        json!({"type":"text","text":"{\"count\":2}"})
    );
    assert_eq!(wire["_meta"]["reify/cacheHint"]["cacheScope"], "task");
    assert!(wire.get("cacheHint").is_none());
    let failed = ToolCallResult {
        content: vec![],
        is_error: true,
        cache_hint: None,
    };
    assert_eq!(failed.to_mcp_value().unwrap()["isError"], true);
}
#[test]
fn compiled_tools_auth_and_error_conversion() {
    let tool = CompiledTool {
        definition: serde_json::from_value(
            json!({"name":"get","description":"Get","inputSchema":{"type":"object"}}),
        )
        .unwrap(),
        execution: ExecutionSpec {
            upstream_url_template: "https://example.com/{id}".into(),
            http_method: "GET".into(),
            query_param_mappings: HashMap::from([("q".into(), "query".into())]),
            body_template: None,
            required_auth_keys: vec!["authorization".into()],
        },
    };
    assert_eq!(
        serde_json::from_value::<CompiledTool>(serde_json::to_value(&tool).unwrap()).unwrap(),
        tool
    );
    let auth = AuthContext {
        cookies: HashMap::from([("session".into(), "secret".into())]),
        authorization_header: Some("Bearer secret".into()),
        expires_at: Some(Utc::now()),
        ..AuthContext::default()
    };
    assert!(
        serde_json::from_value::<AuthContext>(serde_json::to_value(&auth).unwrap()).unwrap()
            == auth
    );
    let error: ReifyError = serde_json::from_str::<Value>("{").unwrap_err().into();
    assert!(error.to_string().starts_with("JSON serialization failed:"));
}

#[test]
fn programmatically_invalid_schemas_cannot_be_published() {
    let mut tool: ToolDefinition =
        serde_json::from_value(json!({"name":"a","description":"A","inputSchema":{}})).unwrap();
    tool.metadata = Some(ToolMetadata {
        tags: vec!["api".into()],
        source_endpoint: Some("https://example.com/api".into()),
    });
    let projected = tool.to_mcp_value().unwrap();
    assert_eq!(
        projected["_meta"]["reify/provenance"]["tags"],
        json!(["api"])
    );
    assert!(projected.get("metadata").is_none());
    tool.input_schema = json!({"required":false});
    assert!(tool.validate().is_err());
    assert!(serde_json::to_value(&tool).is_err());
    assert!(tool.to_mcp_value().is_err());
}

#[test]
fn root_slashes_and_all_cache_scope_variants() {
    assert_eq!(
        target("https://example.com////?#x", None).url.as_str(),
        "https://example.com/"
    );
    for (wire, scope) in [
        ("task", CacheScope::Task),
        ("domain", CacheScope::Domain),
        ("global", CacheScope::Global),
    ] {
        let mut map = HeaderMap::new();
        map.insert(MCP_CACHE_SCOPE, HeaderValue::from_static(wire));
        map.insert(MCP_STREAM, HeaderValue::from_static("true"));
        let parsed = McpHeaders::parse(&map).unwrap();
        assert_eq!(parsed.cache_scope, Some(scope));
        assert_eq!(parsed.stream, Some(true));
        assert_eq!(serde_json::to_value(scope).unwrap(), wire);
    }
}

#[test]
fn owned_call_extraction_retains_metadata_and_wire_definitions_match() {
    let mut req = request("tools/call", Some("weather"));
    let expected = req.extract_tool_call().unwrap();
    let pointer = req.params.as_ref().unwrap()["arguments"]["city"]
        .as_str()
        .unwrap()
        .as_ptr();
    let actual = req.take_tool_call().unwrap();
    assert_eq!(actual.arguments["city"].as_str().unwrap().as_ptr(), pointer);
    assert_eq!(actual, expected);
    assert!(req.params.as_ref().unwrap().get("_meta").is_some());
    assert!(req.params.as_ref().unwrap().get("arguments").is_none());
    let definition:ToolDefinition=serde_json::from_value(json!({"name":"search","description":"Unicode 世界 and quotes \"","inputSchema":{"type":"object","properties":{"q":{"type":"string"}}},"metadata":{"tags":["api"],"sourceEndpoint":"https://example.com/api"}})).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(definition.to_mcp_json().unwrap().get()).unwrap(),
        definition.to_mcp_value().unwrap()
    );
    let result = ToolCallResult {
        content: vec![
            ContentBlock::Text {
                text: "a\n世界".into(),
            },
            ContentBlock::Json {
                json: json!({"x":[1,null,true]}),
            },
            ContentBlock::Resource {
                resource: ResourceContent {
                    uri: "file:///a".into(),
                    mime_type: None,
                    text: "hello".into(),
                },
            },
        ],
        is_error: true,
        cache_hint: Some(CacheHint {
            ttl_ms: 0,
            cache_scope: CacheScope::Domain,
            revalidate_in_background: true,
        }),
    };
    assert_eq!(
        result.to_mcp_value().unwrap(),
        result.into_mcp_value().unwrap()
    );
}
#[test]
fn goal_normalization_preserves_contextual_unicode_lowercase() {
    for goal in [
        "  SEARCH\tITEMS\n",
        " ΟΣ \t İСТАНБУЛ  ",
        "\u{2003} A\u{00a0} B ",
        "",
    ] {
        let expected = goal
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        assert_eq!(normalize_goal(goal), expected);
    }
}
