use reify_cli::{Cli, Command as CliCommand, args::InstallAgent};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    workspace: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "reify-install-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let home = root.join("home");
        let workspace = root.join("workspace");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&workspace).unwrap();
        Self {
            root,
            home,
            workspace,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_reify"));
        command
            .current_dir(&self.workspace)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("PATH", self.root.join("bin"))
            .env("NO_COLOR", "1");
        command.args(["--color", "never", "install"]);
        command
    }

    fn run(&self, arguments: &[&str]) -> Output {
        self.command().args(arguments).output().unwrap()
    }

    fn success(&self, arguments: &[&str]) -> String {
        let output = self.run(arguments);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn write(&self, relative: &str, value: Value) -> PathBuf {
        let path = self.home.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(path: &Path, files: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.insert(path.clone(), None);
                visit(&path, files);
            } else {
                files.insert(path.clone(), Some(fs::read(path).unwrap()));
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, &mut files);
    files
}

#[test]
fn install_options_and_workspace_scope_parse_without_confusing_root_goals() {
    let cli = Cli::parse_checked_from(["reify", "install"]).unwrap();
    assert!(matches!(cli.command, Some(CliCommand::Install(options))
        if options.agent == InstallAgent::All && options.global && options.port == 8080));
    for scope in ["--global", "--global=false", "--local"] {
        Cli::parse_checked_from(["reify", "install", scope, "--dry-run"]).unwrap();
    }
    for arguments in [
        vec!["--agent", "unknown"],
        vec!["--port", "0"],
        vec!["--port", "65536"],
        vec!["--host", "https://example.com"],
        vec!["--host", "host/path"],
        vec!["--host", "user@host"],
        vec!["--host", "$(whoami)"],
        vec!["--local", "--global"],
    ] {
        assert!(
            Cli::parse_checked_from(["reify", "install"].into_iter().chain(arguments)).is_err()
        );
    }
    let cli =
        Cli::parse_checked_from(["reify", "https://example.com", "install", "tools"]).unwrap();
    assert_eq!(cli.goal, ["install", "tools"]);
}

#[test]
fn creates_each_agents_requested_schema_with_custom_endpoint() {
    let fixture = Fixture::new();
    for (agent, relative, keys, expected) in [
        (
            "cursor",
            ".cursor/mcp.json",
            vec!["mcpServers"],
            json!({"url":"http://localhost:9090/mcp"}),
        ),
        (
            "claude",
            ".claude.json",
            vec!["mcpServers"],
            json!({"type":"http","url":"http://localhost:9090/mcp"}),
        ),
        (
            "windsurf",
            ".codeium/windsurf/mcp_config.json",
            vec!["mcp", "servers"],
            json!({"type":"remote","url":"http://localhost:9090/mcp","protocol":"2026-07-28"}),
        ),
        (
            "opencode",
            ".config/opencode/opencode.json",
            vec!["mcp", "servers"],
            json!({"type":"remote","url":"http://localhost:9090/mcp","protocol":"2026-07-28"}),
        ),
    ] {
        let output = fixture.success(&["--agent", agent, "--host", "localhost", "--port", "9090"]);
        let path = fixture.home.join(relative);
        let value = read(&path);
        let servers = keys.iter().fold(&value, |value, key| &value[*key]);
        assert_eq!(servers["reify"], expected);
        let bytes = fs::read_to_string(path).unwrap();
        assert!(bytes.starts_with("{\n  \""));
        assert!(bytes.ends_with('\n'));
        assert!(output.contains("Injected Reify MCP gateway"));
        assert!(output.contains("Start gateway: reify serve --host=localhost --port 9090"));
    }
}

#[test]
fn merging_and_reinstall_preserve_other_servers_and_unrelated_settings() {
    let fixture = Fixture::new();
    let path = fixture.write(
        ".cursor/mcp.json",
        json!({
            "mcpServers":{
                "filesystem":{"command":"fs", "args":["/data"]},
                "postgres":{"url":"http://db/mcp"},
                "github":{"env":{"TOKEN":"keep-private"}},
                "reify":{"url":"http://old/mcp"}
            }, "editor":{"enabled":true}
        }),
    );
    let original = read(&path);
    fixture.success(&["--agent", "cursor"]);
    let installed = read(&path);
    for server in ["filesystem", "postgres", "github"] {
        assert_eq!(
            installed["mcpServers"][server],
            original["mcpServers"][server]
        );
    }
    assert_eq!(installed["editor"], original["editor"]);
    assert_eq!(
        installed["mcpServers"]["reify"]["url"],
        "http://127.0.0.1:8080/mcp"
    );
    let bytes = fs::read(&path).unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let output = fixture.success(&["--agent", "cursor"]);
    assert!(output.contains("already configured"));
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
    assert_eq!(installed["mcpServers"].as_object().unwrap().len(), 4);
}

#[test]
fn nested_merge_preserves_sibling_settings_and_existing_servers() {
    let fixture = Fixture::new();
    let path = fixture.write(
        ".config/opencode/opencode.json",
        json!({
            "model":"example", "mcp":{"enabled":true,"servers":{"filesystem":{"command":"fs"}}}
        }),
    );
    fixture.success(&["--agent", "opencode"]);
    assert_eq!(
        read(&path),
        json!({
            "model":"example", "mcp":{"enabled":true,"servers":{
                "filesystem":{"command":"fs"}, "reify":{
                    "type":"remote", "url":"http://127.0.0.1:8080/mcp", "protocol":"2026-07-28"
                }
            }}
        })
    );
}

#[test]
fn dry_run_previews_edits_without_any_filesystem_changes_or_credential_output() {
    let fixture = Fixture::new();
    fixture.write(
        ".cursor/mcp.json",
        json!({"mcpServers":{"github":{"token":"never-print-this"}}}),
    );
    let original = snapshot(&fixture.root);
    let output = fixture.success(&["--agent", "cursor", "--dry-run"]);
    assert!(output.contains("Would inject"));
    assert!(output.contains("mcpServers.reify"));
    assert!(output.contains("http://127.0.0.1:8080/mcp"));
    assert!(!output.contains("never-print-this"));
    fixture.success(&["--agent", "claude", "--dry-run"]);
    fixture.success(&["--agent", "windsurf", "--local", "--dry-run"]);
    assert_eq!(snapshot(&fixture.root), original);
}

#[test]
fn uninstall_removes_only_reify_and_preserves_empty_server_maps() {
    let fixture = Fixture::new();
    let cursor = fixture.write(
        ".cursor/mcp.json",
        json!({
            "mcpServers":{"reify":{"url":"old"},"filesystem":{"command":"fs"}},"setting":42
        }),
    );
    fixture.success(&["--agent", "cursor", "--uninstall"]);
    assert_eq!(
        read(&cursor),
        json!({"mcpServers":{"filesystem":{"command":"fs"}},"setting":42})
    );
    fixture.success(&["--agent", "claude"]);
    fixture.success(&["--agent", "claude", "--uninstall"]);
    assert_eq!(
        read(&fixture.home.join(".claude.json")),
        json!({"mcpServers":{}})
    );
    fixture.success(&["--agent", "opencode"]);
    fixture.success(&["--agent", "opencode", "--uninstall"]);
    assert_eq!(
        read(&fixture.home.join(".config/opencode/opencode.json")),
        json!({"mcp":{"servers":{}}})
    );
    let snapshot = snapshot(&fixture.root);
    let output = fixture.success(&["--agent", "claude", "--uninstall"]);
    assert!(output.contains("already absent"));
    assert!(!output.contains("Start gateway"));
    assert_eq!(self::snapshot(&fixture.root), snapshot);
}

#[test]
fn uninstall_missing_or_unconfigured_files_does_not_create_or_reformat_them() {
    let fixture = Fixture::new();
    fixture.write(".cursor/mcp.json", json!({"editor":{"enabled":true}}));
    let before = snapshot(&fixture.root);
    fixture.success(&["--agent", "cursor", "--uninstall"]);
    fixture.success(&["--agent", "windsurf", "--uninstall"]);
    fixture.success(&["--agent", "claude", "--uninstall", "--dry-run"]);
    assert_eq!(snapshot(&fixture.root), before);
}

#[test]
fn all_only_discovers_installed_environments_and_honors_legacy_claude_path() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.home.join(".cursor")).unwrap();
    let claude = fixture.write(
        ".claude/config.json",
        json!({"mcpServers":{"github":{"command":"gh"}},"theme":"dark"}),
    );
    fixture.success(&[]);
    assert!(fixture.home.join(".cursor/mcp.json").is_file());
    assert_eq!(read(&claude)["theme"], "dark");
    assert!(read(&claude)["mcpServers"]["reify"].is_object());
    assert!(!fixture.home.join(".claude.json").exists());
    assert!(!fixture.home.join(".codeium").exists());
    assert!(!fixture.home.join(".config/opencode").exists());
}

