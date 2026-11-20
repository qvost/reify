//! Server construction, production application builder, and per-target compilation gates.
use crate::{
    failure, handler,
    healing::SelfHealingCoordinator,
    proxy::UpstreamProxy,
    store::{CachedApp, ServerStore},
};
use reify_core::{ReifyError, cache::CacheKey, domain::Target};
use reify_probe::ProbeEngine;
use reify_synth::SynthEngine;
use std::{
    collections::HashMap,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};

/// Injectable asynchronous compiler boundary; production and fixture builders use
/// the same mount and healing paths. No external mocking runtime is required.
pub trait AppBuilder: Send + Sync + 'static {
    /// Probe and synthesize an isolated application, returning no partial writes.
    fn build(
        &self,
        target: Target,
    ) -> Pin<Box<dyn Future<Output = Result<CachedApp, ReifyError>> + Send + '_>>;
}
/// Production Chromium probe plus deterministic synth compiler.
pub struct BrowserAppBuilder {
    probe: Arc<ProbeEngine>,
    synth: Arc<SynthEngine>,
    slots: Semaphore,
}
impl BrowserAppBuilder {
    /// Limit concurrent browser processes to two across bootstrap and recovery.
    pub fn new(probe: Arc<ProbeEngine>, synth: Arc<SynthEngine>) -> Self {
        Self {
            probe,
            synth,
            slots: Semaphore::new(2),
        }
    }
}
impl AppBuilder for BrowserAppBuilder {
    fn build(
        &self,
        target: Target,
    ) -> Pin<Box<dyn Future<Output = Result<CachedApp, ReifyError>> + Send + '_>> {
        Box::pin(async move {
            let _permit = self
                .slots
                .acquire()
                .await
                .map_err(|_| failure("compiler closed"))?;
            let output = self.probe.probe(&target, Duration::from_secs(30)).await?;
            let synth = self.synth.clone();
            tokio::task::spawn_blocking(move || {
                let scope = Target::from_url(output.final_url.clone(), target.goal.clone())?;
                let compiled = synth.synthesize(&scope, &output.frames, &output.auth)?;
                Ok(CachedApp {
                    target,
                    tools: compiled.tools,
                    auth: output.auth,
                    created_at: Instant::now(),
                    ttl: Duration::from_secs(300),
                })
            })
            .await
            .map_err(|_| failure("synthesis worker failed"))?
        })
    }
}
#[derive(Default)]
pub(crate) struct MountRegistry {
    by_key: HashMap<CacheKey, Arc<str>>,
    by_token: HashMap<Arc<str>, CacheKey>,
}
pub(crate) struct State {
    pub store: ServerStore,
    pub builder: Arc<dyn AppBuilder>,
    pub healing: SelfHealingCoordinator,
    pub proxy: std::sync::OnceLock<UpstreamProxy>,
    pub gates: Mutex<HashMap<CacheKey, Weak<AsyncMutex<()>>>>,
    pub mounts: Mutex<MountRegistry>,
    pub origins: Vec<String>,
}
impl State {
    pub async fn mount(&self, target: Target) -> Result<MountedApp, ReifyError> {
        tokio::time::timeout(Duration::from_secs(45), self.mount_inner(target))
            .await
            .map_err(|_| failure("website compilation timed out"))?
    }
    async fn mount_inner(&self, target: Target) -> Result<MountedApp, ReifyError> {
        let target = Target::from_url(target.url, target.goal)?;
        if !target.url.username().is_empty() || target.url.password().is_some() {
            return Err(failure("URL credentials are not supported"));
        }
        let key = CacheKey::from_target(&target);
        if let Some(app) = self.store.snapshot(&key).await {
            return Ok(MountedApp {
                key,
                token: self.token(key),
                app,
                cache_hit: true,
            });
        }
        let gate = self.gate(key);
        let _guard = gate.lock().await;
        if let Some(app) = self.store.snapshot(&key).await {
            return Ok(MountedApp {
                key,
                token: self.token(key),
                app,
                cache_hit: true,
            });
        }
        let built = self.builder.build(target).await?;
        if !built.valid_for(key) {
            return Err(failure("compiler returned an invalid tool mount"));
        }
        self.store.insert(key, built).await;
        let app = self
            .store
            .snapshot(&key)
            .await
            .ok_or_else(|| failure("compiled credentials are already expired"))?;
        Ok(MountedApp {
            key,
            token: self.token(key),
            app,
            cache_hit: false,
        })
    }
    pub fn gate(&self, key: CacheKey) -> Arc<AsyncMutex<()>> {
        let mut gates = self.gates.lock().expect("mount gate mutex");
        if let Some(gate) = gates.get(&key).and_then(Weak::upgrade) {
            return gate;
        }
        gates.retain(|_, gate| gate.strong_count() > 0);
        let gate = Arc::new(AsyncMutex::new(()));
        gates.insert(key, Arc::downgrade(&gate));
        gate
    }
    pub fn token(&self, key: CacheKey) -> String {
        let mut registry = self.mounts.lock().expect("mount registry mutex");
        if let Some(token) = registry.by_key.get(&key) {
            return token.to_string();
        }
        let token: Arc<str> = uuid::Uuid::new_v4().to_string().into();
        registry.by_key.insert(key, token.clone());
        registry.by_token.insert(token.clone(), key);
        token.to_string()
    }
    pub fn resolve(&self, token: &str) -> Option<CacheKey> {
        // Every minted token is a hyphenated UUID. Reject oversized metadata
        // before hashing it while holding the registry lock.
        if token.len() != 36 {
            return None;
        }
        self.mounts
            .lock()
            .expect("mount registry mutex")
            .by_token
            .get(token)
            .copied()
    }
}
/// Immutable result of resolving or compiling a mounted application.
#[derive(Clone)]
pub struct MountedApp {
    /// Domain/goal compilation identity.
    pub key: CacheKey,
    /// Opaque per-server routing token for MCP request metadata.
    pub token: String,
    /// Shared application snapshot.
    pub app: Arc<CachedApp>,
    /// True when no browser compilation was needed by this request.
    pub cache_hit: bool,
}
/// Clonable server configuration. Each instance is one trusted principal's cache.
#[derive(Clone)]
pub struct ReifyServer {
    pub(crate) state: Arc<State>,
}
impl ReifyServer {
    /// Construct the production server without starting any background tasks.
    pub fn new(probe: Arc<ProbeEngine>, synth: Arc<SynthEngine>) -> Self {
        Self::with_builder(Arc::new(BrowserAppBuilder::new(probe, synth)))
    }
    /// Construct with an alternate real/fixture application builder.
    pub fn with_builder(builder: Arc<dyn AppBuilder>) -> Self {
        let store = ServerStore::new();
        let healing =
            SelfHealingCoordinator::new(store.clone(), builder.clone(), Duration::from_secs(30));
        Self {
            state: Arc::new(State {
                store,
                builder,
                healing,
                proxy: std::sync::OnceLock::new(),
                gates: Mutex::new(HashMap::new()),
                mounts: Mutex::new(MountRegistry::default()),
                origins: vec![],
            }),
        }
    }
    /// Permit browser Origin values. Default rejects every present Origin.
    /// Configure before cloning/constructing a router; shared state is immutable.
    pub fn with_allowed_origins(mut self, origins: Vec<String>) -> Result<Self, ReifyError> {
        let state = Arc::get_mut(&mut self.state)
            .ok_or_else(|| failure("configure origins before sharing the server"))?;
        for origin in &origins {
            let url = url::Url::parse(origin).map_err(|_| failure("invalid allowed origin"))?;
            if url.origin().ascii_serialization() != *origin
                || !matches!(url.scheme(), "http" | "https")
            {
                return Err(failure("allowed origins must be canonical HTTP origins"));
            }
        }
        state.origins = origins;
        Ok(self)
    }
    /// Resolve a BLAKE3 cache entry or compile it once, sharing the HTTP handler path.
    pub async fn mount(&self, target: Target) -> Result<MountedApp, ReifyError> {
        self.state.mount(target).await
    }
    /// Serve a pre-bound listener and drain requests after an explicit shutdown.
    pub async fn serve_listener<F>(
        &self,
        listener: tokio::net::TcpListener,
        shutdown: F,
    ) -> Result<(), ReifyError>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        axum::serve(listener, self.router())
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(|_| failure("MCP HTTP server failed"))
    }
    /// Store handle for preloading compiled applications and inspecting generations.
    pub fn store(&self) -> ServerStore {
        self.state.store.clone()
    }
    /// Construct an in-process router; it can be nested behind host authentication.
    pub fn router(&self) -> axum::Router {
        axum::Router::new()
            .route("/mcp", axum::routing::post(handler::handle))
            .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024))
            .with_state(self.state.clone())
    }
    /// Bind and serve until task cancellation or listener failure.
    pub async fn serve(&self, addr: SocketAddr) -> Result<(), ReifyError> {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|_| failure("cannot bind MCP listener"))?;
        axum::serve(listener, self.router())
            .await
            .map_err(|_| failure("MCP HTTP server failed"))
    }
}
