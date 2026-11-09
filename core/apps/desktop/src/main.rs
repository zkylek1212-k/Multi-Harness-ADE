// The desktop app: windows, dialogs and shell calls, plus the pipe between each page and wb-core.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(any(debug_assertions, feature = "e2e"))]
mod e2e;
mod jumplist;
mod legacy_storage;
mod updates;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::webview::{PageLoadEvent, WebviewBuilder};
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, RunEvent, Url, Webview, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_dialog::DialogExt;
use wb_core::{AppInfo, Core, Host};

const APP_ID: &str = "io.github.zkylek1212-k.agent-workbench";
const TERMINAL_WINDOW: u32 = u32::MAX;
const TERMINAL_LABEL: &str = "terminal";
/// Output merged per window before crossing into the page. Measured in the prototype: IPC messages, not bytes, are the cost.
const FLUSH_EVERY: Duration = Duration::from_millis(8);
/// Events for a page that has not subscribed yet
const MAX_QUEUED: usize = 4096;

#[derive(Default)]
struct Pipe {
    queue: Vec<String>,
    channel: Option<Channel<InvokeResponseBody>>,
}

#[derive(Default)]
struct DesktopHost {
    app: OnceLock<AppHandle>,
    pipes: Mutex<HashMap<u32, Pipe>>,
    focused: Mutex<Option<u32>>,
    next_window: AtomicU32,
    /// Child webviews by (window, id), so a page can only touch its own
    children: Mutex<HashMap<(u32, u64), Webview>>,
}

fn label_of(window: u32) -> String {
    if window == TERMINAL_WINDOW {
        TERMINAL_LABEL.to_string()
    } else {
        format!("w{window}")
    }
}

fn id_of(label: &str) -> Option<u32> {
    if label == TERMINAL_LABEL {
        return Some(TERMINAL_WINDOW);
    }
    label.strip_prefix('w')?.parse().ok()
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// ShellExecute: the default app for a file or link
fn shell_open(target: &str) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let (verb, path) = (wide("open"), wide(target));
    let r = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), path.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
    if r as isize > 32 {
        Ok(())
    } else {
        Err(format!("Could not open {target} (shell error {})", r as isize))
    }
}

impl DesktopHost {
    fn handle(&self) -> &AppHandle {
        self.app.get().expect("host starts with the app")
    }

    fn push(&self, window: u32, item: String) {
        let mut pipes = self.pipes.lock().unwrap();
        let pipe = pipes.entry(window).or_default();
        if pipe.channel.is_none() && pipe.queue.len() >= MAX_QUEUED {
            pipe.queue.remove(0);
        }
        pipe.queue.push(item);
    }

    fn flush(&self) {
        let mut pipes = self.pipes.lock().unwrap();
        for pipe in pipes.values_mut() {
            let Some(channel) = &pipe.channel else { continue };
            if pipe.queue.is_empty() {
                continue;
            }
            let body = format!("[{}]", pipe.queue.join(","));
            pipe.queue.clear();
            let _ = channel.send(InvokeResponseBody::Json(body));
        }
    }

    fn open_window(self: &Arc<Self>, core: &Arc<Core>, requested: Option<&str>) -> Result<(), String> {
        let id = self.next_window.fetch_add(1, Ordering::SeqCst) + 1;
        let ws = core.register_window(id, requested);
        let url = if ws.is_empty() { "index.html".to_string() } else { format!("index.html?workspace={}", encode_query(&ws)) };
        let window = WebviewWindowBuilder::new(self.handle(), label_of(id), WebviewUrl::App(PathBuf::from(url)))
            .title(Core::window_title(&ws))
            .inner_size(1440.0, 900.0)
            .min_inner_size(900.0, 600.0)
            .decorations(false)
            .background_color(tauri::webview::Color(0x16, 0x16, 0x18, 0xff))
            // Off, or WebView2's own HTML5 drag and drop (panels, tabs) stops working
            .disable_drag_drop_handler();
        with_test_port(window).build().map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// Error capture for the end-to-end checks, in the builds that have them
fn with_test_port<'a, R: tauri::Runtime, M: Manager<R>>(b: WebviewWindowBuilder<'a, R, M>) -> WebviewWindowBuilder<'a, R, M> {
    #[cfg(any(debug_assertions, feature = "e2e"))]
    if std::env::var_os("WB_E2E_PORT").is_some() {
        return b.initialization_script(e2e::INIT);
    }
    b
}

