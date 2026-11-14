//! Conservative first-party scoping without unsafe registrable-domain guessing.
use reify_core::domain::{CapturedFrame, ResourceType};
use url::Url;
/// Host and its subdomains are first party. Sibling hosts are deliberately excluded.
pub fn is_first_party(target: &Url, candidate: &Url) -> bool {
    let (Some(a), Some(b)) = (target.host_str(), candidate.host_str()) else {
        return false;
    };
    matches!(candidate.scheme(), "http" | "https")
        && (a == b
            || (target.domain().is_some()
                && b.strip_suffix(a)
                    .is_some_and(|prefix| prefix.ends_with('.'))))
}
/// Discard static resources and known tracker URL substrings before retaining state.
pub fn is_noise(url: &Url, resource_type: &str) -> bool {
    if is_static_type(resource_type) {
        return true;
    }
    let lower = if url.as_str().bytes().any(|b| b.is_ascii_uppercase()) {
        std::borrow::Cow::Owned(url.as_str().to_ascii_lowercase())
    } else {
        std::borrow::Cow::Borrowed(url.as_str())
    };
    if [
        "google-analytics",
        "doubleclick",
        "segment.io",
        "sentry",
        "datadog",
        "clarity.ms",
        "mixpanel",
        "intercom",
        "hotjar",
        "facebook.net",
        "launchdarkly",
        "telemetry",
    ]
    .iter()
    .any(|s| lower.contains(s))
    {
        return true;
    }
    let path = url.path();
    [
        ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".ico", ".avif", ".css", ".woff",
        ".woff2", ".ttf", ".otf", ".mp4", ".webm", ".mp3", ".wav", ".ogg", ".m4a",
    ]
    .iter()
    .any(|s| {
        path.get(path.len().saturating_sub(s.len())..)
            .is_some_and(|end| end.eq_ignore_ascii_case(s))
    })
}
/// Retain API traffic by initiator, JSON/GraphQL MIME, or GraphQL query heuristics.
pub fn is_api(frame: &CapturedFrame, mime: &str) -> bool {
    if matches!(frame.resource_type, ResourceType::Xhr | ResourceType::Fetch) {
        return true;
    }
    let mime = mime.split(';').next().unwrap_or("").trim();
    ["application/json", "text/json", "application/graphql"]
        .iter()
        .any(|m| mime.eq_ignore_ascii_case(m))
        || mime
            .get(mime.len().saturating_sub(5)..)
            .is_some_and(|s| s.eq_ignore_ascii_case("+json"))
        || frame.is_graphql()
}
/// Classify static initiators before URL parsing or frame allocation.
pub fn is_static_type(resource_type: &str) -> bool {
    matches!(
        resource_type,
        "Image" | "Stylesheet" | "Font" | "Media" | "Script" | "Manifest" | "TextTrack"
    )
}
