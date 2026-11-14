//! Terminal output stays separate from machine-readable command JSON.
use crate::{CliError, Color, commands::CompileMetrics};
use colored::Colorize;
use reify_server::MountedApp;
use serde_json::Value;
use std::{
    collections::BTreeSet,
    io::{self, IsTerminal, Write},
};
/// Decide color use without modifying the process environment.
pub fn use_color(color: Color) -> bool {
    match color {
        Color::Always => true,
        Color::Never => false,
        Color::Auto => {
            io::stdout().is_terminal()
                && std::env::var_os("NO_COLOR").is_none()
                && std::env::var("TERM").is_ok_and(|term| term != "dumb")
        }
    }
}
fn accent(text: &str, color: bool) -> String {
    if color {
        text.cyan().bold().to_string()
    } else {
        text.to_owned()
    }
}
/// Remove terminal controls and directional overrides from untrusted site strings.
pub fn safe_terminal(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() || matches!(c,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}') {
                ' '
            } else {
                c
            }
        })
        .collect()
}
/// Compact parameter type, including union and format annotations.
pub fn parameter_type(schema: &Value) -> String {
    fn inner(schema: &Value, depth: usize) -> String {
        if depth > 4 {
            return "json".into();
        }
        if let Some(kind) = schema["type"].as_str() {
            return if let Some(format) = schema["format"].as_str() {
                format!("{kind}({format})")
            } else {
                kind.into()
            };
        }
        if let Some(kinds) = schema["type"].as_array() {
            return kinds
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" | ");
        }
        if let Some(branches) = schema["anyOf"].as_array() {
            return branches
                .iter()
                .map(|s| inner(s, depth + 1))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(" | ");
        }
        "json".into()
    }
    safe_terminal(&inner(schema, 0))
}
/// Configuration snippets for current Claude Code, Cursor, and OpenCode v2.
pub fn configuration(endpoint: &str) -> String {
    configuration_for_mount(endpoint, None)
}
/// Configuration with a static stateless routing header for a warmed mount.
pub fn configuration_for_mount(endpoint: &str, token: Option<&str>) -> String {
    let mut claude = serde_json::json!({"mcpServers":{"reify":{"type":"http","url":endpoint}}});
    let mut cursor = serde_json::json!({"mcpServers":{"reify":{"url":endpoint}}});
    let mut opencode = serde_json::json!({"mcp":{"servers":{"reify":{"type":"remote","url":endpoint,"protocol":"2026-07-28"}}}});
    if let Some(token) = token {
        let headers = serde_json::json!({"Reify-Mount":token});
        claude["mcpServers"]["reify"]["headers"] = headers.clone();
        cursor["mcpServers"]["reify"]["headers"] = headers.clone();
        opencode["mcp"]["servers"]["reify"]["headers"] = headers;
    }
    format!(
        "Claude Code · .mcp.json\n{}\n\nCursor · .cursor/mcp.json\n{}\n\nOpenCode v2 · opencode.json\n{}\n",
        serde_json::to_string_pretty(&claude).expect("JSON object"),
        serde_json::to_string_pretty(&cursor).expect("JSON object"),
        serde_json::to_string_pretty(&opencode).expect("JSON object")
    )
}
/// Gateway startup banner with an immediately usable endpoint.
pub fn gateway_banner(endpoint: &str, color: bool) -> String {
    format!(
        "{}\nStateless MCP 2026-07-28 · {}\nUse the reify meta-tool to compile a target. Ctrl+C to stop.\n\n{}",
        accent("Reify gateway", color),
        safe_terminal(endpoint),
        configuration(endpoint)
    )
}
/// Human-facing default-mode summary; credentials and captured scalar values are omitted.
pub fn mount_summary(
    mounted: &MountedApp,
    metrics: Option<&CompileMetrics>,
    endpoint: &str,
    color: bool,
) -> String {
    let mut output = format!("{}\n", accent("Reify · tools mounted", color));
    if mounted.cache_hit {
        output.push_str("Cache hit · browser capture skipped\n");
    } else if let Some(metrics) = metrics {
        output.push_str(&format!(
            "Captured {} API frames in {}ms · synthesis {}ms\n",
            metrics.frames, metrics.capture_ms, metrics.synthesis_ms
        ));
    }
    output.push_str(&format!("\nTools ({})\n", mounted.app.tools.len()));
    for tool in &mounted.app.tools {
        output.push_str(&format!(
            "  {}\n",
            accent(&safe_terminal(&tool.definition.name), color)
        ));
        let desc = safe_terminal(&tool.definition.description);
        let description: String = desc.chars().take(180).collect();
        output.push_str(&format!(
            "    {description}{}\n",
            if desc.chars().count() > 180 {
                "…"
            } else {
                ""
            }
        ));
        let schema = &tool.definition.input_schema;
        let mut parameters = Vec::new();
        if let Some(properties) = schema["properties"].as_object() {
            for (key, value) in properties {
                let required = schema["required"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|r| r.as_str() == Some(key)));
                parameters.push(format!(
                    "{}{}: {}",
                    safe_terminal(key),
                    if required { "" } else { "?" },
                    parameter_type(value)
                ));
            }
        }
        output.push_str(&format!(
            "    {}\n",
            if parameters.is_empty() {
                "No parameters".into()
            } else {
                parameters.join(", ")
            }
        ));
    }
    output.push_str(&format!("\nMCP    {}\nCache  {}\nMount  {}\n\nFor scoped tools/list and tools/call requests, set params._meta[\"reify/mount\"] to the mount token.\nCtrl+C to stop.\n\n{}",safe_terminal(endpoint),mounted.key,mounted.token,configuration_for_mount(endpoint,Some(&mounted.token))));
    output
}
/// Write a human-facing block without panicking on a closed stdout pipe.
pub fn write_stdout(text: &str) -> Result<(), CliError> {
    let mut out = io::stdout().lock();
    out.write_all(text.as_bytes())?;
    out.flush()?;
    Ok(())
}
/// Print a single pretty JSON document; status/progress never contaminates stdout.
pub fn write_json(value: &Value) -> Result<(), CliError> {
    write_serializable(value)
}
/// Serialize directly through a bounded stdout buffer instead of allocating the
/// whole pretty JSON document. Broken-pipe errors retain their normal IO kind.
pub fn write_serializable(value: &impl serde::Serialize) -> Result<(), CliError> {
    let mut out = io::BufWriter::with_capacity(64 * 1024, io::stdout().lock());
    serde_json::to_writer_pretty(&mut out, value).map_err(|e| match e.io_error_kind() {
        Some(kind) => CliError::Io(io::Error::new(kind, e)),
        None => CliError::Json(e),
    })?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}
/// Small stderr spinner owned by the active operation. Drop aborts and clears it.
pub struct Progress {
    task: Option<tokio::task::JoinHandle<()>>,
    width: usize,
}
impl Progress {
    /// Start only on a real stderr terminal; scripts receive no progress bytes.
    pub fn start(message: &str, _color: bool) -> Self {
        if !io::stderr().is_terminal() || std::env::var_os("CI").is_some() {
            return Self {
                task: None,
                width: 0,
            };
        }
        let message = safe_terminal(message);
        let width = message.chars().count() + 4;
        let task = tokio::spawn(async move {
            let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            let mut i = 0;
            loop {
                let _ = write!(
                    io::stderr().lock(),
                    "\r{} {}",
                    frames[i % frames.len()],
                    message
                );
                i += 1;
                tokio::time::sleep(std::time::Duration::from_millis(90)).await;
            }
        });
        Self {
            task: Some(task),
            width,
        }
    }
}
impl Drop for Progress {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = write!(io::stderr().lock(), "\r{}\r", " ".repeat(self.width));
        }
    }
}
