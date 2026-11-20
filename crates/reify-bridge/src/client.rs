//! Native access to the companion's live, revocable credentials on loopback.
use reify_core::{ReifyError, domain::AuthContext};
use serde::Deserialize;
use url::Url;

/// Pooled native client; credentials and bearer tokens never implement Debug.
#[derive(Clone)]
pub struct BridgeClient {
    client: reqwest::Client,
    token: String,
}
impl BridgeClient {
    /// Enable companion integration when REIFY_BRIDGE_TOKEN is configured.
    pub fn from_env() -> Result<Option<Self>, ReifyError> {
        std::env::var("REIFY_BRIDGE_TOKEN")
            .ok()
            .map(Self::new)
            .transpose()
    }
    /// Connect to the fixed IPv4 loopback companion service on port 9099.
    pub fn new(token: String) -> Result<Self, ReifyError> {
        if token.len() < 16
            || !token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-._~".contains(&byte))
        {
            return Err(ReifyError::ExecutionError(
                "bridge token requires at least 16 safe ASCII characters".into(),
            ));
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .map_err(|_| ReifyError::ExecutionError("cannot construct companion client".into()))?;
        Ok(Self { client, token })
    }
    /// Read current credentials for exactly one origin. Revoked/expired sessions
    /// return None; unavailable or unauthorized bridges fail without stale fallback.
    pub async fn auth_for(&self, url: &Url) -> Result<Option<AuthContext>, ReifyError> {
        let failed = || {
            ReifyError::ExecutionError(
                "cannot read companion session; run reify bridge with the same REIFY_BRIDGE_TOKEN"
                    .into(),
            )
        };
        let mut response = self
            .client
            .get("http://127.0.0.1:9099/session/auth")
            .bearer_auth(&self.token)
            .query(&[("origin", url.origin().ascii_serialization())])
            .send()
            .await
            .map_err(|_| failed())?
            .error_for_status()
            .map_err(|_| failed())?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| failed())? {
            if bytes.len() + chunk.len() > 1024 * 1024 {
                return Err(failed());
            }
            bytes.extend_from_slice(&chunk);
        }
        #[derive(Deserialize)]
        struct Session {
            auth: Option<AuthContext>,
        }
        let session: Session = serde_json::from_slice(&bytes).map_err(|_| failed())?;
        Ok(session.auth)
    }
}
