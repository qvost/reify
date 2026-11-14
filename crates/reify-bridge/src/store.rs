//! Exact-host credential storage. No browser databases or OS credential stores are read.
use reify_core::{ReifyError, domain::AuthContext};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use url::Host;
/// Normalize a bare hostname/IPv4/bracketed IPv6 key, including IDNA.
pub fn normalize_domain(domain: &str) -> Result<String, ReifyError> {
    if domain.is_empty()
        || domain.trim() != domain
        || domain.contains(['/', '@', '?', '#'])
        || (domain.contains(':') && !domain.starts_with('['))
    {
        return Err(ReifyError::InvalidTargetUrl(
            "session keys must be bare domain names".into(),
        ));
    }
    Host::parse(domain.trim_end_matches('.'))
        .map(|host| host.to_string().to_ascii_lowercase())
        .map_err(|_| ReifyError::InvalidTargetUrl("invalid session domain".into()))
}
struct Session {
    auth: AuthContext,
    headers: Arc<HashMap<String, String>>,
}
/// Thread-safe, exact-host session-header bundles. No Debug implementation exposes secrets.
#[derive(Clone, Default)]
pub struct SessionStore {
    entries: Arc<RwLock<HashMap<String, Arc<Session>>>>,
}
impl SessionStore {
    /// Construct an empty store.
    pub fn new() -> Self {
        Self::default()
    }
    /// Validate and install a bundle. Host aliases/subdomains do not inherit it.
    pub async fn insert(&self, domain: &str, auth: AuthContext) -> Result<(), ReifyError> {
        let domain = normalize_domain(domain)?;
        let headers = auth
            .to_headers()?
            .iter()
            .map(|(name, value)| {
                Ok((
                    name.to_string(),
                    std::str::from_utf8(value.as_bytes())
                        .map_err(|_| {
                            ReifyError::HeaderParseError(
                                "configured credential is not UTF-8".into(),
                            )
                        })?
                        .to_owned(),
                ))
            })
            .collect::<Result<_, ReifyError>>()?;
        self.entries.write().await.insert(
            domain,
            Arc::new(Session {
                auth,
                headers: Arc::new(headers),
            }),
        );
        Ok(())
    }
    /// Read a nonexpired bundle. Invalid names and expired bundles return None.
    pub async fn get(&self, domain: &str) -> Option<AuthContext> {
        let domain = normalize_domain(domain).ok()?;
        let entry = self.entries.read().await.get(&domain).cloned()?;
        entry
            .auth
            .expires_at
            .is_none_or(|expiry| expiry > chrono::Utc::now())
            .then(|| entry.auth.clone())
    }
    /// Prepared headers for a validated URL: borrow its canonical host for lookup
    /// and avoid cloning/re-encoding the original credential maps on each forward.
    pub(crate) async fn headers_for(&self, url: &url::Url) -> Option<Arc<HashMap<String, String>>> {
        let host = url.host_str()?.trim_end_matches('.');
        let entry = self.entries.read().await.get(host).cloned()?;
        entry
            .auth
            .expires_at
            .is_none_or(|expiry| expiry > chrono::Utc::now())
            .then(|| entry.headers.clone())
    }
    /// Remove a configured bundle.
    pub async fn remove(&self, domain: &str) -> Result<(), ReifyError> {
        let domain = normalize_domain(domain)?;
        self.entries.write().await.remove(&domain);
        Ok(())
    }
}
