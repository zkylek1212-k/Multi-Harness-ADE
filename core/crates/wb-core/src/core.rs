// The backend as one object: a window asks through invoke(channel, args), the host carries the answer and any events.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Value};

use crate::bgtasks::BgCache;
use crate::files::Watchers;
use crate::pty::PtyManager;
use crate::session_scan::Scanner;
use crate::util::{basename, exists, resolve};

/// Everything that needs a real window, dialog or shell. The desktop app implements it; tests stub it.
pub trait Host: Send + Sync {
    /// args is what the UI listener receives, as a JSON array
    fn send(&self, window: u32, channel: &str, args: Value);
    fn send_all(&self, channel: &str, args: Value);
    fn focused_window(&self) -> Option<u32>;
    fn windows(&self) -> Vec<u32>;
    fn pick_folder(&self, window: u32, default_path: &str) -> Option<String>;
    fn pick_file(&self, title: &str) -> Option<String>;
    fn open_path(&self, path: &str) -> Result<(), String>;
    fn show_in_folder(&self, path: &str);
    fn open_url(&self, url: &str);
    fn notify(&self, title: &str, body: &str);
    fn set_title(&self, window: u32, title: &str);
    fn open_project_window(&self, workspace: Option<&str>) -> bool;
    fn open_terminal_window(&self) -> bool;
    fn close_terminal_window(&self) -> bool;
    fn set_title_bar_theme(&self, window: u32, theme: &str) -> bool;
    fn update_jump_list(&self, _recent: &[String], _language: Option<&str>) {}
    /// A file of the built phone app, by a path the server already validated
    fn static_asset(&self, _rel: &str) -> Option<Vec<u8>> {
        None
    }
}

#[derive(Clone, Default)]
pub struct AppInfo {
    pub version: String,
    pub is_packaged: bool,
    pub exe_path: String,
    pub resources: PathBuf,
}

#[derive(Default)]
pub struct WindowState {
    pub workspaces: HashMap<u32, String>,
    pub default_workspace: String,
}

pub struct Core {
    pub host: Arc<dyn Host>,
    pub data_dir: PathBuf,
    pub info: AppInfo,
    pub windows: Mutex<WindowState>,
    pub scanner: Scanner,
    pub bg: BgCache,
    pub watchers: Watchers,
    pub pty: PtyManager,
    pub cached_sessions: Mutex<Option<Vec<Value>>>,
    pub features: OnceLock<crate::Features>,
}

impl Core {
    pub fn new(host: Arc<dyn Host>, data_dir: PathBuf, info: AppInfo) -> Arc<Self> {
        let _ = std::fs::create_dir_all(&data_dir);
        let core = Arc::new(Self {
            host,
            scanner: Scanner::new(data_dir.clone()),
            data_dir,
            info,
            windows: Mutex::default(),
            bg: BgCache::default(),
            watchers: Watchers::default(),
            pty: PtyManager::default(),
            cached_sessions: Mutex::new(None),
            features: OnceLock::new(),
        });
        crate::files::start_watch_timer(&core);
        crate::pty::start_timer(&core);
        crate::start_features(&core);
        core
    }

    pub fn features(&self) -> &crate::Features {
        self.features.get().expect("features start with the core")
    }

    /// The workspace of the calling window; otherwise the focused window's, otherwise the last one used
    pub fn workspace_for(&self, window: Option<u32>) -> String {
        let w = self.windows.lock().unwrap();
        if let Some(ws) = window.and_then(|id| w.workspaces.get(&id)) {
            return ws.clone();
        }
        if let Some(ws) = self.host.focused_window().and_then(|id| w.workspaces.get(&id)) {
            return ws.clone();
        }
        w.default_workspace.clone()
    }

    /// The old singleton workspace: focused window, else the last one used
    pub fn workspace_root(&self) -> String {
        self.workspace_for(None)
    }

    pub fn project_workspaces(&self) -> Vec<(u32, String)> {
        let open = self.host.windows();
        self.windows.lock().unwrap().workspaces.iter().filter(|(id, _)| open.contains(id)).map(|(id, ws)| (*id, ws.clone())).collect()
    }

