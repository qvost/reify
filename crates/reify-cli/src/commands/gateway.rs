//! Explicit remote-gateway cache reuse over the modern stateless MCP envelope.
use crate::{CliError, failure};
use reify_core::mcp::{McpRequest, PROTOCOL_VERSION, RequestId, encode_header_value};
use serde_json::{Value, json};
use std::{net::SocketAddr, time::Duration};
use url::Url;
/// Display a usable loopback endpoint for wildcard-bound local listeners.
pub fn endpoint(address: SocketAddr) -> String {
    let address = if address.ip().is_unspecified() {
        SocketAddr::new(
            if address.is_ipv4() {
                std::net::Ipv4Addr::LOCALHOST.into()
            } else {
                std::net::Ipv6Addr::LOCALHOST.into()
            },
            address.port(),
        )
    } else {
        address
    };
    format!("http://{address}/mcp")
}
async fn rpc(
    client: &reqwest::Client,
    endpoint: &Url,
    id: u64,
    params: Value,
) -> Result<Value, CliError> {
    let name = params["name"]
        .as_str()
        .ok_or_else(|| failure("missing RPC tool name"))?;
    let name_header = encode_header_value(name);
    let message = McpRequest::new(RequestId::Number(id.into()), "tools/call", Some(params))?;
    let response = client
        .post(endpoint.clone())
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", PROTOCOL_VERSION)
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", name_header)
        .json(&message)
        .send()
        .await
        .map_err(|_| failure("cannot reach gateway; check --gateway and start 'reify serve'"))?;
    let status = response.status();
    let mut response = response;
    if response
        .content_length()
        .is_some_and(|n| n > 8 * 1024 * 1024)
    {
        return Err(failure("gateway response exceeds 8 MiB"));
    }
    let mut bytes =
        Vec::with_capacity(response.content_length().unwrap_or(0).min(8 * 1024 * 1024) as usize);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| failure("gateway response read failed"))?
    {
        if bytes.len() + chunk.len() > 8 * 1024 * 1024 {
            return Err(failure("gateway response exceeds 8 MiB"));
        }
        bytes.extend_from_slice(&chunk);
    }
    let mut value: Value = serde_json::from_slice(&bytes)?;
    if !status.is_success() || value.get("error").is_some() {
        return Err(failure(format!(
            "gateway rejected request (HTTP {status}, RPC code {})",
            value["error"]["code"]
        )));
    }
    if value["jsonrpc"] != "2.0" || value["id"] != id {
        return Err(failure("gateway returned a mismatched JSON-RPC response"));
    }
    value
        .as_object_mut()
        .and_then(|value| value.remove("result"))
        .ok_or_else(|| failure("gateway returned no result"))
}
fn meta() -> Value {
    json!({"io.modelcontextprotocol/protocolVersion":PROTOCOL_VERSION,"io.modelcontextprotocol/clientInfo":{"name":"reify-cli","version":env!("CARGO_PKG_VERSION")},"io.modelcontextprotocol/clientCapabilities":{}})
}
/// Bootstrap a cache-backed remote mount, then invoke its exact tool without retries.
pub async fn call_remote(
    endpoint: &Url,
    url: &Url,
    goal: Option<String>,
    name: &str,
    arguments: Value,
) -> Result<Value, CliError> {
    call_remote_with_fresh(endpoint, url, goal, name, arguments, false).await
}
/// Bootstrap a remote mount, optionally forcing live discovery instead of a profile.
pub async fn call_remote_with_fresh(
    endpoint: &Url,
    url: &Url,
    goal: Option<String>,
    name: &str,
    arguments: Value,
    fresh: bool,
) -> Result<Value, CliError> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(50))
        .build()
        .map_err(|_| failure("cannot construct gateway client"))?;
    let mounted = rpc(
        &client,
        endpoint,
        1,
        json!({"name":"reify","arguments":{"url":url,"goal":goal,"fresh":fresh},"_meta":meta()}),
    )
    .await?;
    if mounted["isError"] == true {
        return Err(failure(
            "gateway could not compile the target; check its Chromium configuration",
        ));
    }
    let token = mounted["structuredContent"]["mount"]
        .as_str()
        .ok_or_else(|| failure("gateway returned no mount token"))?;
    let mut meta = meta();
    meta["reify/mount"] = json!(token);
    rpc(
        &client,
        endpoint,
        2,
        json!({"name":name,"arguments":arguments,"_meta":meta}),
    )
    .await
}
