//! Immutable snapshots keep cache locks short and make conditional swaps atomic.
use reify_core::{
    cache::CacheKey,
    domain::{AuthContext, CompiledTool, Target},
};
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::RwLock;

/// One immutable compiled application. Debug is omitted because auth contains secrets.
#[derive(Clone)]
pub struct CachedApp {
    /// Target that produced this application.
    pub target: Target,
    /// Compiled tool plans.
    pub tools: Vec<CompiledTool>,
    /// Per-application upstream credentials.
    pub auth: AuthContext,
    /// Monotonic compilation time.
    pub created_at: Instant,
    /// Maximum freshness duration.
    pub ttl: Duration,
}
impl CachedApp {
    pub(crate) fn valid_for(&self, key: CacheKey) -> bool {
        let mut names = std::collections::HashSet::new();
        CacheKey::from_target(&self.target) == key
            && !self.tools.is_empty()
            && self.tools.iter().all(|tool| {
                tool.definition.name != "reify"
                    && names.insert(tool.definition.name.clone())
                    && tool.definition.validate().is_ok()
                    && crate::parameter_headers::validate(
                        &tool.definition.input_schema,
                        &serde_json::json!({}),
                        &http::HeaderMap::new(),
                    )
                    .is_ok()
            })
    }
    /// Remaining cache freshness, also capped by credential expiry.
    pub fn remaining(&self) -> Duration {
        let ttl = self.ttl.saturating_sub(self.created_at.elapsed());
        self.auth.expires_at.map_or(ttl, |expiry| {
            ttl.min((expiry - chrono::Utc::now()).to_std().unwrap_or_default())
        })
    }
}
/// Generation-bound runtime data. Validators and wire definitions are prepared
/// once outside the map lock; no process-wide cache can retain old credentials.
pub struct PreparedApp {
    /// The exact immutable generation described by the prepared data.
    pub app: Arc<CachedApp>,
    pub(crate) definitions: Vec<Box<serde_json::value::RawValue>>,
    pub(crate) renderers: Vec<Result<reify_synth::compiler::PreparedTool, String>>,
    pub(crate) mirrors: Vec<Result<crate::parameter_headers::ParameterMirrors, ()>>,
    pub(crate) headers: Result<http::HeaderMap, String>,
}
#[derive(Clone)]
struct PrewarmedParts {
    tool: CompiledTool,
    definition: Box<serde_json::value::RawValue>,
    renderer: reify_synth::compiler::PreparedTool,
    mirrors: crate::parameter_headers::ParameterMirrors,
}
static PREWARMED_RENDERERS: OnceLock<HashMap<String, Result<PrewarmedParts, String>>> =
    OnceLock::new();

