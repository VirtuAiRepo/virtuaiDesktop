//! Supervises `virtuai run --desktop`: starts it, restarts it when it dies,
//! and speaks its line protocol (cli/src/virtuai_cli/desktop_bridge.py).
//!
//! The runner is the existing CLI, unchanged in how it serves agents. The app
//! only gives it a lifecycle (it runs while the app runs) and someone to ask.

use serde::Deserialize;
use std::env;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

const PREFIX: &str = "@@virtuai ";

/// One line from the runner, when it is meant for the app.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    State {
        state: String,
    },
    Status {
        level: String,
        message: String,
        #[serde(default)]
        state: Option<String>,
    },
    ApprovalRequest {
        id: String,
        command: String,
        #[serde(default)]
        reason: String,
        #[serde(default)]
        workdir: String,
    },
    ApprovalExpired {
        id: String,
    },
}

pub fn parse_line(line: &str) -> Option<Event> {
    serde_json::from_str(line.strip_prefix(PREFIX)?).ok()
}

/// What the tray shows. Strings, not an enum, because they come from the
/// runner and the tray only displays them.
pub type OnEvent = Arc<dyn Fn(Event) + Send + Sync>;

#[derive(Default)]
struct Inner {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    /// Bumped on every stop: a supervisor loop from an older start sees it
    /// changed and leaves, so there is never more than one runner.
    generation: u64,
}

#[derive(Clone)]
pub struct Runner {
    inner: Arc<Mutex<Inner>>,
    on_event: OnEvent,
}

impl Runner {
    pub fn new(on_event: OnEvent) -> Self {
        Self {
            inner: Arc::default(),
            on_event,
        }
    }

    /// Start (or restart) the runner against `server`, working in `workdir`.
    pub fn start(&self, server: String, workdir: PathBuf) {
        let generation = self.stop();
        let this = self.clone();
        thread::spawn(move || this.supervise(server, workdir, generation));
    }

