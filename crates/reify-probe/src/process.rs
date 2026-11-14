//! Chromium discovery, isolated profiles, startup detection, and independent reaping.
use crate::failure;
use reify_core::ReifyError;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};
use tokio::sync::oneshot;
use url::Url;
static MANAGED_CHILDREN: AtomicUsize = AtomicUsize::new(0);
struct ReapingChild;
impl Drop for ReapingChild {
    fn drop(&mut self) {
        MANAGED_CHILDREN.fetch_sub(1, Ordering::AcqRel);
    }
}
/// Wait for managed children and reapers after their owning async runtime has
/// shut down. This blocking process-exit hook lets SIGKILL/reaping finish before
/// the host exits; callers must drop all browser owners first.
pub fn wait_for_cleanup(timeout: Duration) -> Result<(), ReifyError> {
    let start = std::time::Instant::now();
    while MANAGED_CHILDREN.load(Ordering::Acquire) != 0 {
        if start.elapsed() >= timeout {
            return Err(failure(
                "Chromium cleanup",
                "managed browser did not finish reaping",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

/// A browser whose process group is terminated and reaped even if the async runtime exits.
pub struct ChromiumProcess {
    child: Option<Child>,
    profile: PathBuf,
    endpoint: Option<Url>,
}
fn executable(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
/// Discover an executable without mutating environment state. Explicit overrides fail closed.
pub fn discover_in(
    override_path: Option<&Path>,
    home: Option<&Path>,
    path: Option<&std::ffi::OsStr>,
) -> Result<PathBuf, ReifyError> {
    if let Some(p) = override_path {
        return if executable(p) {
            Ok(p.to_owned())
        } else {
            Err(failure(
                "REIFY_CHROMIUM_PATH is not executable",
                p.display(),
            ))
        };
    }
    if let Some(home) = home {
        let cache = home.join(".cache/ms-playwright");
        if let Ok(entries) = cache.read_dir() {
            let mut entries: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
            entries.sort();
            entries.reverse();
            for entry in entries {
                if !entry
                    .file_name()
                    .is_some_and(|s| s.to_string_lossy().starts_with("chromium"))
                {
                    continue;
                }
                for relative in [
                    "chrome-linux64/chrome",
                    "chrome-headless-shell",
                    "chrome-linux/chrome",
                    "chrome-linux64/chrome-headless-shell",
                    "chrome-linux/headless_shell",
                    "chrome-headless-shell-linux64/chrome-headless-shell",
                    "chrome-linux64/headless_shell",
                ] {
                    let p = entry.join(relative);
                    if executable(&p) {
                        return Ok(p);
                    }
                }
            }
        }
    }
    if let Some(path) = path {
        for dir in std::env::split_paths(path) {
            for name in [
                "chromium",
                "chromium-browser",
                "google-chrome-stable",
                "google-chrome",
                "chrome-headless-shell",
            ] {
                let p = dir.join(name);
                if executable(&p) {
                    return Ok(p);
                }
            }
        }
    }
    Err(failure(
        "Chromium discovery",
        "set REIFY_CHROMIUM_PATH or install Chromium/Playwright",
    ))
}
/// Find Chromium using the configured override, Playwright cache, then PATH.
pub fn discover() -> Result<PathBuf, ReifyError> {
    discover_in(
        std::env::var_os("REIFY_CHROMIUM_PATH")
            .as_deref()
            .map(Path::new),
        std::env::var_os("HOME").as_deref().map(Path::new),
        std::env::var_os("PATH").as_deref(),
    )
}
/// Parse only a loopback DevTools announcement, ignoring unrelated stderr.
pub fn devtools_endpoint(line: &str) -> Option<Url> {
    let url = Url::parse(line.trim().strip_prefix("DevTools listening on ")?).ok()?;
    (url.scheme() == "ws"
        && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
        && url.port().is_some())
    .then_some(url)
}
impl ChromiumProcess {
    /// Launch with a fresh profile and wait at most ten seconds for DevTools.
    pub async fn launch(binary: &Path) -> Result<Self, ReifyError> {
        Self::launch_optional(Some(binary.to_owned())).await
    }
    pub(crate) async fn discover_and_launch() -> Result<Self, ReifyError> {
        Self::launch_optional(None).await
    }
    async fn launch_optional(binary: Option<PathBuf>) -> Result<Self, ReifyError> {
        // The blocking closure owns its process guard. If the await is canceled,
        // dropping the detached task's output still terminates/reaps its child.
        let (mut process, rx) = tokio::task::spawn_blocking(move || {
            let binary = match binary {
                Some(binary) => binary,
                None => discover()?,
            };
            Self::spawn(&binary)
        })
        .await
        .map_err(|_| failure("Chromium launch", "blocking worker failed"))??;
        process.endpoint = Some(
            tokio::time::timeout(Duration::from_secs(10), rx)
                .await
                .map_err(|_| failure("Chromium startup", "DevTools timeout"))?
                .map_err(|_| failure("Chromium startup", "exited without a DevTools endpoint"))?,
        );
        Ok(process)
    }
    fn spawn(binary: &Path) -> Result<(Self, oneshot::Receiver<Url>), ReifyError> {
        let profile = std::env::temp_dir().join(format!("reify-chromium-{}", uuid::Uuid::new_v4()));
        let mut directory = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            directory.mode(0o700);
        }
        directory
            .create(&profile)
            .map_err(|e| failure("create Chromium profile", e))?;
        let mut command = Command::new(binary);
        let shell = binary
            .file_name()
            .is_some_and(|n| n.to_string_lossy().contains("shell"));
        command
            .args([
                if shell {
                    "--headless"
                } else {
                    "--headless=new"
                },
                "--disable-gpu",
                "--no-sandbox",
                "--disable-dev-shm-usage",
                "--disable-extensions",
                "--disable-background-networking",
                "--no-first-run",
                "--no-default-browser-check",
                "--remote-debugging-address=127.0.0.1",
                "--remote-debugging-port=0",
            ])
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&profile);
                return Err(failure("spawn Chromium", e));
            }
        };
        MANAGED_CHILDREN.fetch_add(1, Ordering::AcqRel);
        let mut process = Self {
            child: Some(child),
            profile,
            endpoint: None,
        };
        let stderr = process
            .child
            .as_mut()
            .and_then(|c| c.stderr.take())
            .ok_or_else(|| failure("Chromium stderr", "pipe missing"))?;
        let (tx, rx) = oneshot::channel();
        std::thread::spawn(move || {
            let mut tx = Some(tx);
            // Keep draining after discovery so browser logging cannot fill the pipe.
            // read_until with take bounds individual lines, including malicious launch wrappers.
            let mut reader = BufReader::new(stderr);
            let mut line = Vec::with_capacity(256);
            loop {
                line.clear();
                use std::io::Read;
                match reader.by_ref().take(64 * 1024).read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if let Some(url) = devtools_endpoint(&String::from_utf8_lossy(&line))
                            && let Some(tx) = tx.take()
                        {
                            let _ = tx.send(url);
                        }
                    }
                }
            }
        });
        Ok((process, rx))
    }
    /// Announced endpoint; discovery uses its port with `/json/version`.
    pub fn endpoint(&self) -> &Url {
        self.endpoint
            .as_ref()
            .expect("launch returns only after discovery")
    }
}
impl Drop for ChromiumProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let profile = self.profile.clone();
            let registration = ReapingChild;
            std::thread::spawn(move || {
                let _registration = registration;
                #[cfg(unix)]
                {
                    // The child starts a dedicated process group. Its PID remains owned
                    // until wait(), so it cannot be recycled before these signals.
                    unsafe {
                        libc::kill(-(child.id() as i32), libc::SIGTERM);
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = child.kill();
                }
                // Do not call try_wait before the final group signal: reaping the
                // leader early would permit PID/process-group reuse.
                std::thread::sleep(Duration::from_millis(500));
                #[cfg(unix)]
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_dir_all(profile);
            });
        }
    }
}
