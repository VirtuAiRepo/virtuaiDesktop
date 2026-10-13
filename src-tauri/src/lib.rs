//! VirtuAI Desktop: the server's web chat in a native window, plus the local
//! connector (`virtuai run`) supervised from the menu bar.
//!
//! Design: docs/workitems/desktop-connector.md. The window loads the server's
//! own web chat, so the app ships no copy of the frontend and the remote page
//! gets no IPC. The only bridge from the page to the app is navigating to a
//! `virtuai://` link, which `on_navigation` intercepts.

mod runner;
mod settings;
#[cfg(target_os = "macos")]
mod titlebar;

use runner::{Event, Runner};
use settings::Settings;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::ipc::CapabilityBuilder;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder, WindowEvent, Wry};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_opener::OpenerExt;
use url::Url;

const MAIN: &str = "main";

/// Lets the web chat know it runs inside the app, on which OS (macOS gets a
/// title bar drawn by the page, frontend/src/desktop/), and how to ask it to
/// pair (frontend/src/components/CliPairingCard.tsx). Runs on every page the
/// window loads; it grants nothing, it only names a link.
fn desktop_script() -> String {
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut script = format!(
        r#"
window.__VIRTUAI_DESKTOP__ = Object.freeze({{
  version: "{version}",
  platform: "{platform}",
  pair: (code) => {{ window.location.href = "virtuai://pair?code=" + encodeURIComponent(code); }},
  openWorkdir: () => {{ window.location.href = "virtuai://workdir/open"; }},
  chooseWorkdir: () => {{ window.location.href = "virtuai://workdir/choose"; }},
}});
"#,
        version = env!("CARGO_PKG_VERSION"),
        platform = std::env::consts::OS,
    );
    #[cfg(target_os = "macos")]
    script.push_str(TITLEBAR_FALLBACK);
    script
}

/// A server whose web chat predates the app draws no title bar: its own
/// controls would sit under the traffic lights, and nothing would drag the
/// window. A page that draws the bar marks <html> (frontend/src/desktop/
/// desktopApp.ts); any other gets a plain draggable strip in its own
/// background colour.
///
/// Decided half a second after `load`, not at DOMContentLoaded: in WKWebView the web chat's module
/// script had not run yet at DOMContentLoaded, the check found no mark, and
/// the strip covered the page's real bar (z-index max). The ids let the page
/// remove a strip that got there anyway.
#[cfg(target_os = "macos")]
const TITLEBAR_FALLBACK: &str = r#"
window.addEventListener("load", () => setTimeout(() => {
  if (document.documentElement.classList.contains("desktop-titlebar")) return;
  const bar = document.createElement("div");
  bar.id = "virtuai-titlebar-fallback";
  bar.setAttribute("data-tauri-drag-region", "");
  bar.style.cssText = "position:fixed;top:0;left:0;right:0;height:40px;z-index:2147483647";
  bar.style.background = getComputedStyle(document.body).backgroundColor;
  document.body.appendChild(bar);
  const style = document.createElement("style");
  style.id = "virtuai-titlebar-fallback-style";
  style.textContent = "body{padding-top:40px!important}";
  document.head.appendChild(style);
}, 500));
"#;

/// Height of the bar the page draws on macOS; the traffic lights are centred
/// in it. Keep in step with DESKTOP_TITLEBAR_HEIGHT in frontend/src/desktop/.
#[cfg(target_os = "macos")]
const TITLEBAR_HEIGHT: f64 = 40.0;

/// The server's page draws the title bar, so it must be able to drag the
/// window by it (Tauri's `data-tauri-drag-region` script). That is all it
/// gets: two window permissions, for this one origin, and no command of ours.
fn allow_title_bar_dragging(app: &AppHandle, server: &str) {
    let id = format!(
        "titlebar-{}",
        server.replace(|c: char| !c.is_ascii_alphanumeric(), "-")
    );
    let capability = CapabilityBuilder::new(id)
        .remote(format!("{server}/*"))
        .window(MAIN)
        .permission("core:window:allow-start-dragging")
        .permission("core:window:allow-internal-toggle-maximize");
    if let Err(err) = app.add_capability(capability) {
        // Already granted (the same server set twice) is the only expected case.
        eprintln!("[app] title bar capability for {server}: {err}");
    }
}

