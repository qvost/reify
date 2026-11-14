//! Bounded drift worker: queued -> rebuilding -> cooldown; failed builds retain old state.
use crate::{server::AppBuilder, store::ServerStore};
use reify_core::{cache::CacheKey, mcp::ContentBlock};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::mpsc, task::JoinSet, time::Instant};

/// Passive evidence of upstream authentication or endpoint/schema changes.
#[derive(Debug, Clone)]
pub enum DriftEvent {
    /// Authentication was rejected upstream.
    AuthDrift {
        /// Compiled application identity.
        key: CacheKey,
        /// Upstream status (401/403).
        status: u16,
    },
    /// Endpoint disappeared or explicitly rejected a schema/field.
    SchemaDrift {
        /// Compiled application identity.
        key: CacheKey,
        /// Upstream status (400/404).
        status: u16,
        /// Sanitized diagnostic category.
        reason: String,
    },
}
impl DriftEvent {
    /// Target key for deduplication.
    pub fn key(&self) -> CacheKey {
        match self {
            Self::AuthDrift { key, .. } | Self::SchemaDrift { key, .. } => *key,
        }
    }
}
/// Classify drift without treating ordinary validation/business-rule 400s as drift.
pub fn detect(key: CacheKey, status: u16, content: &[ContentBlock]) -> Option<DriftEvent> {
    match status {
        401 | 403 => Some(DriftEvent::AuthDrift { key, status }),
        404 => Some(DriftEvent::SchemaDrift {
            key,
            status,
            reason: "endpoint not found".into(),
        }),
        400 => {
            let text = content
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text } => text.clone(),
                    ContentBlock::Json { json } => json.to_string(),
                    _ => String::new(),
                })
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase();
            [
                "unknown field",
                "unrecognized field",
                "unexpected field",
                "unknown parameter",
                "unknown argument",
                "cannot query field",
                "schema mismatch",
                "schema validation",
                "missing required field",
            ]
            .iter()
            .any(|s| text.contains(s))
            .then(|| DriftEvent::SchemaDrift {
                key,
                status,
                reason: "upstream field/schema rejection".into(),
            })
        }
        _ => None,
    }
}
/// Lazy-started coordinator; constructing a server does not require a Tokio runtime.
#[derive(Clone)]
pub struct SelfHealingCoordinator {
    inner: Arc<Coordinator>,
}
struct Coordinator {
    sender: mpsc::Sender<DriftEvent>,
    receiver: Mutex<Option<mpsc::Receiver<DriftEvent>>>,
    store: ServerStore,
    builder: Arc<dyn AppBuilder>,
    cooldown: Duration,
    task: Mutex<Option<tokio::task::AbortHandle>>,
}
impl SelfHealingCoordinator {
    /// Create a 64-event queue and a canonical-authority cooldown.
    pub fn new(store: ServerStore, builder: Arc<dyn AppBuilder>, cooldown: Duration) -> Self {
        let (sender, receiver) = mpsc::channel(64);
        Self {
            inner: Arc::new(Coordinator {
                sender,
                receiver: Mutex::new(Some(receiver)),
                store,
                builder,
                cooldown,
                task: Mutex::new(None),
            }),
        }
    }
    /// Queue a drift observation without delaying the failed tool response.
    /// False means the bounded queue is full/closed; a later invocation may retry.
    pub fn submit(&self, event: DriftEvent) -> bool {
        if tokio::runtime::Handle::try_current().is_err() {
            return false;
        }
        if let Some(receiver) = self
            .inner
            .receiver
            .lock()
            .expect("coordinator mutex")
            .take()
        {
            let store = self.inner.store.clone();
            let builder = self.inner.builder.clone();
            let cooldown = self.inner.cooldown;
            let task = tokio::spawn(worker(receiver, store, builder, cooldown));
            *self.inner.task.lock().expect("coordinator task mutex") = Some(task.abort_handle());
        }
        self.inner.sender.try_send(event).is_ok()
    }
}
async fn worker(
    mut receiver: mpsc::Receiver<DriftEvent>,
    store: ServerStore,
    builder: Arc<dyn AppBuilder>,
    cooldown: Duration,
) {
    let mut jobs = JoinSet::new();
    let mut recent: HashMap<String, Instant> = HashMap::new();
    let mut active = std::collections::HashSet::new();
    let mut identities = HashMap::new();
    let mut active_domains = std::collections::HashSet::new();
    loop {
        tokio::select! {
            event = receiver.recv(), if jobs.len() < 2 => {
                let Some(event) = event else { break; }; let key = event.key();
                recent.retain(|_,at| at.elapsed() < cooldown);
                if active.contains(&key) { continue; }
                let Some(previous) = store.current(&key).await else { continue; };
                let domain = format!("{}:{:?}",previous.target.url.host_str().unwrap_or(""),previous.target.url.port());
                if active_domains.contains(&domain) || recent.contains_key(&domain) { continue; }
                active_domains.insert(domain.clone());
                active.insert(key); let builder = builder.clone(); let store = store.clone();
                let job = jobs.spawn(async move {
                    let result = tokio::time::timeout(Duration::from_secs(45),builder.build(previous.target.clone())).await;
                    if let Ok(Ok(updated)) = result && updated.valid_for(key) {
                        store.replace_if_current(key,&previous,updated).await;
                    }
                    key
                });
                identities.insert(job.id(),(key,domain));
            }
            Some(done) = jobs.join_next_with_id(), if !jobs.is_empty() => {
                let id = match &done { Ok((id,_)) => *id, Err(error) => error.id() };
                if let Some((key,domain)) = identities.remove(&id) { active.remove(&key); active_domains.remove(&domain); recent.insert(domain,Instant::now()); }
            }
        }
    }
    jobs.abort_all();
}

impl Drop for Coordinator {
    fn drop(&mut self) {
        if let Some(task) = self.task.get_mut().expect("coordinator task mutex").take() {
            task.abort();
        }
    }
}