#[test]
fn empty_discovery_does_not_create_configs() {
    let fixture = Fixture::new();
    let original = snapshot(&fixture.root);
    assert!(
        fixture
            .success(&[])
            .contains("No supported agent environments detected")
    );
    assert_eq!(snapshot(&fixture.root), original);
}

#[test]
fn xdg_cursor_and_opencode_paths_are_discovered_and_preserved() {
    let fixture = Fixture::new();
    let xdg = fixture.root.join("custom-config");
    fs::create_dir_all(xdg.join("Cursor")).unwrap();
    fs::create_dir_all(xdg.join("opencode")).unwrap();
    let output = fixture
        .command()
        .env("XDG_CONFIG_HOME", &xdg)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(read(&xdg.join("Cursor/mcp.json"))["mcpServers"]["reify"].is_object());
    assert!(read(&xdg.join("opencode/opencode.json"))["mcp"]["servers"]["reify"].is_object());
    assert!(!fixture.home.join(".cursor").exists());
}

#[test]
fn local_and_global_false_install_only_in_the_workspace() {
    let fixture = Fixture::new();
    for (agent, scope, relative) in [
        ("cursor", "--local", ".cursor/mcp.json"),
        ("claude", "--global=false", ".mcp.json"),
        ("windsurf", "--local", ".codeium/windsurf/mcp_config.json"),
        ("opencode", "--local", "opencode.json"),
    ] {
        fixture.success(&["--agent", agent, scope]);
        assert!(fixture.workspace.join(relative).is_file());
    }
    assert!(fs::read_dir(&fixture.home).unwrap().next().is_none());
}