struct AppState {
    settings: Mutex<Settings>,
    config_dir: PathBuf,
    runner: Runner,
    paused: Mutex<bool>,
    status_item: MenuItem<Wry>,
    pause_item: MenuItem<Wry>,
}

fn status_text(state: &str) -> &'static str {
    match state {
        "starting" | "connecting" => "Connector: connecting…",
        "connected" => "Connector: connected",
        "reconnecting" | "restarting" => "Connector: reconnecting…",
        "not_paired" => "Connector: not paired — open Settings → CLI",
        "rejected" => "Connector: not paired with this server — Settings → CLI → Connect",
        "missing_cli" => "Connector: VirtuAI CLI not found",
        "pairing" => "Connector: pairing…",
        "paused" => "Connector: paused",
        "error" => "Connector: error (see log)",
        _ => "Connector: off",
    }
}

fn set_status(app: &AppHandle, state: &str) {
    let _ = app.state::<AppState>().status_item.set_text(status_text(state));
}

fn on_runner_event(app: &AppHandle, event: Event) {
    match event {
        Event::State { state } => set_status(app, &state),
        Event::Status {
            level,
            message,
            state,
        } => {
            eprintln!("[runner:{level}] {message}");
            if let Some(state) = state {
                set_status(app, &state);
            }
        }
        Event::ApprovalRequest {
            id,
            command,
            reason,
            workdir,
        } => ask_approval(app, id, command, reason, workdir),
        // The dialog cannot be taken back; an answer after this is ignored
        // by the runner, and the agent was already told nobody answered.
        Event::ApprovalExpired { id } => eprintln!("[runner] approval {id} expired"),
    }
}

fn ask_approval(app: &AppHandle, id: String, command: String, reason: String, workdir: String) {
    let runner = app.state::<AppState>().runner.clone();
    let text = format!(
        "An agent wants to run this on your computer:\n\n{command}\n\nWhy you are asked: {reason}\nFolder: {workdir}"
    );
    app.dialog()
        .message(text)
        .title("VirtuAI — allow this command?")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom("Allow".into(), "Deny".into()))
        .show(move |allowed| runner.answer(&id, allowed));
}

/// What the agents must never reach: the app's own data and the app itself.
fn protected_dirs(app: &AppHandle) -> Vec<PathBuf> {
    let paths = app.path();
    let mut dirs: Vec<PathBuf> = [
        paths.app_config_dir(),
        paths.app_data_dir(),
        paths.app_local_data_dir(),
        paths.app_cache_dir(),
        paths.app_log_dir(),
        paths.resource_dir(),
    ]
    .into_iter()
    .flatten()
    .collect();
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)) {
        dirs.push(exe_dir);
    }
    dirs
}