    /// Stop the runner; returns the generation a new start should use.
    pub fn stop(&self) -> u64 {
        let mut inner = self.inner.lock().unwrap();
        inner.generation += 1;
        // Closing stdin is the runner's cue to stop on its own (it releases
        // its single-runner lock that way); the kill is for one that hangs.
        inner.stdin = None;
        if let Some(mut child) = inner.child.take() {
            for _ in 0..30 {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        inner.generation
    }

    pub fn answer(&self, id: &str, approved: bool) {
        let line = serde_json::json!({"type": "approval_response", "id": id, "approved": approved});
        let mut inner = self.inner.lock().unwrap();
        if let Some(stdin) = inner.stdin.as_mut() {
            let _ = writeln!(stdin, "{line}").and_then(|_| stdin.flush());
        }
    }

    fn current(&self, generation: u64) -> bool {
        self.inner.lock().unwrap().generation == generation
    }

    fn emit_state(&self, state: &str) {
        (self.on_event)(Event::State {
            state: state.into(),
        });
    }

    fn supervise(&self, server: String, workdir: PathBuf, generation: u64) {
        let mut delay = 2;
        while self.current(generation) {
            let Some(cli) = find_cli() else {
                self.emit_state("missing_cli");
                return;
            };
            self.emit_state("starting");
            let spawned = Command::new(&cli)
                // Always a folder: without one the CLI works in its current
                // directory, which for the app is its source tree or `/`.
                .args(["run", "--desktop", "--server", &server])
                .arg("--workdir")
                .arg(&workdir)
                .env("PATH", login_path())
                .env("PYTHONUNBUFFERED", "1")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn();
            let mut child = match spawned {
                Ok(child) => child,
                Err(err) => {
                    eprintln!("[runner] could not start {}: {err}", cli.display());
                    self.emit_state("missing_cli");
                    return;
                }
            };
            let stdout = child.stdout.take().expect("piped stdout");
            {
                let mut inner = self.inner.lock().unwrap();
                if inner.generation != generation {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
                inner.stdin = child.stdin.take();
                inner.child = Some(child);
            }

            let mut not_paired = false;
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Some(event) = parse_line(&line) else {
                    eprintln!("[runner] {line}");
                    continue;
                };
                match &event {
                    Event::State { state } if state == "not_paired" => not_paired = true,
                    Event::Status { state: Some(s), .. } if s == "connected" => delay = 2,
                    _ => {}
                }
                (self.on_event)(event);
            }

            // stdout closed: the runner exited (or was stopped).
            {
                let mut inner = self.inner.lock().unwrap();
                if inner.generation == generation {
                    inner.stdin = None;
                    if let Some(mut child) = inner.child.take() {
                        let _ = child.wait();
                    }
                }
            }
            if !self.current(generation) {
                return;
            }
            if not_paired {
                // Nothing to retry until the person pairs from the web chat.
                return;
            }
            self.emit_state("restarting");
            thread::sleep(Duration::from_secs(delay));
            delay = (delay * 2).min(30);
        }
    }
}

/// Where the app's own copy of the CLI lives (`<resources>/runner`), set once
/// at startup. Absent in development unless built with the runner config.
static BUNDLED_RUNNER: OnceLock<PathBuf> = OnceLock::new();

pub fn set_bundled_runner_dir(dir: PathBuf) {
    let _ = BUNDLED_RUNNER.set(dir);
}

/// The CLI to run, in order: an explicit override (development), the copy
/// bundled inside the app (desktop/runner/build.sh), then where `pipx` / `uv
/// tool` / Homebrew put it, then PATH.
pub fn find_cli() -> Option<PathBuf> {
    if let Some(path) = env::var_os("VIRTUAI_CLI") {
        return Some(PathBuf::from(path));
    }
    cli_candidates(
        BUNDLED_RUNNER.get().map(PathBuf::as_path),
        env::var_os("HOME").map(PathBuf::from).as_deref(),
        env::var_os("PATH"),
    )
    .into_iter()
    .find(|p| p.is_file())
}

fn cli_candidates(bundled: Option<&Path>, home: Option<&Path>, path: Option<OsString>) -> Vec<PathBuf> {
    let exe = if cfg!(windows) { "virtuai.exe" } else { "virtuai" };
    let mut candidates = Vec::new();
    if let Some(dir) = bundled {
        candidates.push(dir.join(exe));
    }
    if let Some(home) = home {
        candidates.push(home.join(".local/bin").join(exe));
    }
    candidates.push(PathBuf::from("/opt/homebrew/bin").join(exe));
    candidates.push(PathBuf::from("/usr/local/bin").join(exe));
    if let Some(path) = path {
        candidates.extend(env::split_paths(&path).map(|dir| dir.join(exe)));
    }
    candidates
}

/// A Finder-launched app gets PATH=/usr/bin:/bin:/usr/sbin:/sbin, without
/// Homebrew or ~/.local/bin — and the agent's commands inherit it. Add the
/// usual places back so `git`, `node` or `brew` resolve as in a terminal.
fn login_path() -> String {
    let mut dirs: Vec<PathBuf> = env::var_os("PATH")
        .map(|p| env::split_paths(&p).collect())
        .unwrap_or_default();
    let mut extra = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    if let Some(home) = env::var_os("HOME") {
        extra.push(PathBuf::from(home).join(".local/bin"));
    }
    for dir in extra {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    env::join_paths(dirs)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `virtuai pair <code> --server <server>`: the token goes to the keychain,
/// where the runner reads it.
pub fn pair(code: &str, server: &str) -> Result<(), String> {
    let cli = find_cli().ok_or("The VirtuAI CLI is not installed.")?;
    let output = Command::new(cli)
        .args(["pair", code, "--server", server])
        .env("PATH", login_path())
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        let text = String::from_utf8_lossy(&output.stdout);
        let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
        Err(format!("Pairing failed. {last}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_bridge_protocol() {
        assert_eq!(
            parse_line(r#"@@virtuai {"type": "approval_request", "id": "a1", "command": "rm -rf build", "reason": "no classifier", "workdir": "/w"}"#),
            Some(Event::ApprovalRequest {
                id: "a1".into(),
                command: "rm -rf build".into(),
                reason: "no classifier".into(),
                workdir: "/w".into(),
            })
        );
        assert_eq!(
            parse_line(r#"@@virtuai {"type": "state", "state": "not_paired"}"#),
            Some(Event::State {
                state: "not_paired".into()
            })
        );
        assert!(matches!(
            parse_line(r#"@@virtuai {"type": "status", "level": "info", "message": "Connected. Workdir: /w", "state": "connected"}"#),
            Some(Event::Status { state: Some(ref s), .. }) if s == "connected"
        ));
    }

    #[test]
    fn the_bundled_runner_comes_before_anything_installed() {
        let found = cli_candidates(
            Some(Path::new("/Applications/VirtuAI.app/Contents/Resources/runner")),
            Some(Path::new("/Users/ana")),
            Some(OsString::from("/usr/bin")),
        );
        assert_eq!(
            found.first().unwrap(),
            Path::new("/Applications/VirtuAI.app/Contents/Resources/runner/virtuai")
        );
        assert!(found.contains(&PathBuf::from("/Users/ana/.local/bin/virtuai")));
        assert_eq!(found.last().unwrap(), Path::new("/usr/bin/virtuai"));
    }

    #[test]
    fn without_a_bundle_it_falls_back_to_an_installed_cli() {
        let found = cli_candidates(None, Some(Path::new("/Users/ana")), None);
        assert_eq!(found.first().unwrap(), Path::new("/Users/ana/.local/bin/virtuai"));
    }

    #[test]
    fn ignores_ordinary_output() {
        assert_eq!(parse_line("VirtuAI CLI v1.0.19 — Press Ctrl+C to stop."), None);
        assert_eq!(parse_line("@@virtuai not json"), None);
        assert_eq!(parse_line(r#"@@virtuai {"type": "something_new"}"#), None);
    }
}
