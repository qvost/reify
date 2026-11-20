//! One bounded attempt to excite a safe discovery control in the current page.
use crate::cdp::CdpClient;
use reify_core::ReifyError;
use serde_json::json;
use url::Url;

/// The discovery action dispatched by the page inspection script.
#[derive(Debug)]
pub struct ActionResult {
    /// Explicit search form destination, used to scope subsequent API capture.
    pub destination: Option<Url>,
}

fn words(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
}

/// Whether the task explicitly calls for discovery through user interaction.
pub fn is_interactive(goal: Option<&str>) -> bool {
    goal.is_some_and(|goal| {
        words(goal).any(|word| {
            [
                "search",
                "searching",
                "query",
                "find",
                "filter",
                "filtering",
                "lookup",
            ]
            .iter()
            .any(|action| word.eq_ignore_ascii_case(action))
        })
    })
}

fn probe_term(goal: Option<&str>) -> String {
    let term = words(goal.unwrap_or_default())
        .filter(|word| {
            ![
                "search",
                "searching",
                "query",
                "find",
                "filter",
                "filtering",
                "lookup",
                "for",
                "the",
                "a",
                "an",
                "please",
                "me",
                "show",
                "get",
            ]
            .iter()
            .any(|stop| word.eq_ignore_ascii_case(stop))
        })
        .collect::<Vec<_>>()
        .join(" ");
    if term.is_empty() {
        "test".into()
    } else {
        term.chars().take(64).collect()
    }
}

/// Inspect the main document and excite at most one safe search control.
/// Missing controls and page script errors are benign misses. Transport errors
/// remain errors; the caller owns the bounded network observation window.
pub async fn excite(
    cdp: &CdpClient,
    session: &str,
    goal: Option<&str>,
) -> Result<Option<ActionResult>, ReifyError> {
    let expression = format!(
        "({})({})",
        include_str!("action.js"),
        json!(probe_term(goal))
    );
    let response = cdp
        .command(
            "Runtime.evaluate",
            json!({
                "expression": expression, "returnByValue": true,
            }),
            Some(session),
        )
        .await?;
    if response.get("exceptionDetails").is_some() {
        tracing::debug!("search excitation skipped: page script evaluation failed");
        return Ok(None);
    }
    let value = &response["result"]["value"];
    if value["acted"].as_bool() != Some(true) {
        tracing::debug!("search excitation skipped: no safe discovery control");
        return Ok(None);
    }
    let destination = value["destination"]
        .as_str()
        .and_then(|s| Url::parse(s).ok())
        .filter(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.username().is_empty()
                && url.password().is_none()
        });
    tracing::debug!("dispatched one search excitation");
    Ok(Some(ActionResult { destination }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_terms_are_bounded_data_with_a_benign_default() {
        assert_eq!(probe_term(None), "test");
        assert_eq!(probe_term(Some("please search for the")), "test");
        assert_eq!(probe_term(Some("find café stories")), "café stories");
        assert_eq!(probe_term(Some(&"猫".repeat(100))).chars().count(), 64);
        assert!(!probe_term(Some("find x');fetch('/bad');//")).contains(['\'', ';', '(', ')']));
    }
}