/// Compile and retain validators for the embedded catalogs during server startup.
pub fn warm_profiles() -> Result<(), reify_core::ReifyError> {
    reify_core::profiles::warm()?;
    let renderers = PREWARMED_RENDERERS.get_or_init(|| {
        let mut renderers = HashMap::new();
        for profile in reify_core::profiles::PROFILES {
            if let Ok(Some(tools)) = reify_core::profiles::compile_tools(profile, None) {
                for tool in tools {
                    let prepared = (|| {
                        Ok(PrewarmedParts {
                            tool: tool.clone(),
                            definition: tool
                                .definition
                                .to_mcp_json()
                                .map_err(|error| error.to_string())?,
                            renderer: reify_synth::compiler::PreparedTool::new(&tool)
                                .map_err(|error| error.to_string())?,
                            mirrors: crate::parameter_headers::ParameterMirrors::new(
                                &tool.definition.input_schema,
                            )
                            .map_err(|_| "invalid pre-warmed parameter schema".to_owned())?,
                        })
                    })();
                    renderers.insert(tool.definition.name.clone(), prepared);
                }
            }
        }
        renderers
    });
    if renderers.len()
        != reify_core::profiles::PROFILES
            .iter()
            .map(|profile| profile.tools.len())
            .sum::<usize>()
    {
        return Err(reify_core::ReifyError::SchemaValidationError(
            "cannot prepare embedded profile validators".into(),
        ));
    }
    if let Some(error) = renderers
        .values()
        .find_map(|renderer| renderer.as_ref().err())
    {
        return Err(reify_core::ReifyError::SchemaValidationError(error.clone()));
    }
    Ok(())
}
impl PreparedApp {
    fn new(app: Arc<CachedApp>) -> Result<Self, String> {
        let prewarmed = app.tools.iter().all(|tool| {
            tool.definition
                .metadata
                .as_ref()
                .is_some_and(|metadata| metadata.tags.iter().any(|tag| tag == "prewarmed"))
        });
        let (definitions, renderers, mirrors) = if prewarmed {
            let renderers = PREWARMED_RENDERERS
                .get()
                .ok_or_else(|| "pre-warmed profile validators were not initialized".to_owned())?;
            let parts = app
                .tools
                .iter()
                .map(|tool| {
                    renderers
                        .get(&tool.definition.name)
                        .filter(|part| part.as_ref().is_ok_and(|part| part.tool == *tool))
                        .cloned()
                        .ok_or_else(|| "missing pre-warmed renderer".to_owned())?
                })
                .collect::<Result<Vec<_>, String>>()?;
            (
                parts.iter().map(|part| part.definition.clone()).collect(),
                parts.iter().map(|part| Ok(part.renderer.clone())).collect(),
                parts.iter().map(|part| Ok(part.mirrors.clone())).collect(),
            )
        } else {
            let definitions = app
                .tools
                .iter()
                .map(|tool| tool.definition.to_mcp_json().map_err(|e| e.to_string()))
                .collect::<Result<_, _>>()?;
            let renderers = app
                .tools
                .iter()
                .map(|tool| {
                    reify_synth::compiler::PreparedTool::new(tool).map_err(|e| e.to_string())
                })
                .collect();
            let mirrors = app
                .tools
                .iter()
                .map(|tool| {
                    crate::parameter_headers::ParameterMirrors::new(&tool.definition.input_schema)
                })
                .collect();
            (definitions, renderers, mirrors)
        };
        let headers = app.auth.to_headers().map_err(|e| e.to_string());
        Ok(Self {
            app,
            definitions,
            renderers,
            mirrors,
            headers,
        })
    }
    /// Return a prepared renderer for one tool without rebuilding its schema.
    pub fn renderer(
        &self,
        index: usize,
    ) -> Result<&reify_synth::compiler::PreparedTool, reify_core::ReifyError> {
        self.renderers
            .get(index)
            .ok_or_else(|| {
                reify_core::ReifyError::ExecutionError("tool index is not in this mount".into())
            })?
            .as_ref()
            .map_err(|e| reify_core::ReifyError::SchemaValidationError(e.clone()))
    }
}
struct StoredApp {
    app: Arc<CachedApp>,
    prepared: Result<Arc<PreparedApp>, String>,
}
impl StoredApp {
    async fn build(app: CachedApp) -> Arc<Self> {
        let app = Arc::new(app);
        let prewarmed = app.tools.iter().all(|tool| {
            tool.definition
                .metadata
                .as_ref()
                .is_some_and(|metadata| metadata.tags.iter().any(|tag| tag == "prewarmed"))
        });
        if prewarmed {
            return Arc::new(Self {
                prepared: PreparedApp::new(app.clone()).map(Arc::new),
                app,
            });
        }
        let source = app.clone();
        let prepared = tokio::task::spawn_blocking(move || PreparedApp::new(source).map(Arc::new))
            .await
            .unwrap_or_else(|_| Err("cache preparation worker failed".into()));
        Arc::new(Self { app, prepared })
    }
}
/// Shared store. Readers clone immutable snapshots under a short Tokio read lock.
#[derive(Clone, Default)]
pub struct ServerStore {
    entries: Arc<RwLock<HashMap<CacheKey, Arc<StoredApp>>>>,
}
impl ServerStore {
    /// Construct an empty store.
    pub fn new() -> Self {
        Self::default()
    }
    /// Read a fresh application as an owned copy. Hot paths use prepared snapshots.
    pub async fn get(&self, key: &CacheKey) -> Option<CachedApp> {
        self.snapshot(key).await.map(|app| (*app).clone())
    }
    /// Read a fresh app without copying schemas/credentials.
    pub async fn snapshot(&self, key: &CacheKey) -> Option<Arc<CachedApp>> {
        self.current(key)
            .await
            .filter(|app| !app.remaining().is_zero())
    }
    /// Read generation-bound validators, header plans, and serialized definitions.
    pub async fn prepared(
        &self,
        key: &CacheKey,
    ) -> Result<Option<Arc<PreparedApp>>, reify_core::ReifyError> {
        let entry = self.entries.read().await.get(key).cloned();
        match entry {
            Some(entry) if !entry.app.remaining().is_zero() => entry
                .prepared
                .as_ref()
                .map(|p| Some(p.clone()))
                .map_err(|e| reify_core::ReifyError::SchemaValidationError(e.clone())),
            _ => Ok(None),
        }
    }
    /// Read the installed generation, including stale generations for recovery.
    pub async fn current(&self, key: &CacheKey) -> Option<Arc<CachedApp>> {
        self.entries
            .read()
            .await
            .get(key)
            .map(|entry| entry.app.clone())
    }
    /// Prepare off the async executor and install atomically. Expired entries are pruned.
    pub async fn insert(&self, key: CacheKey, app: CachedApp) {
        let entry = StoredApp::build(app).await;
        // Keep a reference to retired generations until after unlocking: dropping
        // their validator/schema graphs can otherwise stall every cache reader.
        let mut retired = Vec::new();
        let mut entries = self.entries.write().await;
        entries.retain(|_, entry| {
            if entry.app.remaining().is_zero() {
                retired.push(entry.clone());
                false
            } else {
                true
            }
        });
        let previous = entries.insert(key, entry);
        drop(entries);
        drop(previous);
        drop(retired);
    }
    /// Remove a generation so in-flight healers cannot resurrect it.
    pub async fn invalidate(&self, key: &CacheKey) {
        let previous = self.entries.write().await.remove(key);
        drop(previous);
    }
    /// Replace only the generation that triggered recovery; newer writes win.
    pub async fn replace_if_current(
        &self,
        key: CacheKey,
        expected: &Arc<CachedApp>,
        app: CachedApp,
    ) -> bool {
        if !self
            .current(&key)
            .await
            .is_some_and(|app| Arc::ptr_eq(&app, expected))
        {
            return false;
        }
        let entry = StoredApp::build(app).await;
        let mut entries = self.entries.write().await;
        if entries
            .get(&key)
            .is_some_and(|entry| Arc::ptr_eq(&entry.app, expected))
        {
            let previous = entries.insert(key, entry);
            drop(entries);
            drop(previous);
            true
        } else {
            drop(entries);
            false
        }
    }
}
