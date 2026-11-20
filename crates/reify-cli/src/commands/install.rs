//! Discovery, non-destructive JSON edits, and atomic agent configuration writes.
use crate::{
    CliError,
    args::{InstallAgent, InstallArgs},
    failure, ui,
};
use colored::Colorize;
use reify_core::mcp::PROTOCOL_VERSION;
use serde_json::{Map, Value, json};
use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const AGENTS: [InstallAgent; 4] = [
    InstallAgent::Cursor,
    InstallAgent::Claude,
    InstallAgent::Windsurf,
    InstallAgent::Opencode,
];

struct Environment {
    home: PathBuf,
    config: PathBuf,
    workspace: PathBuf,
}

fn existing(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn first_existing(paths: &[PathBuf]) -> PathBuf {
    paths
        .iter()
        .find(|path| existing(path))
        .unwrap_or(&paths[0])
        .clone()
}

impl Environment {
    fn discover() -> Result<Self, CliError> {
        let home = env::var_os("HOME")
            .or_else(|| env::var_os("USERPROFILE"))
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| failure("cannot locate user home directory"))?;
        let config = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home.join(".config"));
        Ok(Self {
            home,
            config,
            workspace: env::current_dir()?,
        })
    }

    fn path(&self, agent: InstallAgent, global: bool) -> PathBuf {
        if !global {
            return self.workspace.join(match agent {
                InstallAgent::Cursor => ".cursor/mcp.json",
                InstallAgent::Claude => ".mcp.json",
                InstallAgent::Windsurf => ".codeium/windsurf/mcp_config.json",
                _ => "opencode.json",
            });
        }
        match agent {
            InstallAgent::Cursor => {
                let paths = [
                    self.home.join(".cursor/mcp.json"),
                    self.config.join("Cursor/mcp.json"),
                ];
                if paths.iter().any(|path| existing(path)) || !self.config.join("Cursor").is_dir() {
                    first_existing(&paths)
                } else {
                    paths[1].clone()
                }
            }
            InstallAgent::Claude => first_existing(&[
                self.home.join(".claude.json"),
                self.home.join(".claude/config.json"),
            ]),
            InstallAgent::Windsurf => self.home.join(".codeium/windsurf/mcp_config.json"),
            _ => self.config.join("opencode/opencode.json"),
        }
    }

    fn detected(&self, agent: InstallAgent) -> bool {
        let directory = match agent {
            InstallAgent::Cursor => self.home.join(".cursor"),
            InstallAgent::Claude => self.home.join(".claude"),
            InstallAgent::Windsurf => self.home.join(".codeium/windsurf"),
            _ => self.config.join("opencode"),
        };
        directory.is_dir()
            || (agent == InstallAgent::Cursor && self.config.join("Cursor").is_dir())
            || self
                .workspace
                .join(match agent {
                    InstallAgent::Cursor => ".cursor",
                    InstallAgent::Claude => ".claude",
                    InstallAgent::Windsurf => ".codeium/windsurf",
                    _ => ".opencode",
                })
                .is_dir()
            || existing(&self.path(agent, true))
            || existing(&self.path(agent, false))
            || binary_on_path(&format!("{agent:?}").to_ascii_lowercase())
    }

    fn display(&self, path: &Path) -> String {
        let display = if let Ok(relative) = path.strip_prefix(&self.home) {
            format!("~/{}", relative.display())
        } else {
            path.display().to_string()
        };
        ui::safe_terminal(&display)
    }
}

fn binary_on_path(name: &str) -> bool {
    env::var_os("PATH").is_some_and(|path| {
        env::split_paths(&path).any(|dir| {
            let binary = dir.join(if cfg!(windows) {
                format!("{name}.exe")
            } else {
                name.into()
            });
            fs::metadata(binary).is_ok_and(|metadata| {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
                }
                #[cfg(not(unix))]
                {
                    metadata.is_file()
                }
            })
        })
    })
}

fn server_path(agent: InstallAgent) -> &'static [&'static str] {
    match agent {
        InstallAgent::Windsurf | InstallAgent::Opencode => &["mcp", "servers"],
        _ => &["mcpServers"],
    }
}

fn entry(agent: InstallAgent, endpoint: &str) -> Value {
    match agent {
        InstallAgent::Cursor => json!({"url":endpoint}),
        InstallAgent::Claude => json!({"type":"http", "url":endpoint}),
        _ => json!({"type":"remote", "url":endpoint, "protocol":PROTOCOL_VERSION}),
    }
}

fn servers<'a>(
    root: &'a mut Value,
    keys: &[&str],
    create: bool,
    path: &Path,
) -> Result<Option<&'a mut Map<String, Value>>, CliError> {
    let invalid = || {
        failure(format!(
            "{}: expected JSON objects along {}",
            path.display(),
            keys.join(".")
        ))
    };
    let mut object = root.as_object_mut().ok_or_else(invalid)?;
    for key in keys {
        if !object.contains_key(*key) {
            if !create {
                return Ok(None);
            }
            object.insert((*key).into(), json!({}));
        }
        object = object
            .get_mut(*key)
            .and_then(Value::as_object_mut)
            .ok_or_else(invalid)?;
    }
    Ok(Some(object))
}

fn read(path: &Path) -> Result<Option<Vec<u8>>, CliError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(failure(format!("cannot read {}: {error}", path.display()))),
    }
}