/// The folder the agents work in: the person's choice, or ~/virtuai. A saved
/// choice that is gone or no longer allowed falls back to the default rather
/// than leaving the runner without one.
fn current_workdir(app: &AppHandle) -> Option<PathBuf> {
    let chosen = app.state::<AppState>().settings.lock().unwrap().workdir.clone();
    if let Some(chosen) = chosen {
        if let Ok(dir) = settings::validate_workdir(Path::new(&chosen), &protected_dirs(app)) {
            return Some(dir);
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let dir = settings::default_workdir(&home);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn start_runner(app: &AppHandle) {
    let state = app.state::<AppState>();
    if *state.paused.lock().unwrap() {
        return;
    }
    let server = state.settings.lock().unwrap().server_url.clone();
    match (server, current_workdir(app)) {
        (Some(server), Some(workdir)) => state.runner.start(server, workdir),
        (Some(_), None) => set_status(app, "error"),
        (None, _) => set_status(app, "off"),
    }
}

fn open_workdir(app: &AppHandle) {
    if let Some(dir) = current_workdir(app) {
        let _ = app.opener().open_path(dir.to_string_lossy(), None::<&str>);
    }
}

/// A native folder picker; the folder is checked, saved, and the runner
/// restarted in it. Refused folders say why.
fn choose_workdir(app: &AppHandle) {
    let handle = app.clone();
    app.dialog().file().set_title("Folder your agents work in").pick_folder(move |picked| {
        let Some(picked) = picked.and_then(|p| p.into_path().ok()) else {
            return;
        };
        match settings::validate_workdir(&picked, &protected_dirs(&handle)) {
            Ok(dir) => {
                let state = handle.state::<AppState>();
                let saved = {
                    let mut settings = state.settings.lock().unwrap();
                    settings.workdir = Some(dir.to_string_lossy().into_owned());
                    settings.save(&state.config_dir)
                };
                if let Err(err) = saved {
                    eprintln!("[app] could not save the working folder: {err}");
                }
                start_runner(&handle);
            }
            Err(why) => {
                handle
                    .dialog()
                    .message(why)
                    .title("VirtuAI — choose another folder")
                    .kind(MessageDialogKind::Warning)
                    .show(|_| {});
            }
        }
    });
}

/// `virtuai://pair?code=…`, sent by the web chat's own pairing button. The
/// server is always the configured one, never one named in the link: a link
/// must not be able to point this machine's runner somewhere else.
/// `virtuai://workdir/open` and `virtuai://workdir/choose` come from the
/// title bar's folder controls; neither carries a path, the person picks it.
fn handle_desktop_link(app: &AppHandle, url: &Url) {
    match (url.host_str(), url.path()) {
        (Some("workdir"), "/open") => return open_workdir(app),
        (Some("workdir"), "/choose") => return choose_workdir(app),
        (Some("pair"), _) => {}
        _ => return,
    }
    let Some(code) = url
        .query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.into_owned())
        .filter(|c| !c.is_empty() && c.len() <= 32 && c.chars().all(|ch| ch.is_ascii_alphanumeric()))
    else {
        return;
    };
    let app = app.clone();
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let Some(server) = state.settings.lock().unwrap().server_url.clone() else {
            return;
        };
        set_status(&app, "pairing");
        state.runner.stop();
        let result = runner::pair(&code, &server);
        *state.paused.lock().unwrap() = false;
        let _ = state.pause_item.set_text("Pause connector");
        start_runner(&app);
        let folder = current_workdir(&app)
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| "~/virtuai".into());
        let (kind, text) = match result {
            Ok(()) => (
                MessageDialogKind::Info,
                format!(
                    "This computer is connected. Your agents can now work on it, in {folder}; \
                     you will be asked before anything that is not clearly safe. \
                     Change the folder from the title bar or the menu bar."
                ),
            ),
            Err(err) => (MessageDialogKind::Error, err),
        };
        app.dialog().message(text).title("VirtuAI").kind(kind).show(|_| {});
    });
}

fn open_main_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(MAIN) {
        window.show()?;
        return window.set_focus();
    }
    let server = app.state::<AppState>().settings.lock().unwrap().server_url.clone();
    let url = match server.as_deref().map(Url::parse) {
        Some(Ok(url)) => WebviewUrl::External(url),
        _ => WebviewUrl::App("index.html".into()),
    };
    build_main_window(app, url)
}

fn build_main_window(app: &AppHandle, url: WebviewUrl) -> tauri::Result<()> {
    let handle = app.clone();
    let builder = WebviewWindowBuilder::new(app, MAIN, url)
        .title("VirtuAI")
        .inner_size(1200.0, 800.0)
        .min_inner_size(480.0, 480.0)
        .initialization_script(desktop_script())
        .on_navigation(move |url| {
            if url.scheme() == "virtuai" {
                handle_desktop_link(&handle, url);
                return false;
            }
            true
        });
    // The page draws the bar in its own theme (light or dark) with back,
    // forward and the sidebar button; macOS keeps only the traffic lights,
    // centred in it. The inset's y is the bar height minus the 14 pt buttons.
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(16.0, (TITLEBAR_HEIGHT + 14.0) / 2.0));
    let window = builder.build()?;
    #[cfg(target_os = "macos")]
    titlebar::narrow_to_traffic_lights(&window);
    #[cfg(not(target_os = "macos"))]
    let _ = window;
    Ok(())
}