fn encode_query(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The Core asks the host for windows; the host needs the Core to open one
static SELF: OnceLock<(Arc<DesktopHost>, Arc<Core>)> = OnceLock::new();

impl Host for DesktopHost {
    /// The phone app ships inside the executable, same as the desktop UI
    fn static_asset(&self, rel: &str) -> Option<Vec<u8>> {
        self.app.get()?.asset_resolver().get(format!("/{rel}")).map(|a| a.bytes)
    }

    fn send(&self, window: u32, channel: &str, args: Value) {
        self.push(window, json!([channel, args]).to_string());
    }

    fn send_all(&self, channel: &str, args: Value) {
        let item = json!([channel, args]).to_string();
        for w in self.windows() {
            self.push(w, item.clone());
        }
    }

    fn focused_window(&self) -> Option<u32> {
        *self.focused.lock().unwrap()
    }

    fn windows(&self) -> Vec<u32> {
        self.handle().windows().keys().filter_map(|l| id_of(l)).collect()
    }

    fn pick_folder(&self, window: u32, default_path: &str) -> Option<String> {
        let mut d = self.handle().dialog().file();
        if let Some(w) = self.handle().get_window(&label_of(window)) {
            d = d.set_parent(&w);
        }
        if !default_path.is_empty() {
            d = d.set_directory(default_path);
        }
        d.blocking_pick_folder().and_then(|p| p.into_path().ok()).map(|p| p.to_string_lossy().into_owned())
    }

    fn pick_file(&self, title: &str) -> Option<String> {
        self.handle()
            .dialog()
            .file()
            .set_title(title)
            .add_filter("Executables (*.exe, *.cmd, *.bat)", &["exe", "cmd", "bat"])
            .add_filter("All Files (*.*)", &["*"])
            .blocking_pick_file()
            .and_then(|p| p.into_path().ok())
            .map(|p| p.to_string_lossy().into_owned())
    }

    fn open_path(&self, path: &str) -> Result<(), String> {
        shell_open(path)
    }

    fn show_in_folder(&self, path: &str) {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("explorer.exe").raw_arg(format!("/select,\"{path}\"")).spawn();
    }

    fn open_url(&self, url: &str) {
        if is_external(url) {
            let _ = shell_open(url);
        }
    }

    fn notify(&self, title: &str, body: &str) {
        use tauri_winrt_notification::Toast;
        let aumid = if cfg!(debug_assertions) { Toast::POWERSHELL_APP_ID } else { APP_ID };
        let app = self.handle().clone();
        let host = SELF.get().map(|s| s.0.clone());
        // A click brings a window forward, so the terminal waiting for approval can be found
        let _ = Toast::new(aumid)
            .title(title)
            .text1(body)
            .on_activated(move |_| {
                let label = host.as_ref().and_then(|h| h.windows().first().copied()).map(label_of);
                if let Some(w) = label.and_then(|l| app.get_window(&l)) {
                    let _ = w.unminimize();
                    let _ = w.set_focus();
                }
                Ok(())
            })
            .show();
    }

    fn set_title(&self, window: u32, title: &str) {
        if let Some(w) = self.handle().get_window(&label_of(window)) {
            let _ = w.set_title(title);
        }
    }

    fn open_project_window(&self, workspace: Option<&str>) -> bool {
        let Some((host, core)) = SELF.get() else { return false };
        if let Some(target) = workspace.filter(|t| Path::new(t).exists()) {
            let norm = wb_core::util::resolve(target).to_lowercase();
            for (id, ws) in core.project_workspaces() {
                if !ws.is_empty() && wb_core::util::resolve(&ws).to_lowercase() == norm {
                    if let Some(w) = self.handle().get_window(&label_of(id)) {
                        let _ = w.unminimize();
                        let _ = w.show();
                        let _ = w.set_focus();
                        return true;
                    }
                }
            }
        }
        host.open_window(core, workspace).is_ok()
    }

    fn open_terminal_window(&self) -> bool {
        let app = self.handle();
        if let Some(w) = app.get_window(TERMINAL_LABEL) {
            let _ = w.set_focus();
            return true;
        }
        let window = WebviewWindowBuilder::new(app, TERMINAL_LABEL, WebviewUrl::App(PathBuf::from("index.html?mode=terminal-detached")))
            .title("Agent Terminals - Agent Workbench")
            .inner_size(1100.0, 700.0)
            .min_inner_size(600.0, 400.0)
            .decorations(false)
            .background_color(tauri::webview::Color(0x16, 0x16, 0x18, 0xff))
            .disable_drag_drop_handler();
        with_test_port(window).build().is_ok()
    }

    fn close_terminal_window(&self) -> bool {
        if let Some(w) = self.handle().get_window(TERMINAL_LABEL) {
            let _ = w.close();
        }
        true
    }

    fn set_title_bar_theme(&self, _window: u32, _theme: &str) -> bool {
        true // The page draws the buttons and handles this itself
    }

    fn update_jump_list(&self, recent: &[String], language: Option<&str>) {
        jumplist::update(recent, language);
    }
}

fn is_external(url: &str) -> bool {
    let l = url.to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://") || l.starts_with("mailto:")
}

/// A page in a child webview can send IPC messages too. Only an app window's own page gets an answer.
fn caller(webview: &Webview) -> Result<u32, String> {
    let url = webview.url().map_err(|e| e.to_string())?;
    let own = url.scheme() == "tauri" || url.host_str() == Some("tauri.localhost");
    let window = id_of(webview.label());
    match (own, window) {
        (true, Some(id)) => Ok(id),
        _ => Err("not allowed".into()),
    }
}

fn state() -> &'static (Arc<DesktopHost>, Arc<Core>) {
    SELF.get().expect("state is set before any window exists")
}

