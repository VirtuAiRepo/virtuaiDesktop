//! What the app remembers between launches: the server address and the
//! folder the agents work in. The runner's token lives where `virtuai pair`
//! puts it (the system keychain).

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use url::Url;

const FILE: &str = "settings.json";

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub server_url: Option<String>,
    /// Where the agents work. None until the person picks one: ~/virtuai.
    #[serde(default)]
    pub workdir: Option<String>,
}

/// ~/virtuai, the folder the CLI's own messages and the pairing card name.
pub fn default_workdir(home: &Path) -> PathBuf {
    home.join("virtuai")
}

/// The folder the agents may work in, or why not.
///
/// The runner jails every command to this folder, so the folder *is* what an
/// agent can reach. It must not expose the app itself: not the root of the
/// disk, and nothing that contains, or sits inside, the app's own data or
/// resources (`protected`) — which also rules out the home folder as a whole,
/// since the app's data lives under it. Launched without one, the runner used
/// its current directory: the app's source in development, `/` from Finder.
pub fn validate_workdir(chosen: &Path, protected: &[PathBuf]) -> Result<PathBuf, String> {
    let dir = chosen
        .canonicalize()
        .map_err(|_| format!("{} is not a folder this app can open.", chosen.display()))?;
    if !dir.is_dir() {
        return Err(format!("{} is not a folder.", dir.display()));
    }
    if dir.parent().is_none() {
        return Err("The whole disk cannot be the working folder: pick a folder inside it.".into());
    }
    for guarded in protected {
        let guarded = guarded.canonicalize().unwrap_or_else(|_| guarded.clone());
        if guarded.starts_with(&dir) {
            return Err(format!(
                "{} contains this app's own data. Pick a folder for your work instead, such as ~/virtuai or a project folder.",
                dir.display()
            ));
        }
        if dir.starts_with(&guarded) {
            return Err(format!("{} is inside this app's own data.", dir.display()));
        }
    }
    Ok(dir)
}

impl Settings {
    pub fn load(dir: &Path) -> Self {
        fs::read_to_string(dir.join(FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        fs::write(dir.join(FILE), text)
    }
}

/// The server as an origin (`https://host[:port]`), or why it cannot be one.
///
/// Plain http is accepted only for this machine: the window carries a login
/// token, and the runner it pairs will run commands for whoever the server
/// says.
pub fn normalize_server(input: &str) -> Result<String, String> {
    let input = input.trim();
    let with_scheme = if input.contains("://") {
        input.to_string()
    } else {
        format!("https://{input}")
    };
    let url = Url::parse(&with_scheme).map_err(|_| "That is not a web address.".to_string())?;
    let host = url
        .host_str()
        .ok_or_else(|| "That address has no host.".to_string())?;
    match url.scheme() {
        "https" => {}
        "http" if matches!(host, "localhost" | "127.0.0.1" | "[::1]") => {}
        "http" => return Err("Use an https:// address.".into()),
        _ => return Err("Use an https:// address.".into()),
    }
    Ok(url.origin().ascii_serialization())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_the_origin() {
        assert_eq!(
            normalize_server(" https://virtuai.example.com/chat?x=1 ").unwrap(),
            "https://virtuai.example.com"
        );
    }

    #[test]
    fn a_bare_host_means_https() {
        assert_eq!(
            normalize_server("virtuai.example.com").unwrap(),
            "https://virtuai.example.com"
        );
    }

    #[test]
    fn http_only_for_this_machine() {
        assert_eq!(
            normalize_server("http://localhost:8000").unwrap(),
            "http://localhost:8000"
        );
        assert!(normalize_server("http://virtuai.example.com").is_err());
        assert!(normalize_server("file:///etc/passwd").is_err());
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("virtuai-wd-{}-{name}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_work_folder_is_accepted() {
        let home = scratch("home-ok");
        let work = home.join("virtuai");
        fs::create_dir_all(&work).unwrap();
        let app_data = home.join("Library/Application Support/com.ucloudstore.virtuai.desktop");
        fs::create_dir_all(&app_data).unwrap();
        assert_eq!(validate_workdir(&work, &[app_data]).unwrap(), work.canonicalize().unwrap());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn nothing_that_holds_or_is_inside_the_apps_data() {
        let home = scratch("home-guard");
        let app_data = home.join("Library/Application Support/com.ucloudstore.virtuai.desktop");
        fs::create_dir_all(app_data.join("cache")).unwrap();
        let protected = [app_data.clone()];
        // The home folder holds the app's data; so does anything above it.
        assert!(validate_workdir(&home, &protected).is_err());
        assert!(validate_workdir(&home.join("Library"), &protected).is_err());
        assert!(validate_workdir(&app_data, &protected).is_err());
        assert!(validate_workdir(&app_data.join("cache"), &protected).is_err());
        assert!(validate_workdir(Path::new("/"), &protected).is_err());
        assert!(validate_workdir(&home.join("missing"), &protected).is_err());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn an_older_settings_file_has_no_workdir() {
        let dir = scratch("old-settings");
        fs::write(dir.join(FILE), r#"{"server_url":"https://a.example"}"#).unwrap();
        let loaded = Settings::load(&dir);
        assert_eq!(loaded.server_url.as_deref(), Some("https://a.example"));
        assert_eq!(loaded.workdir, None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("virtuai-settings-{}", std::process::id()));
        let saved = Settings {
            server_url: Some("https://a.example".into()),
            workdir: Some("/Users/ana/virtuai".into()),
        };
        saved.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir).server_url.as_deref(), Some("https://a.example"));
        let _ = fs::remove_dir_all(&dir);
    }
}
