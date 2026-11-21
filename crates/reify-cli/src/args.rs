//! Clap parser and boundary validation, shared with integration tests.
use clap::{CommandFactory, Parser, Subcommand, ValueEnum, error::ErrorKind};
use reify_core::domain::Target;
use serde_json::Value;
use std::path::PathBuf;
use url::Url;
/// Explicit terminal color preference.
#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum Color {
    /// Detect a terminal and honor NO_COLOR.
    Auto,
    /// Always emit ANSI styles.
    Always,
    /// Never emit ANSI styles.
    Never,
}
/// Agent configuration layouts supported by the installer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum InstallAgent {
    /// Cursor editor.
    Cursor,
    /// Claude Code.
    Claude,
    /// Windsurf editor.
    Windsurf,
    /// OpenCode v2.
    Opencode,
    /// Discover installed agents automatically.
    All,
}
/// Configure an agent to connect to the Reify gateway.
#[derive(Debug, clap::Args)]
pub struct InstallArgs {
    /// Agent to configure; all discovers installed environments.
    #[arg(long, value_enum, default_value = "all")]
    pub agent: InstallAgent,
    /// Gateway bind port.
    #[arg(long, default_value_t = 8080, value_parser = clap::value_parser!(u16).range(1..))]
    pub port: u16,
    /// Gateway bind host.
    #[arg(long, default_value = "127.0.0.1", value_parser = parse_bind_host)]
    pub host: String,
    /// Install in the user home directory; use --global=false for the workspace.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set,
        num_args = 0..=1, require_equals = true, default_missing_value = "true")]
    pub global: bool,
    /// Install in the current workspace (alias for --global=false).
    #[arg(long, conflicts_with = "global")]
    pub local: bool,
    /// Display planned edits without writing files or creating directories.
    #[arg(long)]
    pub dry_run: bool,
    /// Remove only the reify server entry.
    #[arg(long)]
    pub uninstall: bool,
}