#[test]
fn malformed_json_and_incompatible_container_types_are_left_byte_for_byte_unchanged() {
    let fixture = Fixture::new();
    let path = fixture.home.join(".claude.json");
    for invalid in [
        "{broken",
        "null",
        "[]",
        r#"{"mcpServers":null}"#,
        r#"{"mcpServers":[]}"#,
    ] {
        fs::write(&path, invalid).unwrap();
        let output = fixture.run(&["--agent", "claude"]);
        assert!(!output.status.success());
        assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
        assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
    }
    let path = fixture.write(
        ".config/opencode/opencode.json",
        json!({"mcp":{"servers":[]}}),
    );
    let original = fs::read(&path).unwrap();
    assert!(!fixture.run(&["--agent", "opencode"]).status.success());
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn all_preflights_every_config_before_writing_any_edits() {
    let fixture = Fixture::new();
    fixture.write(
        ".cursor/mcp.json",
        json!({"mcpServers":{"other":{"command":"keep"}}}),
    );
    fs::write(fixture.home.join(".claude.json"), "invalid").unwrap();
    let original = snapshot(&fixture.root);
    assert!(!fixture.run(&[]).status.success());
    assert_eq!(snapshot(&fixture.root), original);
}

#[test]
fn ipv6_and_wildcard_bind_hosts_generate_usable_endpoints() {
    let fixture = Fixture::new();
    for (host, expected) in [
        ("::1", "http://[::1]:8080/mcp"),
        ("[::1]", "http://[::1]:8080/mcp"),
        ("::", "http://[::1]:8080/mcp"),
        ("0.0.0.0", "http://127.0.0.1:8080/mcp"),
    ] {
        fixture.success(&["--agent", "cursor", "--host", host]);
        assert_eq!(
            read(&fixture.home.join(".cursor/mcp.json"))["mcpServers"]["reify"]["url"],
            expected
        );
    }
}

#[cfg(unix)]
#[test]
fn atomic_replacement_preserves_modes_symlinks_and_leaves_no_temporary_files() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    let fixture = Fixture::new();
    let destination = fixture.home.join("shared.json");
    fs::write(
        &destination,
        "{\"mcpServers\":{\"other\":{\"command\":\"keep\"}}}",
    )
    .unwrap();
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o640)).unwrap();
    fs::create_dir_all(fixture.home.join(".cursor")).unwrap();
    let link = fixture.home.join(".cursor/mcp.json");
    symlink(&destination, &link).unwrap();
    let old_inode = fs::metadata(&destination).unwrap().ino();
    fixture.success(&["--agent", "cursor"]);
    assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
    assert_eq!(
        read(&destination)["mcpServers"]["other"],
        json!({"command":"keep"})
    );
    assert_eq!(
        fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let installed_inode = fs::metadata(&destination).unwrap().ino();
    assert_ne!(installed_inode, old_inode);
    fixture.success(&["--agent", "cursor"]);
    assert_eq!(fs::metadata(&destination).unwrap().ino(), installed_inode);
    assert!(
        snapshot(&fixture.root)
            .keys()
            .all(|path| !path.to_string_lossy().contains(".tmp."))
    );
    fixture.success(&["--agent", "claude"]);
    assert_eq!(
        fs::metadata(fixture.home.join(".claude.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[cfg(unix)]
#[test]
fn all_detects_agent_executables_without_existing_configuration_directories() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.root.join("bin")).unwrap();
    let binary = fixture.root.join("bin/claude");
    fs::write(&binary, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    fixture.success(&[]);
    assert!(fixture.home.join(".claude.json").is_file());
    assert!(!fixture.home.join(".cursor").exists());
}
