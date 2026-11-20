//! Command dispatch, cancellation, and cache-aware compilation.
pub mod gateway;
pub mod install;
pub mod probe;
use crate::{Cli, CliError, Command, failure, ui};
use reify_core::domain::Target;
use reify_probe::ProbeEngine;
use reify_server::{AppBuilder, ReifyServer, store::CachedApp};
use reify_synth::SynthEngine;
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{net::TcpListener, sync::watch};

/// Real measured phase metrics, never inferred or hardcoded.
#[derive(Clone, Debug)]
pub struct CompileMetrics {
    /// API exchanges retained by the probe.
    pub frames: usize,
    /// Elapsed browser capture time, including browser startup.
    pub capture_ms: u64,
    /// Elapsed schema compilation time.
    pub synthesis_ms: u64,
}
struct CliBuilder {
    probe: Arc<ProbeEngine>,
    synth: Arc<SynthEngine>,
    timeout: Duration,
    metrics: Arc<Mutex<Option<CompileMetrics>>>,
    slots: tokio::sync::Semaphore,
}
impl AppBuilder for CliBuilder {
    fn build(
        &self,
        target: Target,
    ) -> Pin<Box<dyn Future<Output = Result<CachedApp, reify_core::ReifyError>> + Send + '_>> {
        Box::pin(async move {
            let _permit =
                self.slots.acquire().await.map_err(|_| {
                    reify_core::ReifyError::ExecutionError("compiler stopped".into())
                })?;
            let started = Instant::now();
            let captured = self.probe.probe(&target, self.timeout).await?;
            let capture_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            let frames = captured.frames.len();
            let synth = self.synth.clone();
            let (app, synthesis_ms) = tokio::task::spawn_blocking(move || {
                let scope = Target::from_url(captured.final_url.clone(), target.goal.clone())?;
                let output = synth.synthesize(&scope, &captured.frames, &captured.auth)?;
                Ok::<_, reify_core::ReifyError>((
                    CachedApp {
                        target,
                        tools: output.tools,
                        auth: captured.auth,
                        created_at: Instant::now(),
                        ttl: Duration::from_secs(300),
                    },
                    output.synthesis_duration_ms,
                ))
            })
            .await
            .map_err(|_| {
                reify_core::ReifyError::ExecutionError("synthesis worker failed".into())
            })??;
            *self.metrics.lock().expect("metrics mutex") = Some(CompileMetrics {
                frames,
                capture_ms,
                synthesis_ms,
            });
            Ok(app)
        })
    }
}
/// Build a production probe respecting the explicit executable override.
pub fn configured_probe(cli: &Cli) -> ProbeEngine {
    cli.chromium
        .as_ref()
        .map_or_else(ProbeEngine::new, ProbeEngine::with_chromium)
}
/// Wait until a shared shutdown signal is set or its sender exits.
pub async fn wait_for_shutdown(mut stop: watch::Receiver<bool>) {
    while !*stop.borrow_and_update() {
        if stop.changed().await.is_err() {
            break;
        }
    }
}
struct Watcher(tokio::task::JoinHandle<()>);
impl Drop for Watcher {
    fn drop(&mut self) {
        self.0.abort();
    }
}
/// Run a checked invocation until completion or a caller-provided shutdown signal.
/// Servers drain for up to five seconds. Short commands and active capture are
/// canceled promptly; the probe's browser owner handles process reaping.
pub async fn run_with_shutdown<F>(cli: Cli, shutdown: F) -> Result<(), CliError>
where
    F: Future<Output = ()> + Send + 'static,
{
    let (signal, stop) = watch::channel(false);
    let _watcher = Watcher(tokio::spawn(async move {
        shutdown.await;
        let _ = signal.send(true);
    }));
    let probe = Arc::new(configured_probe(&cli));
    let timeout = Duration::from_secs(cli.timeout);
    let color = ui::use_color(cli.color);
    let metrics = Arc::new(Mutex::new(None));
    let server = ReifyServer::with_builder(Arc::new(CliBuilder {
        probe: probe.clone(),
        synth: Arc::new(SynthEngine::new()),
        timeout,
        metrics: metrics.clone(),
        slots: tokio::sync::Semaphore::new(2),
    }));
    match cli.command {
        Some(Command::Install(options)) => install::run(&options, color),
        Some(Command::Serve { host, port }) => {
            let listener = TcpListener::bind((host.as_str(), port))
                .await
                .map_err(|e| failure(format!("cannot listen on {host}:{port}: {e}")))?;
            let endpoint = gateway::endpoint(listener.local_addr()?);
            ui::write_stdout(&ui::gateway_banner(&endpoint, color))?;
            drain(
                server.serve_listener(listener, wait_for_shutdown(stop.clone())),
                stop,
            )
            .await
        }
        Some(Command::Probe { url, goal }) => {
            let _progress = ui::Progress::start("Capturing API traffic", color);
            let target = Target::from_url(url, Some(goal.join(" ")))?;
            let output = tokio::select! {output=probe.probe(&target,timeout)=>output?,_=wait_for_shutdown(stop)=>return Err(CliError::Interrupted)};
            drop(_progress);
            let scope = Target::from_url(output.final_url.clone(), target.goal)?;
            let tools = SynthEngine::new()
                .synthesize(&scope, &output.frames, &output.auth)
                .map(|compiled| compiled.tools)
                .unwrap_or_else(|_| {
                    tracing::debug!("probe capture did not support a relevant tool schema");
                    Vec::new()
                });
            ui::write_serializable(&probe::report_with_tools(&output, &tools))?;
            Ok(())
        }
        Some(Command::Call {
            url,
            tool_name,
            json_arguments,
            goal,
            gateway,
        }) => {
            let result = tokio::select! {
                result=async {
                    if let Some(endpoint)=gateway {gateway::call_remote(&endpoint,&url,goal,&tool_name,json_arguments).await}
                    else {let mounted=server.mount(Target::new(url.as_str(),goal)?).await?;
                        let tool=mounted.app.tools.iter().find(|tool|tool.definition.name==tool_name).ok_or_else(||failure(format!("tool '{tool_name}' was not synthesized; available: {}",mounted.app.tools.iter().map(|t|t.definition.name.as_str()).collect::<Vec<_>>().join(", "))))?;
                        let result=reify_server::proxy::UpstreamProxy::new()?.execute(tool,&json_arguments,&mounted.app.auth).await?;
                        Ok(result.result.into_mcp_value()?)
                    }
                }=>result?,_=wait_for_shutdown(stop)=>return Err(CliError::Interrupted)
            };
            ui::write_json(&result)?;
            if result["isError"].as_bool() == Some(true) {
                Err(CliError::ToolFailed)
            } else {
                Ok(())
            }
        }
        Some(Command::Bridge {
            port,
            token,
            session_file,
        }) => {
            let token=token.ok_or_else(||failure("set REIFY_BRIDGE_TOKEN (at least 16 safe ASCII characters) before starting the bridge"))?;
            let bridge = reify_bridge::BridgeServer::with_transport(
                Arc::new(reify_bridge::HttpTransport::new()?),
                token,
            )?;
            if let Some(path) = session_file {
                use tokio::io::AsyncReadExt;
                let file = tokio::fs::File::open(&path).await?;
                let mut bytes = Vec::new();
                file.take(1024 * 1024 + 1).read_to_end(&mut bytes).await?;
                if bytes.len() > 1024 * 1024 {
                    return Err(failure("session file exceeds 1 MiB"));
                }
                let sessions: HashMap<String, reify_core::domain::AuthContext> =
                    serde_json::from_slice(&bytes)?;
                for (domain, auth) in sessions {
                    bridge.store().insert(&domain, auth).await?;
                }
            }
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
            ui::write_stdout(&format!(
                "Reify bridge · http://{}\nForwarding requires the configured bearer token.\nCtrl+C to stop\n",
                listener.local_addr()?
            ))?;
            drain(
                bridge.serve_listener(listener, wait_for_shutdown(stop.clone())),
                stop,
            )
            .await
        }
        None => {
            let target = Target::new(
                cli.url
                    .as_ref()
                    .ok_or_else(|| failure("a URL is required"))?
                    .as_str(),
                if cli.goal.is_empty() {
                    None
                } else {
                    Some(cli.goal.join(" "))
                },
            )?;
            // Reserve the port first; a failed bind must not waste a browser capture.
            let listener=TcpListener::bind("127.0.0.1:8080").await.map_err(|e|failure(format!("cannot listen on 127.0.0.1:8080: {e}; stop the existing gateway or use 'reify serve --port <port>'")))?;
            let progress = ui::Progress::start("Capturing and compiling API tools", color);
            let mounted = tokio::select! {mounted=server.mount(target)=>mounted?,_=wait_for_shutdown(stop.clone())=>return Err(CliError::Interrupted)};
            drop(progress);
            let metrics = metrics.lock().expect("metrics mutex").clone();
            ui::write_stdout(&ui::mount_summary(
                &mounted,
                metrics.as_ref(),
                "http://127.0.0.1:8080/mcp",
                color,
            ))?;
            drain(
                server.serve_listener(listener, wait_for_shutdown(stop.clone())),
                stop,
            )
            .await
        }
    }
}
async fn drain<F>(serving: F, stop: watch::Receiver<bool>) -> Result<(), CliError>
where
    F: Future<Output = Result<(), reify_core::ReifyError>>,
{
    tokio::pin!(serving);
    tokio::select! {result=&mut serving=>result.map_err(Into::into),_=wait_for_shutdown(stop)=>{
        match tokio::time::timeout(Duration::from_secs(5),&mut serving).await {Ok(result)=>result.map_err(Into::into),Err(_)=>Ok(())}
    }}
}
/// Wait for Ctrl+C, and on Unix also SIGTERM, without blocking the runtime.
pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{}}
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