    pub fn active_project_window(&self) -> Option<u32> {
        let w = self.windows.lock().unwrap();
        if let Some(f) = self.host.focused_window().filter(|f| w.workspaces.contains_key(f)) {
            return Some(f);
        }
        let open = self.host.windows();
        let mut ids: Vec<u32> = w.workspaces.keys().filter(|id| open.contains(id)).copied().collect();
        ids.sort();
        ids.first().copied()
    }

    /// A new project window opened. Returns the workspace it was given.
    pub fn register_window(self: &Arc<Self>, window: u32, requested: Option<&str>) -> String {
        let ws = requested.filter(|p| exists(p)).map(resolve).unwrap_or_else(|| self.determine_initial_workspace());
        {
            let mut w = self.windows.lock().unwrap();
            w.workspaces.insert(window, ws.clone());
            w.default_workspace = ws.clone();
        }
        // Only a workspace the user chose is remembered or watched
        if !ws.is_empty() {
            self.add_recent_workspace(&ws);
            self.save_last_workspace(&ws);
            crate::files::attach_window(self, window, &ws);
        }
        ws
    }

    pub fn window_closed(self: &Arc<Self>, window: u32) {
        self.pty_cleanup_for_window(window);
        crate::files::detach_window(self, window);
        self.windows.lock().unwrap().workspaces.remove(&window);
    }

    pub fn set_workspace_for_window(self: &Arc<Self>, window: u32, path: &str) {
        {
            let mut w = self.windows.lock().unwrap();
            w.workspaces.insert(window, path.to_string());
            w.default_workspace = path.to_string();
        }
        if path.is_empty() {
            self.host.set_title(window, "Agent Workbench");
            return;
        }
        self.save_last_workspace(path);
        self.add_recent_workspace(path);
        crate::files::attach_window(self, window, path);
        self.host.set_title(window, &format!("{} — Agent Workbench", basename(path)));
    }

    pub fn window_title(ws: &str) -> String {
        if ws.is_empty() {
            "Agent Workbench".to_string()
        } else {
            format!("{} — Agent Workbench", basename(ws))
        }
    }

    pub fn determine_initial_workspace(&self) -> String {
        let Some(last) = self.get_last_workspace() else { return String::new() };
        // A workspace the user removed from the dashboard does not come back on its own
        let norm = resolve(&last).to_lowercase();
        if self.load_dashboard_state().deleted_workspaces.iter().any(|d| resolve(d).to_lowercase() == norm) {
            return String::new();
        }
        last
    }

    /// Quit: terminals end, independent programs started from them survive
    pub fn shutdown(&self) {
        crate::remote::shutdown(self);
        crate::cowork_ipc::shutdown(self);
        self.pty_shutdown();
    }

    /// Answer one UI call. Runs on a worker thread: anything here may block.
    pub fn invoke(self: &Arc<Self>, window: u32, channel: &str, args: &[Value]) -> Result<Value, String> {
        let group = channel.split(':').next().unwrap_or("");
        match group {
            "settings" => self.settings_call(channel, args),
            "files" => crate::files::call(self, window, channel, args),
            "git" => crate::git_ui::call(self, window, channel, args),
            "dashboard" => self.dashboard_call(window, channel, args),
            "ext" => crate::ext::call(self, window, channel, args),
            "conn" => crate::conn::call(self, channel, args),
            "pty" => self.pty_call(window, channel, args),
            "notify" => {
                self.host.notify(arg_str(args, 0), arg_str(args, 1));
                Ok(Value::Null)
            }
            "window" => match channel {
                "window:openTerminalWindow" => Ok(json!(self.host.open_terminal_window())),
                "window:closeTerminalWindow" => Ok(json!(self.host.close_terminal_window())),
                "window:openProjectWindow" => Ok(json!(self.host.open_project_window(args.first().and_then(Value::as_str)))),
                "window:setTitleBarTheme" => Ok(json!(self.host.set_title_bar_theme(window, arg_str(args, 0)))),
                _ => Err(format!("No handler registered for '{channel}'")),
            },
            _ => crate::features_call(self, window, channel, args),
        }
    }
}

pub fn arg_str(args: &[Value], i: usize) -> &str {
    args.get(i).and_then(Value::as_str).unwrap_or("")
}