fn parse_bind_host(value: &str) -> Result<String, String> {
    if let Ok(ip) = value
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<std::net::IpAddr>()
    {
        return Ok(ip.to_string());
    }
    let host =
        url::Host::parse(value).map_err(|_| "expected a hostname or IP address".to_owned())?;
    let host = host.to_string();
    if host.is_empty()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
    {
        return Err("expected a hostname or IP address".into());
    }
    Ok(host)
}
/// Parsed `reify` invocation. A URL without a subcommand selects compile/mount mode.
#[derive(Debug, Parser)]
#[command(
    name = "reify",
    version,
    about = "Turn live websites into typed MCP tools",
    arg_required_else_help = true
)]
pub struct Cli {
    /// Website to compile and mount on 127.0.0.1:8080.
    #[arg(value_name="URL",value_parser=parse_url)]
    pub url: Option<Url>,
    /// Task goal; shell-quoted text or multiple words are accepted.
    #[arg(value_name="GOAL",num_args=0..,trailing_var_arg=true,requires="url")]
    pub goal: Vec<String>,
    /// Explicit command instead of the default compile/mount mode.
    #[command(subcommand)]
    pub command: Option<Command>,
    /// Maximum browser capture duration in seconds.
    #[arg(long,global=true,default_value_t=30,value_parser=clap::value_parser!(u64).range(1..=300))]
    pub timeout: u64,
    /// Chromium executable override (also read by the probe).
    #[arg(long, global = true, env = "REIFY_CHROMIUM_PATH", value_name = "PATH")]
    pub chromium: Option<PathBuf>,
    /// Terminal styling mode.
    #[arg(long,global=true,value_enum,default_value_t=Color::Auto)]
    pub color: Color,
    /// Bypass embedded profiles and perform live discovery.
    #[arg(long, alias = "no-profile", global = true)]
    pub fresh: bool,
}
/// Supported developer commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Connect installed AI agents to the Reify MCP gateway.
    Install(InstallArgs),
    /// Start the long-running stateless MCP gateway.
    Serve {
        /// Bind hostname or IP address.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Bind port (0 selects an ephemeral port).
        #[arg(long, default_value_t = 8080)]
        port: u16,
    },
    /// Print captured API traffic and synthesized tool schemas as JSON.
    Probe {
        /// Target HTTP(S) URL.
        #[arg(value_parser=parse_url)]
        url: Url,
        /// Task goal used to discover interactive search endpoints.
        #[arg(value_name = "GOAL", num_args = 0..)]
        goal: Vec<String>,
    },
    /// Resolve and execute a synthesized tool, printing its MCP result as JSON.
    Call {
        /// Target HTTP(S) URL.
        #[arg(value_parser=parse_url)]
        url: Url,
        /// Exact compiled tool name.
        tool_name: String,
        /// JSON object containing named arguments.
        #[arg(default_value="{}",value_parser=parse_arguments)]
        json_arguments: Value,
        /// Match a task-scoped compilation, rather than the domain-only scope.
        #[arg(long)]
        goal: Option<String>,
        /// Use an existing gateway cache instead of a fresh local process cache.
        #[arg(long,env="REIFY_GATEWAY",value_parser=parse_url)]
        gateway: Option<Url>,
    },
    /// Run the explicit-header local development proxy on loopback.
    Bridge {
        /// Loopback port (0 selects an ephemeral port).
        #[arg(long, default_value_t = 9099)]
        port: u16,
        /// Local authorization token; prefer the environment variable to shell history.
        #[arg(long, env = "REIFY_BRIDGE_TOKEN", hide_env_values = true)]
        token: Option<String>,
        /// JSON object mapping exact domain names to AuthContext values.
        #[arg(long, value_name = "PATH")]
        session_file: Option<PathBuf>,
    },
    /// List embedded profiles and their typed tool signatures.
    Profiles,
}
/// Validate HTTP(S), canonicalize a target, and disallow embedded credentials.
pub fn parse_url(value: &str) -> Result<Url, String> {
    let target = Target::new(value, None)
        .map_err(|_| "expected an absolute HTTP or HTTPS URL".to_owned())?;
    if !target.url.username().is_empty() || target.url.password().is_some() {
        return Err("URL credentials are not supported; configure session headers instead".into());
    }
    Ok(target.url)
}
/// Parse arguments once at the CLI boundary, rejecting scalar/array JSON.
pub fn parse_arguments(value: &str) -> Result<Value, String> {
    let value: Value =
        serde_json::from_str(value).map_err(|e| format!("invalid JSON arguments: {e}"))?;
    if !value.is_object() {
        return Err("tool arguments must be a JSON object".into());
    }
    Ok(value)
}
impl Cli {
    /// Parse with the additional requirement that a root URL or command is present.
    pub fn parse_checked_from<I, T>(args: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        let mut args: Vec<std::ffi::OsString> = args.into_iter().map(Into::into).collect();
        // Normalize boolean profile options before protecting the variadic goal.
        let mut fresh = false;
        let mut fresh_index = 1;
        while fresh_index < args.len() {
            if args[fresh_index] == "--" {
                break;
            }
            if args[fresh_index] == "--fresh" || args[fresh_index] == "--no-profile" {
                args.remove(fresh_index);
                fresh = true;
            } else {
                fresh_index += 1;
            }
        }
        if fresh {
            args.insert(1, "--fresh".into());
        }
        let mut index = 1;
        // Once a root URL starts, command-looking words belong to the goal.
        // Global options precede the URL; subcommand options retain normal Clap parsing.
        while index < args.len() {
            let argument = args[index].to_string_lossy();
            if matches!(argument.as_ref(), "--timeout" | "--chromium" | "--color") {
                index += 2;
                continue;
            }
            if argument == "--fresh" || argument == "--no-profile" {
                index += 1;
                continue;
            }
            if argument.starts_with("--timeout=")
                || argument.starts_with("--chromium=")
                || argument.starts_with("--color=")
            {
                index += 1;
                continue;
            }
            if argument.starts_with('-')
                || matches!(
                    argument.as_ref(),
                    "serve" | "probe" | "call" | "bridge" | "install" | "profiles" | "help"
                )
            {
                break;
            }
            args.insert(index, "--".into());
            break;
        }
        let parsed = Self::try_parse_from(args)?;
        if parsed.command.is_some() && parsed.url.is_some() {
            return Err(Self::command().error(
                ErrorKind::ArgumentConflict,
                "use either a URL or a subcommand",
            ));
        }
        if parsed.command.is_none() && parsed.url.is_none() {
            return Err(Self::command().error(
                ErrorKind::MissingRequiredArgument,
                "provide a URL or a subcommand",
            ));
        }
        Ok(parsed)
    }
}
