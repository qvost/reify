//! Bounded HTTP hydration-state extraction. No JavaScript is evaluated.
//!
//! Frames are JSON projections of the fetched HTML, identified by
//! `x-reify-ssr-state`; their URL is the real page URL, not an invented API route.
//! Nuxt reference tables remain in their original JSON representation.
use crate::{
    auth::{AuthExtractor, BrowserCookie},
    engine::ProbeOutput,
    filter,
};
use chrono::{DateTime, Utc};
use memchr::memchr;
use reify_core::domain::{CapturedFrame, ResourceType, Target};
use reqwest::{Client, Response, header};
use serde_json::Value;
use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
use url::Url;

const MAX_HTML: usize = 8 * 1024 * 1024;
const MAX_STATE: usize = 1024 * 1024;
const MARKERS: [&str; 4] = [
    "__NEXT_DATA__",
    "__NUXT_DATA__",
    "__remixContext",
    "__INITIAL_STATE__",
];
const ACCEPT: &str = "text/html";

/// One complete JSON state value, in document order (at most one per marker).
#[derive(Debug)]
pub struct EmbeddedState {
    /// Framework script ID or global variable name.
    pub marker: &'static str,
    /// Original object or array; no framework-specific reference decoding.
    pub value: Value,
}
/// Reusable HTTP pool with no shared cookie jar or browser state.
#[derive(Clone, Default)]
pub struct SsrProbe {
    client: Arc<OnceLock<Option<Client>>>,
}
impl SsrProbe {
    /// Attempt a single page fetch with up to three first-party redirects.
    /// Any network/decoding/limit failure is a miss. The engine applies a shorter
    /// budget within the overall deadline; direct callers have a 500ms deadline.
    pub async fn probe(&self, target: &Target) -> Option<ProbeOutput> {
        let started = Instant::now();
        let mut output = tokio::time::timeout(Duration::from_millis(500), self.fetch(target))
            .await
            .ok()??;
        output.duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        Some(output)
    }
    async fn fetch(&self, target: &Target) -> Option<ProbeOutput> {
        if !matches!(target.url.scheme(), "http" | "https")
            || target.url.host_str().is_none()
            || !target.url.username().is_empty()
            || target.url.password().is_some()
        {
            return None;
        }
        let client = self
            .client
            .get_or_init(|| {
                Client::builder()
                    .no_proxy()
                    .no_gzip()
                    .no_brotli()
                    .no_deflate()
                    .no_zstd()
                    .redirect(reqwest::redirect::Policy::none())
                    .connect_timeout(Duration::from_millis(500))
                    .timeout(Duration::from_millis(500))
                    .build()
                    .ok()
            })
            .as_ref()?;
        fetch_page(client, target, |request| client.execute(request)).await
    }
}
async fn fetch_page<F>(
    client: &Client,
    target: &Target,
    mut send: impl FnMut(reqwest::Request) -> F,
) -> Option<ProbeOutput>
where
    F: std::future::Future<Output = Result<Response, reqwest::Error>>,
{
    let mut url = target.url.clone();
    for redirects in 0..=3 {
        let request = client
            .get(url.clone())
            .header(header::ACCEPT, ACCEPT)
            // Avoid a decompressor/decompression-bomb path. Unsupported
            // servers that ignore identity fall back to the browser.
            .header(header::ACCEPT_ENCODING, "identity")
            .build()
            .ok()?;
        let response = send(request).await.ok()?;
        if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            // A redirect that establishes credentials needs a real browser
            // cookie jar. Never silently discard that authentication step.
            if redirects == 3 || response.headers().contains_key(header::SET_COOKIE) {
                return None;
            }
            let next = url
                .join(response.headers().get(header::LOCATION)?.to_str().ok()?)
                .ok()?;
            if !redirect_allowed(&target.url, &url, &next) {
                return None;
            }
            url = next;
            continue;
        }
        return from_response(response).await;
    }
    None
}
fn redirect_allowed(target: &Url, previous: &Url, next: &Url) -> bool {
    filter::is_first_party(target, next)
        && !(previous.scheme() == "https" && next.scheme() != "https")
        && next.username().is_empty()
        && next.password().is_none()
}
async fn from_response(mut response: Response) -> Option<ProbeOutput> {
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|n| n > MAX_HTML as u64)
        || response
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .is_some_and(|n| n > MAX_HTML as u64)
        || response
            .headers()
            .get(header::CONTENT_ENCODING)
            .is_some_and(|v| !v.as_bytes().eq_ignore_ascii_case(b"identity"))
    {
        return None;
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)?
        .to_str()
        .ok()?;
    let mime = content_type.split(';').next()?.trim();
    if !mime.eq_ignore_ascii_case("text/html") {
        return None;
    }
    // Only UTF-8/ASCII are parsed: replacing invalid bytes could silently change
    // identifiers, tokens, or script boundaries.
    for parameter in content_type.split(';').skip(1) {
        if let Some((name, value)) = parameter.trim().split_once('=')
            && name.trim().eq_ignore_ascii_case("charset")
            && !["utf-8", "utf8", "us-ascii"].iter().any(|v| {
                value
                    .trim()
                    .trim_matches(['\'', '"'])
                    .eq_ignore_ascii_case(v)
            })
        {
            return None;
        }
    }
    let mut bytes =
        Vec::with_capacity(response.content_length().unwrap_or(0).min(MAX_HTML as u64) as usize);
    while let Some(chunk) = response.chunk().await.ok()? {
        if chunk.len() > MAX_HTML - bytes.len() {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    let html = std::str::from_utf8(&bytes).ok()?;
    let (states, title) = parse_html(html);
    if states.is_empty() {
        return None;
    }
    let mut response_headers: HashMap<String, String> = response
        .headers()
        .iter()
        .filter_map(|(key, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (key.to_string(), value.to_owned()))
        })
        .collect();
    let url = response.url().clone();
    let mut auth = AuthExtractor::default();
    auth.observe_headers(&response_headers, false);
    auth.cookies(&url, response_cookies(&response, &url));
    let auth = auth.finish();
    auth.to_headers().ok()?;
    // Make the projection explicit while retaining the original HTTP metadata.
    // Synth's existing JSON admission check can consume these derived frames.
    if let Some(original) =
        response_headers.insert("content-type".into(), "application/json".into())
    {
        response_headers.insert("x-reify-source-content-type".into(), original);
    }
    let timestamp = Utc::now();
    let last = states.len() - 1;
    let frames = states
        .into_iter()
        .enumerate()
        .map(|(index, state)| {
            let mut headers = if index == last {
                std::mem::take(&mut response_headers)
            } else {
                response_headers.clone()
            };
            headers.insert("x-reify-ssr-state".into(), state.marker.into());
            CapturedFrame {
                id: uuid::Uuid::new_v4(),
                url: url.clone(),
                method: "GET".into(),
                request_headers: HashMap::from([
                    ("accept".into(), ACCEPT.into()),
                    ("accept-encoding".into(), "identity".into()),
                ]),
                request_body: None,
                status_code: response.status().as_u16(),
                response_headers: headers,
                response_body: Some(state.value),
                timestamp,
                resource_type: ResourceType::Other,
            }
        })
        .collect();
    Some(ProbeOutput {
        frames,
        auth,
        final_url: url,
        page_title: title,
        duration_ms: 0,
    })
}
fn response_cookies(response: &Response, url: &Url) -> Vec<BrowserCookie> {
    let mut cookies = Vec::new();
    for header in response.headers().get_all(header::SET_COOKIE) {
        let Ok(header) = header.to_str() else {
            continue;
        };
        let mut parts = header.split(';');
        let Some((name, value)) = parts.next().and_then(|p| p.split_once('=')) else {
            continue;
        };
        let mut domain = url.host_str().unwrap_or("");
        let mut path = url
            .path()
            .rsplit_once('/')
            .map_or("/", |(p, _)| if p.is_empty() { "/" } else { p });
        let mut secure = false;
        let mut max_age = None;
        let mut expiry = None;
        for part in parts {
            let (key, value) = part.trim().split_once('=').unwrap_or((part.trim(), ""));
            if key.eq_ignore_ascii_case("domain") {
                domain = value.trim().trim_start_matches('.');
            } else if key.eq_ignore_ascii_case("path") && value.starts_with('/') {
                path = value;
            } else if key.eq_ignore_ascii_case("secure") {
                secure = true;
            } else if key.eq_ignore_ascii_case("max-age") {
                max_age = value.parse::<i64>().ok();
            } else if key.eq_ignore_ascii_case("expires") {
                expiry = DateTime::parse_from_rfc2822(value)
                    .ok()
                    .map(|d| d.timestamp());
            }
        }
        let host = url.host_str().unwrap_or("");
        let applicable_domain = host.eq_ignore_ascii_case(domain)
            || (url.domain().is_some()
                && host.len() > domain.len()
                && host[host.len() - domain.len()..].eq_ignore_ascii_case(domain)
                && host.as_bytes()[host.len() - domain.len() - 1] == b'.');
        let applicable_path = url.path() == path
            || url
                .path()
                .strip_prefix(path)
                .is_some_and(|tail| path.ends_with('/') || tail.starts_with('/'));
        if domain.is_empty()
            || !applicable_domain
            || !applicable_path
            || (secure && url.scheme() != "https")
        {
            continue;
        }
        if let Some(age) = max_age {
            if age <= 0 {
                cookies.retain(|c: &BrowserCookie| c.name != name.trim());
                continue;
            }
            expiry = Some(
                Utc::now()
                    .timestamp()
                    .saturating_add(age)
                    .min(DateTime::<Utc>::MAX_UTC.timestamp()),
            );
        }
        if expiry.is_some_and(|at| at <= Utc::now().timestamp()) {
            cookies.retain(|c: &BrowserCookie| c.name != name.trim());
            continue;
        }
        cookies.push(BrowserCookie {
            name: name.trim().into(),
            value: value.trim().into(),
            domain: host.into(),
            expires: expiry.map_or(-1.0, |e| e as f64),
        });
    }
    cookies
}

