//! Deterministic compilation keys scoped to canonical host/port and normalized goal.
use crate::domain::{Target, normalize_goal};
use std::{borrow::Cow, fmt};
/// A BLAKE3 digest stored inline as exactly 32 bytes, without heap allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CacheKey([u8; 32]);
impl CacheKey {
    /// Hash a versioned, length-prefixed domain and goal tuple. Paths, queries,
    /// fragments, credentials, and scheme are deliberately excluded. Nondefault
    /// ports remain part of the authority. This is a compilation key, not an
    /// authenticated response-cache key; callers must partition private results.
    pub fn from_target(target: &Target) -> Self {
        let raw = target.url.host_str().unwrap_or("");
        let host = if raw.bytes().any(|b| b.is_ascii_uppercase()) {
            Cow::Owned(raw.to_ascii_lowercase())
        } else {
            Cow::Borrowed(raw)
        };
        let domain = match target.url.port() {
            Some(port) => Cow::Owned(format!("{host}:{port}")),
            None => host,
        };
        let goal = normalize_goal(target.goal.as_deref().unwrap_or(""));
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"reify:target:v1\0");
        for part in [domain.as_bytes(), goal.as_bytes()] {
            hasher.update(&(part.len() as u64).to_le_bytes());
            hasher.update(part);
        }
        Self(*hasher.finalize().as_bytes())
    }
    /// Lowercase, 64-character hexadecimal representation.
    pub fn to_hex(&self) -> String {
        blake3::Hash::from_bytes(self.0).to_hex().to_string()
    }
    /// Borrow the exact 256-bit digest.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}
impl fmt::Display for CacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", blake3::Hash::from_bytes(self.0).to_hex())
    }
}
