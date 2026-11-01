//! The desktop IDE's half of `HostWorkspacePort`.
//!
//! The Python service decides whether an action is allowed. This module is
//! what runs once it is: a path inside the workdir, a new file or folder, a
//! directory watch, and a login shell. The remote webview does not call this.

use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// One live watch of the workdir. Replaced when the folder changes.
static WATCH: Mutex<Option<RecommendedWatcher>> = Mutex::new(None);

/// The absolute path `raw` names, or why it is not inside `workdir`.
///
/// `..` and a symlink that leaves the folder both fail. A file that does not
/// exist yet is allowed when its parent is inside.
pub fn jail(workdir: &Path, raw: &str) -> Result<PathBuf, String> {
    if raw.is_empty() || raw.contains('\0') {
        return Err("missing path".into());
    }
    let root = workdir
        .canonicalize()
        .map_err(|_| format!("{} is not a folder", workdir.display()))?;
    let candidate = Path::new(raw);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let lexical = lexical(joined)?;
    let resolved = follow_links(&lexical, 0)?;
    if !resolved.starts_with(&root) {
        return Err(format!("outside the working directory ({})", root.display()));
    }
    Ok(resolved)
}

fn lexical(path: PathBuf) -> Result<PathBuf, String> {
    let mut acc = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => acc.push(prefix.as_os_str()),
            Component::RootDir => acc.push("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                if !acc.pop() {
                    return Err("outside the working directory".into());
                }
            }
            Component::Normal(part) => acc.push(part),
        }
    }
    Ok(acc)
}

fn follow_links(path: &Path, depth: u8) -> Result<PathBuf, String> {
    if depth > 16 {
        return Err("outside the working directory".into());
    }
    let mut cur = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => cur.push("/"),
            Component::Normal(part) => {
                cur.push(part);
                if cur.is_symlink() {
                    let target = std::fs::read_link(&cur).map_err(|err| err.to_string())?;
                    let next = if target.is_absolute() {
                        target
                    } else {
                        cur.parent().unwrap_or(Path::new("/")).join(target)
                    };
                    // Components after this link are applied to the target.
                    cur = follow_links(&lexical(next)?, depth + 1)?;
                }
            }
            Component::Prefix(prefix) => cur.push(prefix.as_os_str()),
            Component::CurDir | Component::ParentDir => {}
        }
    }
    Ok(cur)
}

pub fn create_file(workdir: &Path, raw: &str, content: &str) -> Result<(), String> {
    let path = jail(workdir, raw)?;
    if path.exists() {
        return Err("already exists".into());
    }
    if !path
        .parent()
        .filter(|parent| parent.is_dir())
        .is_some()
    {
        return Err("that file's folder is not in the workspace".into());
    }
    std::fs::write(&path, content).map_err(|err| err.to_string())
}

pub fn create_directory(workdir: &Path, raw: &str) -> Result<(), String> {
    let path = jail(workdir, raw)?;
    if path.exists() {
        return Err("already exists".into());
    }
    std::fs::create_dir(&path).map_err(|err| err.to_string())
}

/// Arm a watch on `raw` and return the names in it now.
///
/// shortcut: the callback only keeps the watcher alive, the explorer pane
/// reads the listing this returns. Upgrade when that pane subscribes to events.
pub fn watch(workdir: &Path, raw: &str) -> Result<Vec<String>, String> {
    let dir = jail(workdir, raw)?;
    if !dir.is_dir() {
        return Err("that folder is not in the workspace".into());
    }
    let mut watcher = notify::recommended_watcher(|_| {})
        .map_err(|err| err.to_string())?;
    watcher
        .watch(&dir, RecursiveMode::NonRecursive)
        .map_err(|err| err.to_string())?;
    *WATCH.lock().unwrap() = Some(watcher);
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(names)
}

/// A login shell in the workdir. The child's id, once it is running.
///
/// shortcut: the process is the host shell, not yet drawn in a pane. Upgrade
/// when the IDE pane owns the pty. Keystrokes are the person's: this does not
/// ask the command guard.
pub fn open_shell(workdir: &Path) -> Result<u32, String> {
    let dir = jail(workdir, ".")?;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let shell = if Path::new(&shell).exists() {
        shell
    } else {
        "/bin/sh".into()
    };
    let child = Command::new(shell)
        .arg("-l")
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| err.to_string())?;
    Ok(child.id())
}

/// One action, the same kinds the Python desktop adapter sends.
pub fn apply(workdir: &Path, kind: &str, raw: &str, content: Option<&str>) -> Result<String, String> {
    match kind {
        "create_file" => {
            create_file(workdir, raw, content.unwrap_or(""))?;
            Ok("created".into())
        }
        "create_directory" => {
            create_directory(workdir, raw)?;
            Ok("created".into())
        }
        "watch" => Ok(watch(workdir, raw)?.join("\n")),
        "open_shell" => Ok(open_shell(workdir)?.to_string()),
        _ => Err(format!("the desktop IDE does not run {kind}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn scratch() -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "virtuai-host-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    #[test]
    fn a_path_outside_is_refused() {
        let scratch = scratch();
        let dir = &scratch.0;
        assert!(jail(dir, "../outside").is_err());
        assert!(create_file(dir, "../note.txt", "x").is_err());
    }

    #[test]
    fn a_symlink_that_leaves_is_refused() {
        let scratch = scratch();
        let dir = &scratch.0;
        std::os::unix::fs::symlink(dir.parent().unwrap(), dir.join("leak")).unwrap();
        assert!(jail(dir, "leak/secret").is_err());
    }

    #[test]
    fn create_file_folder_and_watch_stay_inside() {
        let scratch = scratch();
        let dir = &scratch.0;
        apply(dir, "create_directory", "src", None).unwrap();
        apply(dir, "create_file", "src/main.txt", Some("hi")).unwrap();
        let listing = watch(dir, ".").unwrap();
        assert!(listing.iter().any(|name| name == "src"));
        assert_eq!(fs::read_to_string(dir.join("src/main.txt")).unwrap(), "hi");
        assert!(apply(dir, "create_file", "src/main.txt", Some("x")).is_err());
    }
}
