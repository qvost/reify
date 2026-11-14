//! Credential encoding shared by the gateway and local bridge.
use crate::ReifyError;
use http::{HeaderMap, HeaderName, HeaderValue};
fn failure(message: &str) -> ReifyError {
    ReifyError::HeaderParseError(message.into())
}
impl super::AuthContext {
    /// Build validated, sensitive HTTP credential headers without logging values.
    /// Conventional CSRF cookies map to header names. Reserved transport/body
    /// headers and malformed cookie octets are rejected.
    pub fn to_headers(&self) -> Result<HeaderMap, ReifyError> {
        let auth = self;
        let mut headers = HeaderMap::new();
        let mut set = |name: &str, value: &str| -> Result<(), ReifyError> {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| failure("invalid credential header name"))?;
            if matches!(
                name.as_str(),
                "host"
                    | "content-length"
                    | "transfer-encoding"
                    | "connection"
                    | "upgrade"
                    | "proxy-authorization"
                    | "content-type"
                    | "content-encoding"
                    | "te"
                    | "trailer"
                    | "keep-alive"
                    | "proxy-connection"
            ) {
                return Err(failure("reserved credential header"));
            }
            let mut value = HeaderValue::from_str(value)
                .map_err(|_| failure("invalid credential header value"))?;
            value.set_sensitive(true);
            headers.insert(name, value);
            Ok(())
        };
        let mut custom: Vec<_> = auth.custom_headers.iter().collect();
        custom.sort();
        for (key, value) in custom {
            set(key, value)?;
        }
        let mut csrf: Vec<_> = auth.csrf_tokens.iter().collect();
        csrf.sort();
        for (key, value) in csrf {
            let key = key.to_ascii_lowercase();
            let name = match key.as_str() {
                "csrftoken" | "_csrf" | "csrf-token" => "x-csrf-token",
                "xsrf-token" => "x-xsrf-token",
                _ => &key,
            };
            set(name, value)?;
        }
        if let Some(value) = &auth.authorization_header {
            set("authorization", value)?;
        }
        if !auth.cookies.is_empty() {
            let mut cookies: Vec<_> = auth.cookies.iter().collect();
            cookies.sort();
            let mut pairs = Vec::new();
            for (name, value) in cookies {
                if name.is_empty()
                    || !name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                    || !value.bytes().all(|b| {
                        b == 0x21
                            || (0x23..=0x2b).contains(&b)
                            || (0x2d..=0x3a).contains(&b)
                            || (0x3c..=0x5b).contains(&b)
                            || (0x5d..=0x7e).contains(&b)
                    })
                {
                    return Err(failure("invalid cookie name or value"));
                }
                pairs.push(format!("{name}={value}"));
            }
            set("cookie", &pairs.join("; "))?;
        }
        Ok(headers)
    }
}
