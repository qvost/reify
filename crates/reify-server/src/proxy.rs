//! Bounded upstream invocation. Redirects are disabled to prevent credential forwarding.
use crate::failure;
use bytes::Bytes;
use chrono::Utc;
use futures_util::{Stream, stream};
use http::Method;
use reify_core::{
    ReifyError,
    domain::{AuthContext, CompiledTool},
    mcp::{ContentBlock, ToolCallResult},
};
use serde_json::Value;
use std::{pin::Pin, time::Duration};

const MAX_RESPONSE: usize = 4 * 1024 * 1024;
pub(crate) type UpstreamChunks = Pin<Box<dyn Stream<Item = Result<Bytes, &'static str>> + Send>>;

/// Upstream status and a bounded stream of response body chunks.
pub(crate) struct StreamingProxyResponse {
    /// Upstream HTTP status, including non-success statuses.
    pub status: u16,
    /// Body chunks, or one terminal bounded-read error.
    pub chunks: UpstreamChunks,
}
/// One invocation plus status for drift classification.
pub struct ProxyResponse {
    /// Upstream HTTP status, including non-success statuses.
    pub status: u16,
    /// Portable content plus a tool-level failure flag.
    pub result: ToolCallResult,
}
/// Connection-pooled upstream client with bounded timeouts and TLS support.
#[derive(Clone)]
pub struct UpstreamProxy {
    client: reqwest::Client,
    bridge: Option<reify_bridge::BridgeClient>,
}
impl UpstreamProxy {
    /// Configure a pooled client; no automatic retries of mutation methods.
    pub fn new() -> Result<Self, ReifyError> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .user_agent(concat!("reify/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|_| failure("cannot construct upstream client"))?;
        Ok(Self {
            client,
            bridge: reify_bridge::BridgeClient::from_env()?,
        })
    }
    /// Render and invoke a synthesized tool, preserving JSON and HTTP failures.
    pub async fn execute(
        &self,
        tool: &CompiledTool,
        arguments: &Value,
        auth: &AuthContext,
    ) -> Result<ProxyResponse, ReifyError> {
        let prepared = reify_synth::compiler::PreparedTool::new(tool)?;
        let headers = auth.to_headers()?;
        self.execute_prepared(tool, &prepared, arguments, auth, &headers)
            .await
    }
    pub(crate) async fn execute_prepared(
        &self,
        tool: &CompiledTool,
        prepared: &reify_synth::compiler::PreparedTool,
        arguments: &Value,
        auth: &AuthContext,
        headers: &http::HeaderMap,
    ) -> Result<ProxyResponse, ReifyError> {
        let (status, mut response) = self
            .send_prepared(tool, prepared, arguments, auth, headers)
            .await?;
        if response
            .content_length()
            .is_some_and(|n| n > MAX_RESPONSE as u64)
        {
            return Ok(body_failure(status, "upstream response exceeds 4 MiB"));
        }
        let mut bytes = Vec::with_capacity(
            response
                .content_length()
                .unwrap_or(0)
                .min(MAX_RESPONSE as u64) as usize,
        );
        loop {
            let chunk = match response.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(_) => return Ok(body_failure(status, "cannot read upstream response")),
            };
            if bytes.len() + chunk.len() > MAX_RESPONSE {
                return Ok(body_failure(status, "upstream response exceeds 4 MiB"));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(body_result(status, bytes))
    }

    /// Invoke a prepared tool and expose its body as bounded upstream chunks.
    pub(crate) async fn execute_prepared_streaming(
        &self,
        tool: &CompiledTool,
        prepared: &reify_synth::compiler::PreparedTool,
        arguments: &Value,
        auth: &AuthContext,
        headers: &http::HeaderMap,
    ) -> Result<StreamingProxyResponse, ReifyError> {
        let (status, response) = self
            .send_prepared(tool, prepared, arguments, auth, headers)
            .await?;
        let oversized = response
            .content_length()
            .is_some_and(|n| n > MAX_RESPONSE as u64);
        let chunks = if oversized {
            Box::pin(stream::once(async {
                Err("upstream response exceeds 4 MiB")
            })) as UpstreamChunks
        } else {
            Box::pin(stream::unfold(
                (response, 0usize, false),
                |(mut response, mut size, done)| async move {
                    if done {
                        return None;
                    }
                    match response.chunk().await {
                        Ok(Some(chunk)) => {
                            if size.saturating_add(chunk.len()) > MAX_RESPONSE {
                                Some((
                                    Err("upstream response exceeds 4 MiB"),
                                    (response, size, true),
                                ))
                            } else {
                                size += chunk.len();
                                Some((Ok(chunk), (response, size, false)))
                            }
                        }
                        Ok(None) => None,
                        Err(_) => {
                            Some((Err("cannot read upstream response"), (response, size, true)))
                        }
                    }
                },
            )) as UpstreamChunks
        };
        Ok(StreamingProxyResponse { status, chunks })
    }

    async fn send_prepared(
        &self,
        tool: &CompiledTool,
        prepared: &reify_synth::compiler::PreparedTool,
        arguments: &Value,
        auth: &AuthContext,
        headers: &http::HeaderMap,
    ) -> Result<(u16, reqwest::Response), ReifyError> {
        let rendered = prepared.render(arguments)?;
        let current = match &self.bridge {
            Some(bridge) => Some(bridge.auth_for(&rendered.url).await?.unwrap_or_default()),
            None => None,
        };
        let current_headers = current.as_ref().map(AuthContext::to_headers).transpose()?;
        let auth = current.as_ref().unwrap_or(auth);
        let headers = current_headers.as_ref().unwrap_or(headers);
        if auth.expires_at.is_some_and(|expiry| expiry <= Utc::now()) {
            return Err(failure(
                "upstream credentials have expired; reify the target again",
            ));
        }
        for key in &tool.execution.required_auth_keys {
            let lower = key.as_str();
            let present = headers.contains_key(lower)
                || auth
                    .csrf_tokens
                    .keys()
                    .any(|k| k.eq_ignore_ascii_case(lower))
                || (lower.eq_ignore_ascii_case("cookie") && !auth.cookies.is_empty());
            if !present {
                return Err(failure(format!(
                    "missing required upstream credential key: {key}"
                )));
            }
        }
        let method = Method::from_bytes(rendered.method.as_bytes())
            .map_err(|_| failure("invalid upstream HTTP method"))?;
        let mut request = self
            .client
            .request(method, rendered.url)
            .headers(headers.clone());
        if let Some(body) = rendered.body {
            request = request.json(body.as_ref());
        }
        let response = request
            .send()
            .await
            .map_err(|_| failure("upstream request failed or timed out"))?;
        let status = response.status().as_u16();
        Ok((status, response))
    }
}
pub(crate) fn body_result(status: u16, bytes: Vec<u8>) -> ProxyResponse {
    let block = match serde_json::from_slice(&bytes) {
        Ok(json) => ContentBlock::Json { json },
        Err(_) => ContentBlock::Text {
            text: String::from_utf8(bytes)
                .unwrap_or_else(|error| String::from_utf8_lossy(&error.into_bytes()).into_owned()),
        },
    };
    ProxyResponse {
        status,
        result: ToolCallResult {
            content: vec![block],
            is_error: !(200..300).contains(&status),
            cache_hint: None,
        },
    }
}
pub(crate) fn body_failure(status: u16, message: &str) -> ProxyResponse {
    // Preserve status evidence for drift even when the body is unavailable.
    ProxyResponse {
        status,
        result: ToolCallResult {
            content: vec![ContentBlock::Text {
                text: message.into(),
            }],
            is_error: true,
            cache_hint: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_are_canonical_sensitive_and_deterministic() {
        let auth = AuthContext {
            cookies: std::collections::HashMap::from([
                ("b".into(), "2".into()),
                ("a".into(), "1".into()),
            ]),
            authorization_header: Some("Bearer secret".into()),
            csrf_tokens: std::collections::HashMap::from([("csrftoken".into(), "csrf".into())]),
            ..Default::default()
        };
        let headers = auth.to_headers().unwrap();
        assert_eq!(headers["cookie"], "a=1; b=2");
        assert_eq!(headers["x-csrf-token"], "csrf");
        assert!(headers["authorization"].is_sensitive());
    }
    #[test]
    fn rejects_header_injection_cookie_separators_and_reserved_headers() {
        let mut auth = AuthContext {
            authorization_header: Some("Bearer x\r\nx-evil: 1".into()),
            ..Default::default()
        };
        assert!(auth.to_headers().is_err());
        auth.authorization_header = None;
        auth.cookies.insert("session".into(), "a; evil=b".into());
        assert!(auth.to_headers().is_err());
        auth.cookies.clear();
        auth.custom_headers
            .insert("Host".into(), "evil.test".into());
        assert!(auth.to_headers().is_err());
    }
}