struct Edit {
    path: PathBuf,
    destination: PathBuf,
    original: Option<Vec<u8>>,
    updated: Option<Vec<u8>>,
    agent: InstallAgent,
}

impl Edit {
    fn plan(
        path: PathBuf,
        agent: InstallAgent,
        endpoint: &str,
        uninstall: bool,
    ) -> Result<Self, CliError> {
        let destination = if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_symlink())
        {
            fs::canonicalize(&path)?
        } else {
            path.clone()
        };
        let original = read(&destination)?;
        let mut value = match &original {
            Some(bytes) => serde_json::from_slice(bytes).map_err(|error| {
                failure(format!(
                    "{}: invalid JSON at line {}, column {}",
                    path.display(),
                    error.line(),
                    error.column()
                ))
            })?,
            None => json!({}),
        };
        let changed = match servers(&mut value, server_path(agent), !uninstall, &path)? {
            Some(servers) if uninstall => servers.remove("reify").is_some(),
            Some(servers) => {
                let entry = entry(agent, endpoint);
                if servers.get("reify") == Some(&entry) {
                    false
                } else {
                    servers.insert("reify".into(), entry);
                    true
                }
            }
            None => false,
        };
        let updated = if changed {
            let mut bytes = serde_json::to_vec_pretty(&value)?;
            bytes.push(b'\n');
            Some(bytes)
        } else {
            None
        };
        Ok(Self {
            path,
            destination,
            original,
            updated,
            agent,
        })
    }

    fn check_current(&self) -> Result<(), CliError> {
        if read(&self.destination)? != self.original {
            return Err(failure(format!(
                "{} changed during installation; retry the command",
                self.path.display()
            )));
        }
        Ok(())
    }

    fn write(&self) -> Result<(), CliError> {
        let Some(bytes) = &self.updated else {
            return Ok(());
        };
        self.check_current()?;
        let parent = self
            .destination
            .parent()
            .ok_or_else(|| failure("configuration has no parent directory"))?;
        fs::create_dir_all(parent)?;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let (temporary, mut file) = loop {
            let mut name = self
                .destination
                .file_name()
                .ok_or_else(|| failure("configuration has no filename"))?
                .to_os_string();
            name.push(format!(
                ".tmp.{}.{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let temporary = parent.join(name);
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&temporary) {
                Ok(file) => break (temporary, file),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        };
        let result = (|| {
            if self.original.is_some() {
                file.set_permissions(fs::metadata(&self.destination)?.permissions())?;
            }
            file.write_all(bytes)?;
            file.sync_all()?;
            self.check_current()?;
            drop(file);
            fs::rename(&temporary, &self.destination)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

/// Discover agent configs, preflight all edits, and apply or preview the changes.
/// Explicit agents can create configs; `all` only targets detected environments.
pub fn run(options: &InstallArgs, color: bool) -> Result<(), CliError> {
    let environment = Environment::discover()?;
    let endpoint = if let Ok(ip) = options.host.parse() {
        super::gateway::endpoint(std::net::SocketAddr::new(ip, options.port))
    } else {
        format!("http://{}:{}/mcp", options.host, options.port)
    };
    let global = options.global && !options.local;
    let agents: Vec<_> = if options.agent == InstallAgent::All {
        AGENTS
            .into_iter()
            .filter(|agent| environment.detected(*agent))
            .collect()
    } else {
        vec![options.agent]
    };
    let edits = agents
        .into_iter()
        .map(|agent| {
            Edit::plan(
                environment.path(agent, global),
                agent,
                &endpoint,
                options.uninstall,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    if edits.is_empty() {
        return ui::write_stdout(
            "No supported agent environments detected. Use --agent cursor, claude, windsurf, or opencode to create a configuration.\n",
        );
    }
    for edit in edits {
        let message = match (options.dry_run, options.uninstall, edit.updated.is_some()) {
            (true, true, true) => "Would remove Reify MCP gateway from",
            (true, false, true) => "Would inject Reify MCP gateway into",
            (_, true, false) => "Reify MCP gateway already absent from",
            (_, false, false) => "Reify MCP gateway already configured in",
            (false, true, true) => "Removed Reify MCP gateway from",
            (false, false, true) => "Injected Reify MCP gateway into",
        };
        if !options.dry_run {
            edit.write()?;
        }
        let mark = if options.dry_run {
            "·".into()
        } else if color {
            "✓".green().to_string()
        } else {
            "✓".into()
        };
        ui::write_stdout(&format!(
            "{mark} {message} {}\n",
            environment.display(&edit.path)
        ))?;
        if options.dry_run && edit.updated.is_some() {
            let change = if options.uninstall {
                "remove reify".into()
            } else {
                serde_json::to_string_pretty(&entry(edit.agent, &endpoint))?
            };
            ui::write_stdout(&format!(
                "  {}.reify: {change}\n",
                server_path(edit.agent).join(".")
            ))?;
        }
    }
    if !options.uninstall {
        let host = if options.host == "127.0.0.1" {
            String::new()
        } else {
            format!(" --host={}", options.host)
        };
        ui::write_stdout(&format!(
            "  Endpoint: {endpoint} (Stateless {PROTOCOL_VERSION})\n  Start gateway: reify serve{host} --port {}\n",
            options.port
        ))?;
    }
    Ok(())
}