/// Extract supported nonempty JSON objects/arrays from inline scripts, without
/// executing code or interpreting JavaScript object literals. HTML/state sizes
/// are bounded to 8 MiB/1 MiB. Strings, comments, templates, and external scripts
/// do not count as state. One value per marker is returned in document order.
pub fn extract_states(html: &str) -> Vec<EmbeddedState> {
    parse_html(html).0
}

struct Tag<'a> {
    name: &'a str,
    attrs: &'a str,
    closing: bool,
    end: usize,
}
fn space(b: u8) -> bool {
    matches!(b, b' ' | b'\n' | b'\r' | b'\t' | 0x0c)
}
fn tag(html: &str, start: usize) -> Option<Tag<'_>> {
    let bytes = html.as_bytes();
    let mut i = start + 1;
    let closing = bytes.get(i) == Some(&b'/');
    i += usize::from(closing);
    let name_start = i;
    if !bytes.get(i)?.is_ascii_alphabetic() {
        return None;
    }
    while bytes
        .get(i)
        .is_some_and(|b| !space(*b) && !matches!(b, b'>' | b'/'))
    {
        i += 1;
    }
    let name = &html[name_start..i];
    let attrs_start = i;
    let mut quote = 0;
    while let Some(&b) = bytes.get(i) {
        if quote != 0 {
            if b == quote {
                quote = 0;
            }
        } else if matches!(b, b'\'' | b'"') {
            quote = b;
        } else if b == b'>' {
            return Some(Tag {
                name,
                attrs: &html[attrs_start..i],
                closing,
                end: i + 1,
            });
        }
        i += 1;
    }
    None
}
fn attribute<'a>(mut attrs: &'a str, wanted: &str) -> Option<&'a str> {
    while !attrs.is_empty() {
        attrs = attrs.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == '/');
        let end = attrs
            .find(|c: char| c.is_ascii_whitespace() || c == '=')
            .unwrap_or(attrs.len());
        if end == 0 {
            return None;
        }
        let name = &attrs[..end];
        attrs = attrs[end..].trim_start_matches(|c: char| c.is_ascii_whitespace());
        let mut value = "";
        if let Some(rest) = attrs.strip_prefix('=') {
            attrs = rest.trim_start_matches(|c: char| c.is_ascii_whitespace());
            if let Some(quote @ (b'\'' | b'"')) = attrs.as_bytes().first() {
                let end = memchr(*quote, &attrs.as_bytes()[1..])? + 1;
                value = &attrs[1..end];
                attrs = &attrs[end + 1..];
            } else {
                let end = attrs
                    .find(|c: char| c.is_ascii_whitespace())
                    .unwrap_or(attrs.len());
                value = &attrs[..end];
                attrs = &attrs[end..];
            }
        }
        if name.eq_ignore_ascii_case(wanted) {
            return Some(value);
        }
    }
    None
}
fn raw_end(html: &str, mut from: usize, name: &str) -> Option<usize> {
    while let Some(offset) = memchr(b'<', &html.as_bytes()[from..]) {
        let at = from + offset;
        let rest = &html.as_bytes()[at + 1..];
        if rest.first() == Some(&b'/')
            && rest
                .get(1..1 + name.len())
                .is_some_and(|s| s.eq_ignore_ascii_case(name.as_bytes()))
            && rest
                .get(1 + name.len())
                .is_some_and(|b| space(*b) || matches!(b, b'>' | b'/'))
        {
            return Some(at);
        }
        from = at + 1;
    }
    None
}
fn parse_html(html: &str) -> (Vec<EmbeddedState>, Option<String>) {
    let mut states = Vec::new();
    let mut title = None;
    if html.len() > MAX_HTML {
        return (states, title);
    }
    let mut cursor = 0;
    let mut templates = 0usize;
    while let Some(offset) = memchr(b'<', &html.as_bytes()[cursor..]) {
        let at = cursor + offset;
        if html[at..].starts_with("<!--") {
            let Some(end) = html[at + 4..].find("-->") else {
                break;
            };
            cursor = at + 4 + end + 3;
            continue;
        }
        let Some(tag) = tag(html, at) else {
            // An unterminated real tag consumes the remaining input. Rescanning
            // each '<' in it could otherwise make malformed HTML quadratic.
            let first = html.as_bytes().get(at + 1).copied();
            if first.is_some_and(|b| b.is_ascii_alphabetic() || b == b'/') {
                break;
            }
            if first == Some(b'!') || first == Some(b'?') {
                let Some(end) = memchr(b'>', &html.as_bytes()[at + 1..]) else {
                    break;
                };
                cursor = at + 1 + end + 1;
                continue;
            }
            cursor = at + 1;
            continue;
        };
        cursor = tag.end;
        if tag.name.eq_ignore_ascii_case("template") {
            if tag.closing {
                templates = templates.saturating_sub(1);
            } else {
                templates += 1;
            }
        }
        if tag.closing {
            continue;
        }
        if tag.name.eq_ignore_ascii_case("plaintext") {
            break;
        }
        let raw = [
            "script", "style", "textarea", "title", "xmp", "iframe", "noembed", "noframes",
            "noscript",
        ]
        .iter()
        .any(|name| tag.name.eq_ignore_ascii_case(name));
        if !raw {
            continue;
        }
        let Some(end) = raw_end(html, cursor, tag.name) else {
            break;
        };
        let text = &html[cursor..end];
        cursor = end;
        if templates > 0 {
            continue;
        }
        if tag.name.eq_ignore_ascii_case("title") && title.is_none() {
            let value = entities(text.trim());
            if !value.is_empty() {
                title = Some(value.into_owned());
            }
        }
        if !tag.name.eq_ignore_ascii_case("script") || attribute(tag.attrs, "src").is_some() {
            continue;
        }
        if let Some(marker) =
            attribute(tag.attrs, "id").and_then(|id| MARKERS.iter().find(|m| **m == id))
        {
            if text.len() <= MAX_STATE
                && let Ok(value) = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            {
                push_state(&mut states, marker, value);
            }
        } else {
            let kind = attribute(tag.attrs, "type").unwrap_or("");
            if ["", "text/javascript", "application/javascript", "module"]
                .iter()
                .any(|s| s.eq_ignore_ascii_case(kind))
            {
                assignments(text, &mut states);
            }
        }
    }
    (states, title)
}
fn push_state(states: &mut Vec<EmbeddedState>, marker: &'static str, value: Value) {
    if (matches!(&value, Value::Object(v) if !v.is_empty())
        || matches!(&value, Value::Array(v) if !v.is_empty()))
        && !states.iter().any(|s| s.marker == marker)
    {
        states.push(EmbeddedState { marker, value });
    }
}
fn entities(text: &str) -> Cow<'_, str> {
    if !text.contains('&') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let decoded = rest.as_bytes()[..rest.len().min(13)]
            .iter()
            .position(|b| *b == b';')
            .and_then(|end| {
                let name = &rest[1..end];
                let c = match name {
                    "amp" => '&',
                    "lt" => '<',
                    "gt" => '>',
                    "quot" => '"',
                    "apos" | "#39" => '\'',
                    "nbsp" => '\u{a0}',
                    _ => {
                        let digits = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X"));
                        let n = if let Some(digits) = digits {
                            u32::from_str_radix(digits, 16).ok()?
                        } else {
                            name.strip_prefix('#')?.parse().ok()?
                        };
                        char::from_u32(n)?
                    }
                };
                Some((end + 1, c))
            });
        if let Some((len, c)) = decoded {
            out.push(c);
            rest = &rest[len..];
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}
fn whitespace_comments(mut s: &str) -> Option<&str> {
    loop {
        s = s.trim_start();
        if let Some(rest) = s
            .strip_prefix("//")
            .or_else(|| s.strip_prefix("<!--"))
            .or_else(|| s.strip_prefix("-->"))
        {
            s = rest.split_once('\n').map_or("", |(_, rest)| rest);
        } else if let Some(rest) = s.strip_prefix("/*") {
            s = rest.split_once("*/")?.1;
        } else {
            return Some(s);
        }
    }
}
fn identifier(s: &str) -> (&str, &str) {
    let end = s
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .unwrap_or(s.len());
    (&s[..end], &s[end..])
}
fn assignment(s: &str) -> Option<(&'static str, Value, &str)> {
    let (mut name, mut rest) = identifier(s);
    if matches!(name, "var" | "let" | "const") {
        (name, rest) = identifier(whitespace_comments(rest)?);
    }
    if matches!(name, "window" | "self" | "globalThis") {
        rest = whitespace_comments(rest)?.strip_prefix('.')?;
        (name, rest) = identifier(whitespace_comments(rest)?);
    }
    let marker = MARKERS.iter().find(|m| **m == name)?;
    rest = whitespace_comments(whitespace_comments(rest)?.strip_prefix('=')?)?;
    let encoded = rest.strip_prefix("JSON.parse");
    let input = if let Some(encoded) = encoded {
        whitespace_comments(encoded)?
            .strip_prefix('(')?
            .trim_start()
    } else {
        rest
    };
    let input_bytes = input.as_bytes();
    let mut stream =
        serde_json::Deserializer::from_slice(&input_bytes[..input.len().min(MAX_STATE)])
            .into_iter::<Value>();
    let value = stream.next()?.ok()?;
    let used = stream.byte_offset();
    if used > MAX_STATE {
        return None;
    }
    rest = whitespace_comments(&input[used..])?;
    let value = if encoded.is_some() {
        rest = whitespace_comments(rest.strip_prefix(')')?)?;
        let value = value.as_str()?;
        if value.len() > MAX_STATE {
            return None;
        }
        serde_json::from_str(value).ok()?
    } else {
        value
    };
    if !rest.is_empty() && !rest.starts_with(';') {
        return None;
    }
    Some((marker, value, rest))
}
fn assignments(mut script: &str, states: &mut Vec<EmbeddedState>) {
    // Most inline application bundles contain no hydration marker. Skip their
    // JavaScript lexing with a vectorized search over the borrowed script text.
    if !MARKERS
        .iter()
        .any(|marker| memchr::memmem::find(script.as_bytes(), marker.as_bytes()).is_some())
    {
        return;
    }
    let mut depth = 0usize;
    let mut statement_start = true;
    while let Some(s) = whitespace_comments(script).filter(|s| !s.is_empty()) {
        script = s;
        if statement_start
            && depth == 0
            && let Some((marker, value, rest)) = assignment(script)
        {
            push_state(states, marker, value);
            script = rest;
            continue;
        }
        statement_start = false;
        let byte = script.as_bytes()[0];
        if matches!(byte, b'\'' | b'"') {
            let mut end = 1;
            while end < script.len() {
                match script.as_bytes()[end] {
                    b'\\' => end += 2,
                    b if b == byte => {
                        end += 1;
                        break;
                    }
                    _ => end += 1,
                }
            }
            script = &script[end.min(script.len())..];
            continue;
        }
        // Templates and regexp/division require a JavaScript parser. Stop this
        // script conservatively rather than mistake their text for an assignment.
        if matches!(byte, b'`' | b'/') {
            break;
        }
        match byte {
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => depth = depth.saturating_sub(1),
            b';' if depth == 0 => statement_start = true,
            _ => {}
        }
        script = &script[script.chars().next().expect("nonempty script").len_utf8()..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::ResponseBuilderExt;
    use serde_json::json;

    pub(super) fn response(body: impl Into<reqwest::Body>) -> Response {
        http::Response::builder()
            .url(Url::parse("https://example.com/catalog/items").unwrap())
            .header("content-type", "text/html; charset=utf-8")
            .body(body.into())
            .unwrap()
            .into()
    }
    #[test]
    fn extracts_all_formats_in_document_order_without_changing_json_text() {
        let html = r#"<!doctype html><title>Products &amp; &#x4E16;&#30028;</title>
            <SCRIPT data-x='>' TYPE="application/json" ID='__NEXT_DATA__'>
              {"props":{"pageProps":{"items":[{"id":1,"text":"&amp; { } \\\""}]}}}
            </SCRIPT >
            <script type="application/json" id=__NUXT_DATA__>[{"data":1}, {"items":2}, [3], {"id":4}, 1]</script>
            <script>window.__remixContext = {"state":{"loaderData":{"items":[1,2]}}}; window.other = 1;</script>
            <script>/* state follows */ const __INITIAL_STATE__ = {"items":[null,true,3.5], "unicode":"世界"};</script>"#;
        let (states, title) = parse_html(html);
        assert_eq!(states.iter().map(|s| s.marker).collect::<Vec<_>>(), MARKERS);
        assert_eq!(
            states[0].value["props"]["pageProps"]["items"][0]["text"],
            "&amp; { } \\\""
        );
        assert_eq!(states[1].value[0], json!({"data":1}));
        assert_eq!(
            states[2].value["state"]["loaderData"]["items"],
            json!([1, 2])
        );
        assert_eq!(
            states[3].value,
            json!({"items":[null,true,3.5],"unicode":"世界"})
        );
        assert_eq!(title.as_deref(), Some("Products & 世界"));
    }
    #[test]
    fn assignment_lexer_handles_comments_strings_and_json_parse() {
        let encoded = serde_json::to_string(
            &json!({"items":["</scriptx>", "雪", "{\"quoted\"}"]}).to_string(),
        )
        .unwrap();
        let html = format!(
            r#"<script>const ignored = "__INITIAL_STATE__ = {{\"bad\":true}}";
            // __NEXT_DATA__ = {{"bad":true}};
            /* __NUXT_DATA__ = ["bad"]; */
            self . __INITIAL_STATE__ /* ok */ = JSON.parse({encoded});</script>"#
        );
        let states = extract_states(&html);
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].marker, "__INITIAL_STATE__");
        assert_eq!(states[0].value["items"][1], "雪");
        assert_eq!(
            extract_states(
                r#"<script>globalThis.__NEXT_DATA__={"props":1};window.__NUXT_DATA__=[1];</script>"#
            )
            .len(),
            2
        );
        assert!(
            extract_states(r#"<script>window.__INITIAL_STATE__={"bad":1} || fallback;</script>"#)
                .is_empty()
        );
    }
    #[test]
    fn ignores_inactive_external_incomplete_and_non_json_state() {
        for html in [
            r#"<!-- <script id="__NEXT_DATA__">{"bad":1}</script> -->"#,
            r#"<div title='<script id="__NEXT_DATA__">{"bad":1}</script>'></div>"#,
            r#"<template><template><script id="__NEXT_DATA__">{"bad":1}</script></template></template>"#,
            r#"<textarea><script id="__NEXT_DATA__">{"bad":1}</script></textarea>"#,
            r#"<style><script id="__NEXT_DATA__">{"bad":1}</script></style>"#,
            r#"<noscript><script id="__NEXT_DATA__">{"bad":1}</script></noscript>"#,
            r#"<script src="/state.js" id="__NEXT_DATA__">{"bad":1}</script>"#,
            r#"<script type="text/plain">window.__INITIAL_STATE__={"bad":1};</script>"#,
            r#"<script>function unused(){window.__INITIAL_STATE__={"bad":1};}</script>"#,
            r#"<script>const literal = `;window.__INITIAL_STATE__={"bad":1};`;</script>"#,
            r#"<script>const regex = /;window.__INITIAL_STATE__={"bad":1};/;</script>"#,
            r#"<script><!-- ignored; window.__INITIAL_STATE__={"bad":1}; --></script>"#,
            r#"<script id="__NEXT_DATA__">{"bad":1}</scriptx>"#,
            r#"<script id="__NEXT_DATA__">{"bad":1}"#,
            r#"<script>window.__INITIAL_STATE__ = {unquoted:1};</script>"#,
            r#"<script>window.__INITIAL_STATE__ = JSON.parse('{"bad":1}');</script>"#,
            r#"<script>window.__INITIAL_STATE__ = fetch('/data');</script>"#,
        ] {
            assert!(
                extract_states(html).is_empty(),
                "unexpected state from {html}"
            );
        }
        for value in [
            "null",
            "true",
            "42",
            "\"text\"",
            "{}",
            "[]",
            "{",
            "{\"x\":1,}",
        ] {
            assert!(
                extract_states(&format!("<script id=__NEXT_DATA__>{value}</script>")).is_empty()
            );
        }
        let states =
            extract_states(r#"<template></template><script id=__NEXT_DATA__>{"good":1}</script>"#);
        assert_eq!(states.len(), 1);
    }
    #[test]
    fn limits_payloads_nesting_and_duplicates() {
        let state = json!({"large":"x".repeat(MAX_STATE)}).to_string();
        assert!(extract_states(&format!("<script id=__NEXT_DATA__>{state}</script>")).is_empty());
        assert!(
            extract_states(&format!(
                "<script>window.__INITIAL_STATE__={state};</script>"
            ))
            .is_empty()
        );
        assert!(extract_states(&"x".repeat(MAX_HTML + 1)).is_empty());
        let deep = format!("{}0{}", "[".repeat(130), "]".repeat(130));
        assert!(extract_states(&format!("<script id=__NUXT_DATA__>{deep}</script>")).is_empty());
        let states = extract_states(
            r#"<script id=__NEXT_DATA__>{"first":1}</script><script id=__NEXT_DATA__>{"second":2}</script>"#,
        );
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].value, json!({"first":1}));
        // Long malformed tags and entity-heavy titles must not trigger rescans.
        assert!(extract_states(&format!("<div title='{}", "<".repeat(10000))).is_empty());
        let (_, title) = parse_html(&format!("<title>{}</title>", "&".repeat(10000)));
        assert_eq!(title.unwrap().len(), 10000);
    }
    #[tokio::test]
    async fn response_projects_state_with_real_url_metadata_and_auth() {
        let mut response =
            response(r#"<title>Items</title><script id="__NEXT_DATA__">{"items":[1,2]}</script>"#);
        response.headers_mut().append(
            header::SET_COOKIE,
            "session=abc; Path=/; Secure; HttpOnly".parse().unwrap(),
        );
        response.headers_mut().append(
            header::SET_COOKIE,
            "csrftoken=csrf; Max-Age=60; Path=/".parse().unwrap(),
        );
        response.headers_mut().append(
            header::SET_COOKIE,
            "wrong=bad; Domain=evil.com".parse().unwrap(),
        );
        response
            .headers_mut()
            .append(header::SET_COOKIE, "path=bad; Path=/other".parse().unwrap());
        response.headers_mut().append(
            header::SET_COOKIE,
            "old=bad; Expires=Thu, 01 Jan 1970 00:00:01 GMT"
                .parse()
                .unwrap(),
        );
        response
            .headers_mut()
            .insert("x-csrf-token", "header-csrf".parse().unwrap());
        let output = from_response(response).await.unwrap();
        assert_eq!(output.page_title.as_deref(), Some("Items"));
        assert_eq!(
            output.final_url.as_str(),
            "https://example.com/catalog/items"
        );
        assert_eq!(output.frames.len(), 1);
        let frame = &output.frames[0];
        assert_eq!(frame.url, output.final_url);
        assert_eq!(frame.method, "GET");
        assert_eq!(frame.resource_type, ResourceType::Other);
        assert!(frame.is_json_response());
        assert_eq!(frame.response_body, Some(json!({"items":[1,2]})));
        assert_eq!(frame.response_headers["x-reify-ssr-state"], "__NEXT_DATA__");
        assert_eq!(
            frame.response_headers["x-reify-source-content-type"],
            "text/html; charset=utf-8"
        );
        assert_eq!(
            output.auth.cookies,
            HashMap::from([
                ("session".into(), "abc".into()),
                ("csrftoken".into(), "csrf".into())
            ])
        );
        assert_eq!(output.auth.csrf_tokens["x-csrf-token"], "header-csrf");
        assert_eq!(output.auth.csrf_tokens["csrftoken"], "csrf");
        assert!(output.auth.expires_at.is_some());
    }
    #[tokio::test]
    async fn invalid_http_responses_are_misses_and_cookies_obey_scope_and_deletion() {
        let body = r#"<script id=__NEXT_DATA__>{"data":1}</script>"#;
        for (name, value) in [
            ("content-type", "application/json"),
            ("content-type", "text/html; charset=iso-8859-1"),
            ("content-encoding", "gzip"),
            ("content-length", "9000000"),
        ] {
            let mut response = response(body);
            response.headers_mut().insert(
                header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
            assert!(
                from_response(response).await.is_none(),
                "accepted {name}: {value}"
            );
        }
        let forbidden: Response = http::Response::builder()
            .status(403)
            .header("content-type", "text/html")
            .body(body)
            .unwrap()
            .into();
        assert!(from_response(forbidden).await.is_none());
        assert!(from_response(response(vec![0xff])).await.is_none());
        assert!(
            from_response(response(vec![b'x'; MAX_HTML + 1]))
                .await
                .is_none()
        );
        assert!(
            from_response(response("<html>client-only</html>"))
                .await
                .is_none()
        );
        let mut response = self::response(body);
        for cookie in [
            "dead=one; Path=/",
            "dead=two; Max-Age=0; Path=/",
            "insecure=ok; Secure; Path=/",
            "wrong=bad; Path=/catalogue",
            "ancestor=ok; Domain=.EXAMPLE.COM; Path=/catalog",
            "hostile=bad; Domain=notexample.com",
        ] {
            response
                .headers_mut()
                .append(header::SET_COOKIE, cookie.parse().unwrap());
        }
        let output = from_response(response).await.unwrap();
        assert!(!output.auth.cookies.contains_key("dead"));
        assert!(!output.auth.cookies.contains_key("wrong"));
        assert!(!output.auth.cookies.contains_key("hostile"));
        assert_eq!(output.auth.cookies["ancestor"], "ok");
    }
    #[test]
    fn redirects_preserve_first_party_and_transport_boundaries() {
        let target = Url::parse("https://example.com").unwrap();
        for next in ["https://example.com/page", "https://www.example.com/page"] {
            assert!(redirect_allowed(
                &target,
                &target,
                &Url::parse(next).unwrap()
            ));
        }
        for next in [
            "https://example.com.evil.org",
            "https://other.com",
            "http://example.com",
            "https://user:secret@example.com",
            "ftp://example.com",
        ] {
            assert!(!redirect_allowed(
                &target,
                &target,
                &Url::parse(next).unwrap()
            ));
        }
    }
    fn streamed(chunks: Vec<Result<bytes::Bytes, std::io::Error>>) -> Response {
        let frames = chunks
            .into_iter()
            .map(|chunk| chunk.map(http_body::Frame::data));
        response(reqwest::Body::wrap(http_body_util::StreamBody::new(
            futures_util::stream::iter(frames),
        )))
    }
    #[tokio::test]
    async fn reads_split_state_and_rejects_oversized_or_failed_streams() {
        let output = from_response(streamed(vec![
            Ok(bytes::Bytes::from_static(b"<script id=__NEXT_")),
            Ok(bytes::Bytes::from_static(
                b"DATA__>{\"items\":[1]}</script>",
            )),
        ]))
        .await
        .unwrap();
        assert_eq!(output.frames[0].response_body, Some(json!({"items":[1]})));
        assert!(
            from_response(streamed(vec![
                Ok(bytes::Bytes::from(vec![b'x'; MAX_HTML])),
                Ok(bytes::Bytes::from_static(b"x")),
            ]))
            .await
            .is_none()
        );
        assert!(
            from_response(streamed(vec![Err(std::io::Error::other(
                "upstream disconnected"
            ))]))
            .await
            .is_none()
        );
    }
    #[tokio::test]
    async fn http_dispatch_builds_real_get_requests_and_follows_only_safe_redirects() {
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let target = Target::new("https://example.com/start", None).unwrap();
        let mut requests = Vec::new();
        let mut step = 0;
        let output = fetch_page(&client, &target, |request| {
            assert_eq!(request.method(), reqwest::Method::GET);
            assert_eq!(request.headers()[header::ACCEPT], ACCEPT);
            assert_eq!(request.headers()[header::ACCEPT_ENCODING], "identity");
            assert!(!request.headers().contains_key(header::COOKIE));
            assert!(request.body().is_none());
            requests.push(request.url().clone());
            let response = if step == 0 {
                http::Response::builder()
                    .url(request.url().clone())
                    .status(302)
                    .header("location", "/catalog/items")
                    .body("")
                    .unwrap()
                    .into()
            } else {
                response(r#"<script id=__NEXT_DATA__>{"items":[1]}</script>"#)
            };
            step += 1;
            std::future::ready(Ok(response))
        })
        .await
        .unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].as_str(), "https://example.com/catalog/items");
        assert_eq!(output.final_url, requests[1]);
        assert_eq!(output.frames[0].response_body, Some(json!({"items":[1]})));
        for (location, cookie, expected_calls) in [
            ("https://other.com/", false, 1),
            ("http://example.com/", false, 1),
            ("/login", true, 1),
            ("/loop", false, 4),
        ] {
            let mut calls = 0;
            let miss = fetch_page(&client, &target, |request| {
                calls += 1;
                let mut response = http::Response::builder()
                    .url(request.url().clone())
                    .status(302)
                    .header("location", location);
                if cookie {
                    response = response.header("set-cookie", "session=secret");
                }
                std::future::ready(Ok(response.body("").unwrap().into()))
            })
            .await;
            assert!(miss.is_none());
            assert_eq!(calls, expected_calls);
        }
    }
}
