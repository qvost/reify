//! First-party credential extraction. Credentials never enter diagnostic logs.
use chrono::{DateTime, Utc};
use reify_core::domain::AuthContext;
use serde::Deserialize;
use std::collections::HashMap;
use url::Url;
/// Cookie fields returned by Network.getCookies.
#[derive(Deserialize)]
pub struct BrowserCookie {
    /// Cookie name.
    pub name: String,
    /// Cookie value.
    pub value: String,
    /// Host or dot-prefixed domain.
    pub domain: String,
    /// Expiry in epoch seconds; negative means session lifetime.
    #[serde(default)]
    pub expires: f64,
}
/// Stateful extraction for one probe; observe only already-scoped traffic.
#[derive(Default)]
pub struct AuthExtractor {
    context: AuthContext,
}
impl AuthExtractor {
    /// Merge Authorization, common API keys, and CSRF headers using lowercase keys.
    pub fn observe_headers(&mut self, headers: &HashMap<String, String>, request: bool) {
        for (name, value) in headers {
            if request && name.eq_ignore_ascii_case("authorization") {
                self.context.authorization_header = Some(value.clone());
            }
            if let Some(key) = ["x-csrf-token", "x-xsrf-token", "csrf-token"]
                .into_iter()
                .find(|key| name.eq_ignore_ascii_case(key))
            {
                self.context
                    .csrf_tokens
                    .insert(key.to_owned(), value.clone());
            }
            if request
                && let Some(key) = [
                    "x-api-key",
                    "api-key",
                    "apikey",
                    "x-auth-token",
                    "x-access-token",
                ]
                .into_iter()
                .find(|key| name.eq_ignore_ascii_case(key))
            {
                self.context
                    .custom_headers
                    .insert(key.to_owned(), value.clone());
            }
        }
    }
    /// Merge domain-matching cookies and conservatively select earliest finite expiry.
    pub fn cookies(&mut self, target: &Url, cookies: impl IntoIterator<Item = BrowserCookie>) {
        let Some(host) = target.host_str() else {
            return;
        };
        for cookie in cookies {
            let domain = cookie.domain.trim_start_matches('.').to_ascii_lowercase();
            let ancestor_cookie =
                host == domain || host.strip_suffix(&domain).is_some_and(|s| s.ends_with('.'));
            let subdomain_cookie = target.domain().is_some()
                && domain.strip_suffix(host).is_some_and(|s| s.ends_with('.'));
            if domain.is_empty() || !(ancestor_cookie || subdomain_cookie) {
                continue;
            }
            let expiry = if cookie.expires.is_finite() && cookie.expires > 0.0 {
                DateTime::from_timestamp(cookie.expires as i64, 0)
            } else {
                None
            };
            if expiry.is_some_and(|e| e <= Utc::now()) {
                continue;
            }
            if matches!(
                cookie.name.to_ascii_lowercase().as_str(),
                "csrftoken" | "xsrf-token" | "_csrf"
            ) {
                self.context
                    .csrf_tokens
                    .insert(cookie.name.clone(), cookie.value.clone());
            }
            if let Some(expiry) = expiry {
                self.context.expires_at = Some(
                    self.context
                        .expires_at
                        .map_or(expiry, |old| old.min(expiry)),
                );
            }
            self.context.cookies.insert(cookie.name, cookie.value);
        }
    }
    /// Finish the per-probe credential bundle.
    pub fn finish(self) -> AuthContext {
        self.context
    }
}
