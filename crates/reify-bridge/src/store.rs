//! Exact-host credential storage. No browser databases or OS credential stores are read.
use reify_core::{ReifyError, domain::AuthContext};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use url::Host;
/// Public session metadata; credential values are never serialized here.
#[derive(serde::Serialize)]
pub struct SessionInfo {
    /// Exact browser origin authorized by the user.
    pub origin: String,
    /// Time at which the shared session expires.
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}
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
        self.insert_key(domain, auth).await
    }
    async fn insert_key(&self, key: String, auth: AuthContext) -> Result<(), ReifyError> {
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
        let previous = self.entries.write().await.insert(
            key,
            Arc::new(Session {
                auth,
                headers: Arc::new(headers),
            }),
        );
        drop(previous);
        Ok(())
    }
    /// Store credentials for one exact HTTP(S) origin, including scheme and port.
    pub async fn insert_origin(
        &self,
        origin: &url::Url,
        auth: AuthContext,
    ) -> Result<(), ReifyError> {
        self.insert_key(origin.origin().ascii_serialization(), auth)
            .await
    }
    /// Read a live browser-origin bundle without falling back to legacy host keys.
    pub async fn auth_for_origin(&self, origin: &url::Url) -> Option<AuthContext> {
        let key = origin.origin().ascii_serialization();
        let entry = self.entries.read().await.get(&key).cloned()?;
        entry
            .auth
            .expires_at
            .is_none_or(|expiry| expiry > chrono::Utc::now())
            .then(|| entry.auth.clone())
    }
    /// List live browser sessions, removing expired credentials from memory.
    pub async fn origins(&self) -> Vec<SessionInfo> {
        let mut entries = self.entries.write().await;
        entries.retain(|_, session| {
            session
                .auth
                .expires_at
                .is_none_or(|expiry| expiry > chrono::Utc::now())
        });
        let mut origins: Vec<_> = entries
            .iter()
            .filter(|(key, _)| key.contains("://"))
            .map(|(origin, session)| SessionInfo {
                origin: origin.clone(),
                expires_at: session.auth.expires_at,
            })
            .collect();
        origins.sort_by(|a, b| a.origin.cmp(&b.origin));
        origins
    }
    /// Revoke only the explicitly shared browser origin.
    pub async fn remove_origin(&self, origin: &url::Url) {
        self.entries
            .write()
            .await
            .remove(&origin.origin().ascii_serialization());
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
        let entries = self.entries.read().await;
        let entry = entries
            .get(&url.origin().ascii_serialization())
            .or_else(|| entries.get(host))
            .cloned()?;
        entry
            .auth
            .expires_at
            .is_none_or(|expiry| expiry > chrono::Utc::now())
            .then(|| entry.headers.clone())
    }
    /// Remove a configured bundle.
    pub async fn remove(&self, domain: &str) -> Result<(), ReifyError> {
        let domain = normalize_domain(domain)?;
        let previous = self.entries.write().await.remove(&domain);
        drop(previous);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn prepared_headers_keep_exact_host_scope_and_replacement_semantics() {
        let store = SessionStore::new();
        let context = AuthContext {
            authorization_header: Some("Bearer first".into()),
            ..Default::default()
        };
        store.insert("EXAMPLE.com.", context).await.unwrap();
        let first = store
            .headers_for(&url::Url::parse("https://example.com./api").unwrap())
            .await
            .unwrap();
        let second = store
            .headers_for(&url::Url::parse("https://example.com").unwrap())
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first["authorization"], "Bearer first");
        assert!(
            store
                .headers_for(&url::Url::parse("https://api.example.com").unwrap())
                .await
                .is_none()
        );
        store
            .insert(
                "example.com",
                AuthContext {
                    authorization_header: Some("Bearer second".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let updated = store
            .headers_for(&url::Url::parse("https://example.com").unwrap())
            .await
            .unwrap();
        assert_eq!(updated["authorization"], "Bearer second");
        assert_eq!(first["authorization"], "Bearer first");
        store.insert("[::1]", AuthContext::default()).await.unwrap();
        assert!(
            store
                .headers_for(&url::Url::parse("http://[::1]:9090").unwrap())
                .await
                .is_some()
        );
    }
}