fn show_setup(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN) {
        let _ = window.destroy();
    }
    let _ = build_main_window(app, WebviewUrl::App("index.html".into()));
}

#[tauri::command]
fn current_server(state: State<'_, AppState>) -> Option<String> {
    state.settings.lock().unwrap().server_url.clone()
}

#[tauri::command]
fn set_server(app: AppHandle, state: State<'_, AppState>, url: String) -> Result<(), String> {
    let server = settings::normalize_server(&url)?;
    {
        let mut settings = state.settings.lock().unwrap();
        settings.server_url = Some(server.clone());
        settings.save(&state.config_dir).map_err(|e| e.to_string())?;
    }
    allow_title_bar_dragging(&app, &server);
    start_runner(&app);
    let window = app.get_webview_window(MAIN).ok_or("No window.")?;
    window
        .navigate(Url::parse(&server).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

fn on_menu(app: &AppHandle, id: &str) {
    let state = app.state::<AppState>();
    match id {
        "open" => {
            let _ = open_main_window(app);
        }
        "pause" => {
            let paused = {
                let mut paused = state.paused.lock().unwrap();
                *paused = !*paused;
                *paused
            };
            if paused {
                state.runner.stop();
                set_status(app, "paused");
                let _ = state.pause_item.set_text("Resume connector");
            } else {
                let _ = state.pause_item.set_text("Pause connector");
                start_runner(app);
            }
        }
        "workdir" => open_workdir(app),
        "choose_workdir" => choose_workdir(app),
        "server" => show_setup(app),
        "quit" => {
            state.runner.stop();
            app.exit(0);
        }
        _ => {}
    }
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![current_server, set_server])
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            if let Ok(resources) = app.path().resource_dir() {
                runner::set_bundled_runner_dir(resources.join("runner"));
            }
            let settings = Settings::load(&config_dir);

            let status_item = MenuItem::with_id(app, "status", status_text("off"), false, None::<&str>)?;
            let pause_item = MenuItem::with_id(app, "pause", "Pause connector", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &status_item,
                    &PredefinedMenuItem::separator(app)?,
                    &MenuItem::with_id(app, "open", "Open VirtuAI", true, None::<&str>)?,
                    &pause_item,
                    &MenuItem::with_id(app, "workdir", "Open working folder", true, None::<&str>)?,
                    &MenuItem::with_id(app, "choose_workdir", "Choose working folder…", true, None::<&str>)?,
                    &MenuItem::with_id(app, "server", "Change server…", true, None::<&str>)?,
                    &PredefinedMenuItem::separator(app)?,
                    &MenuItem::with_id(app, "quit", "Quit VirtuAI", true, None::<&str>)?,
                ],
            )?;
            let mut tray = TrayIconBuilder::with_id("main")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .tooltip("VirtuAI")
                .on_menu_event(|app, event| on_menu(app, event.id.as_ref()));
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            let handle = app.handle().clone();
            let runner = Runner::new(Arc::new(move |event| on_runner_event(&handle, event)));
            app.manage(AppState {
                settings: Mutex::new(settings),
                config_dir,
                runner,
                paused: Mutex::new(false),
                status_item,
                pause_item,
            });

            if let Some(server) = app.state::<AppState>().settings.lock().unwrap().server_url.clone() {
                allow_title_bar_dragging(app.handle(), &server);
            }
            open_main_window(app.handle())?;
            start_runner(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            // AppKit re-lays out the title bar on these; narrow it again.
            #[cfg(target_os = "macos")]
            if matches!(event, WindowEvent::Resized(_) | WindowEvent::Focused(_)) {
                if let Some(webview_window) = window.app_handle().get_webview_window(window.label()) {
                    titlebar::narrow_to_traffic_lights(&webview_window);
                }
            }
            // Closing the window keeps the connector running in the menu bar;
            // "Quit VirtuAI" is what stops it.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building VirtuAI Desktop");

    app.run(|app, event| match event {
        RunEvent::ExitRequested { .. } | RunEvent::Exit => {
            app.state::<AppState>().runner.stop();
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => {
            let _ = open_main_window(app);
        }
        _ => {}
    });
}
