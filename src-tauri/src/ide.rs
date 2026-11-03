//! The local IDE host: the pinned ThinkRail client and `/ws` on one port.
//!
//! It starts only when the connector is paired and the development variables
//! name a Python, the VirtuAI checkout, and a built client. A command it asks
//! about uses the same `@@virtuai` line as the runner; the answer goes to this
//! process, not to the runner.

use crate::runner::{parse_line, Event, OnEvent};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

pub const PORT: u16 = 8765;

#[derive(Clone)]
pub struct Spec {
    pub python: String,
    pub repo: String,
    pub client: String,
    pub server: String,
    pub agent: String,
    pub workdir: String,
}

impl Spec {
    fn fingerprint(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}",
            self.python, self.repo, self.client, self.server, self.agent, self.workdir
        )
    }
}

/// The client build is present when its entry page is.
pub fn client_ready(dir: &Path) -> bool {
    dir.join("index.html").is_file()
}

struct Inner {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    pending: HashSet<String>,
    fingerprint: String,
    generation: u64,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            child: None,
            stdin: None,
            pending: HashSet::new(),
            fingerprint: String::new(),
            generation: 0,
        }
    }
}

#[derive(Clone)]
pub struct IdeHost {
    inner: Arc<Mutex<Inner>>,
    on_event: OnEvent,
}

impl IdeHost {
    pub fn new(on_event: OnEvent) -> Self {
        Self {
            inner: Arc::default(),
            on_event,
        }
    }

    /// Start the host if this exact configuration is not already running.
    pub fn ensure(&self, spec: &Spec) -> bool {
        let fingerprint = spec.fingerprint();
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.fingerprint == fingerprint {
                if let Some(child) = inner.child.as_mut() {
                    if matches!(child.try_wait(), Ok(None)) {
                        return true;
                    }
                }
            }
        }
        self.spawn(spec)
    }

    pub fn stop(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.generation += 1;
        inner.pending.clear();
        inner.fingerprint.clear();
        inner.stdin = None;
        if let Some(mut child) = inner.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Write the answer if this host asked. False leaves it for the runner.
    pub fn answer(&self, id: &str, approved: bool, scope: Option<&str>) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if !inner.pending.remove(id) {
            return false;
        }
        let mut line = serde_json::json!({
            "type": "approval_response",
            "id": id,
            "approved": approved,
        });
        if let Some(scope) = scope {
            line["scope"] = serde_json::json!(scope);
        }
        if let Some(stdin) = inner.stdin.as_mut() {
            let _ = writeln!(stdin, "{line}").and_then(|_| stdin.flush());
        }
        true
    }

    fn spawn(&self, spec: &Spec) -> bool {
        self.stop();
        let mut child = match Command::new(&spec.python)
            .args([
                "-m",
                "pi_compat.host",
                "--root",
                &spec.workdir,
                "--server",
                &spec.server,
                "--agent",
                &spec.agent,
                "--client",
                &spec.client,
                "--host",
                "127.0.0.1",
                "--port",
                &PORT.to_string(),
                "--desktop",
            ])
            .current_dir(&spec.repo)
            .env("PYTHONPATH", &spec.repo)
            .env("PYTHONUNBUFFERED", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                eprintln!("[ide] could not start {}: {err}", spec.python);
                return false;
            }
        };
        let stdout = child.stdout.take().expect("piped stdout");
        let generation = {
            let mut inner = self.inner.lock().unwrap();
            inner.stdin = child.stdin.take();
            inner.child = Some(child);
            inner.fingerprint = spec.fingerprint();
            inner.generation
        };
        let this = self.clone();
        thread::spawn(move || this.read_stdout(stdout, generation));
        true
    }

    fn read_stdout(&self, stdout: impl std::io::Read, generation: u64) {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if self.inner.lock().unwrap().generation != generation {
                return;
            }
            let Some(event) = parse_line(&line) else {
                eprintln!("[ide] {line}");
                continue;
            };
            if let Event::ApprovalRequest { id, .. } = &event {
                self.inner.lock().unwrap().pending.insert(id.clone());
                (self.on_event)(event);
            } else if matches!(event, Event::ApprovalExpired { .. }) {
                (self.on_event)(event);
            } else {
                eprintln!("[ide] {line}");
            }
        }
        let mut inner = self.inner.lock().unwrap();
        if inner.generation == generation {
            inner.stdin = None;
            inner.fingerprint.clear();
            if let Some(mut child) = inner.child.take() {
                let _ = child.wait();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_dir_needs_its_entry_page() {
        let dir = std::env::temp_dir().join(format!("virtuai-ide-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!client_ready(&dir));
        std::fs::write(dir.join("index.html"), "ok").unwrap();
        assert!(client_ready(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_answer_without_a_question_is_not_ours() {
        let host = IdeHost::new(std::sync::Arc::new(|_| {}));
        assert!(!host.answer("missing", true, None));
    }

    #[test]
    fn the_fingerprint_changes_with_the_agent() {
        let mut spec = Spec {
            python: "python".into(),
            repo: "/repo".into(),
            client: "/client".into(),
            server: "https://example.test".into(),
            agent: "one".into(),
            workdir: "/work".into(),
        };
        let first = spec.fingerprint();
        spec.agent = "two".into();
        assert_ne!(first, spec.fingerprint());
    }
}