#[tauri::command]
async fn ipc(webview: Webview, channel: String, args: Vec<Value>) -> Result<Value, String> {
    let window = caller(&webview)?;
    let core = state().1.clone();
    // Anything here may block on disk or a process; a request must not hold up another
    tauri::async_runtime::spawn_blocking(move || core.invoke(window, &channel, &args)).await.map_err(|e| e.to_string())?
}

/// In order and one batch at a time (the page waits for each): keystrokes must not overtake each other
#[tauri::command]
async fn ipc_send(webview: Webview, batch: Vec<(String, Vec<Value>)>) -> Result<(), String> {
    let window = caller(&webview)?;
    let core = state().1.clone();
    tauri::async_runtime::spawn_blocking(move || {
        for (channel, args) in batch {
            if let Err(e) = core.invoke(window, &channel, &args) {
                eprintln!("[ipc_send] {channel}: {e}");
            }
        }
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn subscribe(webview: Webview, channel: Channel<InvokeResponseBody>) -> Result<(), String> {
    let window = caller(&webview)?;
    state().0.pipes.lock().unwrap().entry(window).or_default().channel = Some(channel);
    Ok(())
}

#[tauri::command]
fn window_control(webview: Webview, action: String) -> Result<(), String> {
    caller(&webview)?;
    let w = webview.window();
    match action.as_str() {
        "minimize" => w.minimize(),
        "toggle-maximize" => {
            if w.is_maximized().unwrap_or(false) {
                w.unmaximize()
            } else {
                w.maximize()
            }
        }
        "close" => w.close(),
        "drag" => w.start_dragging(),
        _ => Ok(()),
    }
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn open_external(webview: Webview, url: String) -> Result<(), String> {
    caller(&webview)?;
    if !is_external(&url) {
        return Err("Only web links open externally".into());
    }
    shell_open(&url)
}

#[tauri::command]
fn legacy_local_storage(webview: Webview) -> Result<Value, String> {
    caller(&webview)?;
    Ok(legacy_storage::read(&state().1.data_dir))
}

fn child_url(url: &str) -> Result<Url, String> {
    let parsed: Url = url.parse().map_err(|e| format!("{e}"))?;
    // file: for the PDF viewer, as the old <webview> allowed
    if !matches!(parsed.scheme(), "http" | "https" | "file") {
        return Err(format!("Cannot open {url}"));
    }
    Ok(parsed)
}

fn child_of(window: u32, id: u64) -> Result<Webview, String> {
    state().0.children.lock().unwrap().get(&(window, id)).cloned().ok_or_else(|| "no such webview".to_string())
}

// async: creating a webview inside a sync command deadlocks on Windows (found with the prototype)
#[tauri::command]
async fn webview_create(webview: Webview, id: u64, url: String, x: f64, y: f64, w: f64, h: f64) -> Result<(), String> {
    let window = caller(&webview)?;
    let parsed = child_url(&url)?;
    let host = state().0.clone();
    let label = format!("child-{window}-{id}");
    let notify = move |event: &str, url: String| {
        if let Some((h, _)) = SELF.get() {
            h.send(window, "webview:event", json!([id, event, url]));
        }
    };
    // The old <webview> blocked popups unless asked, and nothing here asks: a page in the embedded browser opens no new windows
    let builder = WebviewBuilder::new(label, WebviewUrl::External(parsed)).on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny).on_page_load(move |wv, payload| {
        let url = wv.url().map(|u| u.to_string()).unwrap_or_default();
        match payload.event() {
            PageLoadEvent::Started => notify("did-start-loading", url),
            PageLoadEvent::Finished => {
                notify("did-navigate", url.clone());
                notify("did-stop-loading", url);
            }
        }
    });
    let child = webview.window().add_child(builder, LogicalPosition::new(x, y), LogicalSize::new(w, h)).map_err(|e| e.to_string())?;
    host.children.lock().unwrap().insert((window, id), child);
    Ok(())
}

#[tauri::command]
async fn webview_close(webview: Webview, id: u64) -> Result<(), String> {
    let window = caller(&webview)?;
    if let Some(c) = state().0.children.lock().unwrap().remove(&(window, id)) {
        c.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn webview_navigate(webview: Webview, id: u64, url: String) -> Result<(), String> {
    let window = caller(&webview)?;
    child_of(window, id)?.navigate(child_url(&url)?).map_err(|e| e.to_string())
}

#[tauri::command]
async fn webview_bounds(webview: Webview, id: u64, x: f64, y: f64, w: f64, h: f64, visible: bool) -> Result<(), String> {
    let window = caller(&webview)?;
    let c = child_of(window, id)?;
    if !visible {
        return c.hide().map_err(|e| e.to_string());
    }
    c.set_position(LogicalPosition::new(x, y)).map_err(|e| e.to_string())?;
    c.set_size(LogicalSize::new(w, h)).map_err(|e| e.to_string())?;
    c.show().map_err(|e| e.to_string())
}

#[tauri::command]
async fn webview_action(webview: Webview, id: u64, action: String) -> Result<(), String> {
    let window = caller(&webview)?;
    let c = child_of(window, id)?;
    match action.as_str() {
        "back" => c.eval("history.back()"),
        "forward" => c.eval("history.forward()"),
        "reload" => c.reload(),
        _ => Ok(()),
    }
    .map_err(|e| e.to_string())
}

fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("WB_DATA_DIR") {
        return PathBuf::from(d);
    }
    // Same folder as before: settings, secrets and phone pairing carry over. A dev build keeps its own.
    let base = PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default());
    base.join(if cfg!(debug_assertions) { "agent-workbench-dev" } else { "agent-workbench" })
}

/// Target folder or file on a command line, from a shortcut, the jump list or a second launch
fn parse_args(core: &Core, argv: &[String], cwd: &str) -> (Option<String>, bool) {
    let mut new_window = false;
    for raw in argv.iter().skip(1) {
        let arg = raw.trim_matches(['"', '\'']).trim();
        if arg.is_empty() {
            continue;
        }
        if arg == "--new-window" {
            new_window = true;
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        let abs = if Path::new(arg).is_absolute() { PathBuf::from(arg) } else { Path::new(cwd).join(arg) };
        let path = wb_core::util::resolve(&abs.to_string_lossy());
        if let Ok(m) = std::fs::metadata(&path) {
            if !core.is_protected_path(&path) {
                if m.is_dir() {
                    return (Some(path), new_window);
                }
                if m.is_file() {
                    return (Some(wb_core::util::dirname(&path)), new_window);
                }
            }
        }
    }
    (None, new_window)
}

/// The installer brings WebView2 along, but a portable copy or a removed runtime lands here: say what to do instead of failing blankly
fn require_webview2() {
    if tauri::webview_version().is_ok() {
        return;
    }
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDOK, MB_ICONERROR, MB_OKCANCEL};
    let text = wide("Agent Workbench needs the Microsoft Edge WebView2 Runtime, which is not installed.\n\nChoose OK to open the download page.");
    let title = wide("Agent Workbench");
    let answer = unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OKCANCEL | MB_ICONERROR) };
    if answer == IDOK {
        let _ = shell_open("https://developer.microsoft.com/microsoft-edge/webview2/");
    }
    std::process::exit(1);
}

fn main() {
    require_webview2();
    jumplist::set_app_id(APP_ID);
    let host = Arc::new(DesktopHost::default());
    let mut builder = tauri::Builder::default();
    if let Some(key) = updates::PUBLIC_KEY {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().pubkey(key).build());
    }
    let app = builder
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            let Some((host, core)) = SELF.get() else { return };
            let (target, new_window) = parse_args(core, &argv, &cwd);
            if target.is_some() || new_window {
                host.open_project_window(target.as_deref());
            } else if let Some(id) = core.active_project_window() {
                if let Some(w) = app.get_window(&label_of(id)) {
                    let _ = w.unminimize();
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler({
            #[cfg(any(debug_assertions, feature = "e2e"))]
            {
                tauri::generate_handler![ipc, ipc_send, subscribe, window_control, open_external, legacy_local_storage, webview_create, webview_close, webview_navigate, webview_bounds, webview_action, e2e::e2e_result]
            }
            #[cfg(not(any(debug_assertions, feature = "e2e")))]
            {
                tauri::generate_handler![ipc, ipc_send, subscribe, window_control, open_external, legacy_local_storage, webview_create, webview_close, webview_navigate, webview_bounds, webview_action]
            }
        })
        .on_window_event(|win, ev| {
            let Some(id) = id_of(win.label()) else { return };
            let Some((host, core)) = SELF.get() else { return };
            match ev {
                WindowEvent::Focused(on) => {
                    let mut f = host.focused.lock().unwrap();
                    if *on {
                        *f = Some(id);
                    } else if *f == Some(id) {
                        *f = None;
                    }
                }
                WindowEvent::Destroyed => {
                    host.pipes.lock().unwrap().remove(&id);
                    host.children.lock().unwrap().retain(|k, _| k.0 != id);
                    if id == TERMINAL_WINDOW {
                        if let Some(active) = core.active_project_window() {
                            core.host.send(active, "terminal:attached", json!([]));
                        }
                        return;
                    }
                    let (core, app) = (core.clone(), win.app_handle().clone());
                    // Killing process trees takes a moment, and this is the UI thread
                    std::thread::spawn(move || {
                        core.window_closed(id);
                        if core.project_workspaces().is_empty() {
                            if let Some(t) = app.get_window(TERMINAL_LABEL) {
                                let _ = t.close();
                            }
                            app.exit(0);
                        }
                    });
                }
                _ => {}
            }
        })
        .setup({
            let host = host.clone();
            move |app| {
                let _ = host.app.set(app.handle().clone());
                #[cfg(any(debug_assertions, feature = "e2e"))]
                e2e::serve(app.handle().clone());
                let info = AppInfo {
                    version: app.package_info().version.to_string(),
                    is_packaged: !cfg!(debug_assertions),
                    exe_path: std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
                    resources: app.path().resource_dir().unwrap_or_default(),
                };
                let as_host: Arc<dyn Host> = host.clone();
                let core = Core::new(as_host, data_dir(), info);
                let _ = SELF.set((host.clone(), core.clone()));
                if updates::PUBLIC_KEY.is_some() {
                    let stopping = core.clone();
                    core.features().updater.set_backend(Arc::new(updates::SignedUpdates::new(app.handle().clone(), Box::new(move || stopping.shutdown()))));
                }
                {
                    let host = host.clone();
                    std::thread::spawn(move || loop {
                        std::thread::sleep(FLUSH_EVERY);
                        host.flush();
                    });
                }
                let argv: Vec<String> = std::env::args().collect();
                let cwd = std::env::current_dir().map(|c| c.to_string_lossy().into_owned()).unwrap_or_default();
                let (target, _) = parse_args(&core, &argv, &cwd);
                host.open_window(&core, target.as_deref()).map_err(|e| Box::<dyn std::error::Error>::from(e))?;
                core.notify_jump_list();
                Ok(())
            }
        })
        .build(tauri::generate_context!())
        .expect("build the app");
    app.run(|_, ev| {
        if let RunEvent::Exit = ev {
            if let Some((_, core)) = SELF.get() {
                core.shutdown();
            }
        }
    });
}
