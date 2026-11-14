//! Diagnostic output intentionally excludes the standalone credential bundle.
use reify_probe::ProbeOutput;
use serde_json::Value;
/// Produce formatted-JSON-ready frames and navigation diagnostics. Raw captured
/// frame headers/bodies remain developer-visible, as requested by `reify probe`.
pub fn as_json(output: ProbeOutput) -> Value {
    serde_json::json!({"frames":output.frames,"final_url":output.final_url,"page_title":output.page_title,"duration_ms":output.duration_ms})
}

/// Borrow diagnostics directly so large captures can stream to stdout without a
/// second owned JSON tree. The standalone credential bundle is never serialized.
pub fn report(output: &ProbeOutput) -> impl serde::Serialize + '_ {
    #[derive(serde::Serialize)]
    struct Report<'a> {
        frames: &'a [reify_core::domain::CapturedFrame],
        final_url: &'a url::Url,
        page_title: &'a Option<String>,
        duration_ms: u64,
    }
    Report {
        frames: &output.frames,
        final_url: &output.final_url,
        page_title: &output.page_title,
        duration_ms: output.duration_ms,
    }
}
