//! A single socket actor correlates commands while independently forwarding events.
use crate::failure;
use futures_util::{SinkExt, StreamExt};
use reify_core::ReifyError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    borrow::Cow,
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use url::Url;

/// An incoming CDP event, including its flattened target session.
#[derive(Debug, Deserialize)]
pub struct CdpEvent {
    /// Domain-qualified event name.
    pub method: String,
    /// Event arguments.
    #[serde(default)]
    pub params: Value,
    /// Page session, absent on browser events.
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
}
/// Parsed socket message for correlation or event dispatch.
#[derive(Debug)]
pub enum Incoming {
    /// A command result or sanitized CDP error.
    Reply(u64, Result<Value, ReifyError>),
    /// Unsolicited notification.
    Event(CdpEvent),
}
/// Decode CDP JSON without assuming event/response ordering.
#[derive(Deserialize)]
struct IncomingWire {
    id: Option<Value>,
    method: Option<String>,
    #[serde(default)]
    params: Value,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(default, deserialize_with = "present")]
    result: Option<Value>,
    #[serde(default, deserialize_with = "present")]
    error: Option<Value>,
}
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

/// Parses a raw Chrome DevTools Protocol JSON message into an [`Incoming`] enum.
/// Decode a CDP envelope directly into typed metadata and owned payloads.
pub fn parse_message(text: &str) -> Result<Incoming, ReifyError> {
    let wire: IncomingWire = serde_json::from_str(text)?;
    if let Some(id) = wire.id.as_ref().and_then(Value::as_u64) {
        let result = if let Some(error) = wire.error {
            Err(failure(
                "CDP command rejected with code",
                error.get("code").unwrap_or(&Value::Null),
            ))
        } else {
            wire.result
                .ok_or_else(|| failure("CDP response", "missing result"))
        };
        Ok(Incoming::Reply(id, result))
    } else {
        Ok(Incoming::Event(CdpEvent {
            method: wire
                .method
                .ok_or_else(|| failure("CDP event", "missing method"))?,
            params: wire.params,
            session_id: wire.session_id,
        }))
    }
}
#[derive(Deserialize)]
#[serde(transparent)]
struct WireString<'a>(#[serde(borrow)] Cow<'a, str>);
#[derive(Deserialize)]
struct FilteredWire<'a> {
    id: Option<Value>,
    #[serde(borrow)]
    method: Option<WireString<'a>>,
    #[serde(rename = "sessionId", borrow)]
    session_id: Option<WireString<'a>>,
    #[serde(default, borrow)]
    params: Option<&'a serde_json::value::RawValue>,
    #[serde(default, deserialize_with = "present")]
    result: Option<Value>,
    #[serde(default, deserialize_with = "present")]
    error: Option<Value>,
}
fn parse_filtered(text: &str, methods: &[&str]) -> Result<Option<Incoming>, ReifyError> {
    let wire: FilteredWire<'_> = serde_json::from_str(text)?;
    if let Some(id) = wire.id.as_ref().and_then(Value::as_u64) {
        let result = if let Some(error) = wire.error {
            Err(failure(
                "CDP command rejected with code",
                error.get("code").unwrap_or(&Value::Null),
            ))
        } else {
            wire.result
                .ok_or_else(|| failure("CDP response", "missing result"))
        };
        return Ok(Some(Incoming::Reply(id, result)));
    }
    let method = wire
        .method
        .ok_or_else(|| failure("CDP event", "missing method"))?
        .0;
    if !methods.contains(&method.as_ref()) {
        return Ok(None);
    }
    let params = wire
        .params
        .map(|p| serde_json::from_str(p.get()))
        .transpose()?
        .unwrap_or(Value::Null);
    Ok(Some(Incoming::Event(CdpEvent {
        method: method.into_owned(),
        params,
        session_id: wire.session_id.map(|session| session.0.into_owned()),
    })))
}
#[derive(Serialize)]
struct CommandMessage<'a> {
    id: u64,
    method: &'a str,
    params: &'a Value,
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    session: Option<&'a str>,
}
struct Call {
    id: u64,
    payload: String,
    reply: oneshot::Sender<Result<Value, ReifyError>>,
}
/// Command handle owning a socket actor. Drop aborts the actor and closes its socket.
pub struct CdpClient {
    commands: mpsc::Sender<Call>,
    events: Option<mpsc::Receiver<CdpEvent>>,
    task: JoinHandle<()>,
    next_id: AtomicU64,
}
impl CdpClient {
    /// Resolve the browser WebSocket through loopback HTTP with proxies disabled.
    pub async fn discover(port: u16) -> Result<Self, ReifyError> {
        Self::discover_with_filter(port, None).await
    }
    pub(crate) async fn discover_for_probe(port: u16) -> Result<Self, ReifyError> {
        const METHODS: &[&str] = &[
            "Page.loadEventFired",
            "Network.requestWillBeSent",
            "Network.responseReceived",
            "Network.loadingFinished",
            "Network.loadingFailed",
        ];
        Self::discover_with_filter(port, Some(METHODS)).await
    }
    async fn discover_with_filter(
        port: u16,
        filter: Option<&'static [&'static str]>,
    ) -> Result<Self, ReifyError> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| failure("DevTools HTTP client", e))?;
        let response: Value = client
            .get(format!("http://127.0.0.1:{port}/json/version"))
            .send()
            .await
            .map_err(|e| failure("DevTools discovery", e))?
            .error_for_status()
            .map_err(|e| failure("DevTools status", e))?
            .json()
            .await
            .map_err(|e| failure("DevTools discovery JSON", e))?;
        let url = response
            .get("webSocketDebuggerUrl")
            .and_then(Value::as_str)
            .ok_or_else(|| failure("DevTools discovery", "missing webSocketDebuggerUrl"))?;
        Self::connect_with_filter(url, filter).await
    }
    /// Open a loopback CDP socket with bounded message and command queues.
    pub async fn connect(endpoint: &str) -> Result<Self, ReifyError> {
        Self::connect_with_filter(endpoint, None).await
    }
    async fn connect_with_filter(
        endpoint: &str,
        filter: Option<&'static [&'static str]>,
    ) -> Result<Self, ReifyError> {
        let url = Url::parse(endpoint).map_err(|e| failure("DevTools WebSocket URL", e))?;
        if url.scheme() != "ws"
            || !matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
        {
            return Err(failure("DevTools endpoint", "must use loopback ws"));
        }
        let config = WebSocketConfig {
            max_message_size: Some(8 * 1024 * 1024),
            max_frame_size: Some(8 * 1024 * 1024),
            ..Default::default()
        };
        let (mut socket, _) = tokio::time::timeout(
            Duration::from_secs(5),
            connect_async_with_config(endpoint, Some(config), false),
        )
        .await
        .map_err(|_| failure("CDP connect", "timeout"))?
        .map_err(|e| failure("CDP connect", e))?;
        let (commands, mut receive) = mpsc::channel::<Call>(64);
        let (events_tx, events) = mpsc::channel(1024);
        let task = tokio::spawn(async move {
            let mut pending: HashMap<u64, oneshot::Sender<Result<Value, ReifyError>>> =
                HashMap::with_capacity(16);
            loop {
                tokio::select! {
                    call = receive.recv() => {
                        let Some(call) = call else { break; };
                        if pending.len()>=256 {pending.retain(|_, sender| !sender.is_closed());}
                        if pending.len() >= 256 { let _ = call.reply.send(Err(failure("CDP", "too many pending commands"))); continue; }
                        let message = Message::Text(call.payload);
                        pending.insert(call.id, call.reply);
                        if !matches!(tokio::time::timeout(Duration::from_secs(5), socket.send(message)).await, Ok(Ok(()))) { break; }
                    }
                    incoming = socket.next() => {
                        match incoming {
                            Some(Ok(Message::Text(text))) => match filter.map_or_else(||parse_message(&text).map(Some),|methods|parse_filtered(&text,methods)) {
                                Ok(Some(Incoming::Reply(id, result))) => { if let Some(sender) = pending.remove(&id) { let _ = sender.send(result); } }
                                Ok(Some(Incoming::Event(event))) => { if events_tx.try_send(event).is_err() { break; } }
                                Ok(None)=>{},
                                Err(_) => break,
                            },
                            Some(Ok(Message::Ping(bytes))) => { if !matches!(tokio::time::timeout(Duration::from_secs(5), socket.send(Message::Pong(bytes))).await, Ok(Ok(()))) { break; } }
                            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                            _ => {}
                        }
                    }
                }
            }
            for (_, sender) in pending {
                let _ = sender.send(Err(failure(
                    "CDP socket",
                    "closed or event queue overflowed",
                )));
            }
        });
        Ok(Self {
            commands,
            events: Some(events),
            task,
            next_id: AtomicU64::new(1),
        })
    }
    /// Take the one event stream. A second call returns None.
    pub fn take_events(&mut self) -> Option<mpsc::Receiver<CdpEvent>> {
        self.events.take()
    }
    /// Execute a browser or flattened-page command with a five-second deadline.
    pub async fn command(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, ReifyError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let payload = serde_json::to_string(&CommandMessage {
            id,
            method,
            params: &params,
            session,
        })?;
        let (reply, response) = oneshot::channel();
        tokio::time::timeout(Duration::from_secs(5), async {
            self.commands
                .send(Call { id, payload, reply })
                .await
                .map_err(|_| failure("CDP command", "socket actor stopped"))?;
            response
                .await
                .map_err(|_| failure("CDP command", "socket closed"))?
        })
        .await
        .map_err(|_| failure("CDP command", "timeout"))?
    }
}
impl Drop for CdpClient {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod filter_tests {
    use super::*;
    #[test]
    fn probe_filter_keeps_replies_nulls_and_selected_events_in_any_key_order() {
        let methods = ["Network.loadingFinished"];
        let envelope: FilteredWire<'_> = serde_json::from_str(
            r#"{"params":{"many":[1,2,3]},"method":"Runtime.consoleAPICalled","sessionId":"page"}"#,
        )
        .unwrap();
        assert!(matches!(envelope.method.unwrap().0, Cow::Borrowed(_)));
        assert!(matches!(envelope.session_id.unwrap().0, Cow::Borrowed(_)));
        assert!(matches!(
            parse_filtered(r#"{"method":"Network.\u006coadingFinished","params":{},"sessionId":"pa\u0067e"}"#, &methods).unwrap(),
            Some(Incoming::Event(CdpEvent { method, session_id: Some(session), .. }))
                if method == methods[0] && session == "page"
        ));
        assert!(
            parse_filtered(
                r#"{"params":{"many":[1,2,3]},"method":"Runtime.consoleAPICalled"}"#,
                &methods
            )
            .unwrap()
            .is_none()
        );
        assert!(matches!(
            parse_filtered(r#"{"result":null,"sessionId":"page","id":1}"#, &methods).unwrap(),
            Some(Incoming::Reply(1, Ok(Value::Null)))
        ));
        let wire =
            r#"{"params":{"requestId":"r"},"sessionId":"page","method":"Network.loadingFinished"}"#;
        match (
            parse_filtered(wire, &methods).unwrap().unwrap(),
            parse_message(wire).unwrap(),
        ) {
            (Incoming::Event(a), Incoming::Event(b)) => {
                assert_eq!(a.method, b.method);
                assert_eq!(a.params, b.params);
                assert_eq!(a.session_id, b.session_id);
            }
            _ => panic!("expected matching events"),
        }
        assert!(
            parse_filtered(r#"{"id":1}"#, &methods)
                .unwrap()
                .is_some_and(|m| matches!(m, Incoming::Reply(1, Err(_))))
        );
        assert!(
            parse_filtered(
                r#"{"method":"Runtime.consoleAPICalled","params":[}"#,
                &methods
            )
            .is_err()
        );
    }
}
