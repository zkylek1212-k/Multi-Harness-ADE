// Workspace files and the watcher that tells each window what agents changed.
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::{json, Value};

use crate::core::{arg_str, Core};
use crate::util::{exists, locale_cmp, p, resolve};

const IGNORED: [&str; 4] = [".git", "node_modules", "out", ".deps"];
const WATCH_IGNORED_DIRS: [&str; 15] = [".git", "node_modules", "out", "dist", "build", ".deps", ".workbench", ".gemini", ".project-memory", ".system_generated", ".vscode", ".idea", ".next", ".nuxt", "coverage"];
const EDITOR_EXTS: [&str; 36] = [
    "ts", "tsx", "js", "jsx", "mjs", "cjs", "json", "css", "scss", "less", "html", "htm", "md", "markdown", "py", "rs", "go", "java", "c", "cpp", "h", "hpp", "sh", "bash", "zsh", "yaml", "yml", "toml", "ini", "sql", "xml", "svg", "docx", "xlsx", "pptx", "pdf",
];

struct WatchEntry {
    _watcher: RecommendedWatcher,
    windows: HashSet<u32>,
}

#[derive(Default)]
struct Pending {
    tree: HashMap<String, Instant>,
    files: HashMap<String, (String, String, String, Instant)>, // key -> full path, relative path, workspace key, due
    /// Saved from the editor: not an outside change
    suppressed: HashMap<String, Instant>,
}

#[derive(Default)]
pub struct Watchers {
    active: Mutex<HashMap<String, WatchEntry>>,
    pending: Mutex<Pending>,
}

fn norm_ws(path: &str) -> String {
    resolve(path).to_lowercase()
}

pub fn trigger_tree_change(core: &Core, ws: &str) {
    core.watchers.pending.lock().unwrap().tree.insert(norm_ws(ws), Instant::now() + Duration::from_millis(250));
}

fn on_watch_event(core: &Core, ws: &str, norm: &str, file: &Path) {
    let Ok(rel) = file.strip_prefix(ws) else { return };
    let rel = rel.to_string_lossy().replace('\\', "/");
    if rel.is_empty() || rel.split('/').any(|part| WATCH_IGNORED_DIRS.contains(&part.to_lowercase().as_str())) {
        return;
    }
    let lower = rel.to_lowercase();
    if [".tmp", ".log", ".swp", ".lock", "package-lock.json", "pnpm-lock.yaml", "yarn.lock"].iter().any(|e| lower.ends_with(e)) {
        return;
    }
    let mut pending = core.watchers.pending.lock().unwrap();
    pending.tree.insert(norm.to_string(), Instant::now() + Duration::from_millis(250));
    let ext = lower.rsplit('.').next().unwrap_or("");
    if !lower.contains('.') || !EDITOR_EXTS.contains(&ext) {
        return;
    }
    let full = p(file);
    let key = full.to_lowercase();
    if pending.suppressed.contains_key(&key) {
        return;
    }
    // Agents write in several chunks: wait for it to settle
    pending.files.insert(key, (full, rel, norm.to_string(), Instant::now() + Duration::from_millis(300)));
}

pub fn start_watch_timer(core: &Arc<Core>) {
    let weak: Weak<Core> = Arc::downgrade(core);
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(50));
        let Some(core) = weak.upgrade() else { return };
        let now = Instant::now();
        let (tree, files) = {
            let mut pd = core.watchers.pending.lock().unwrap();
            pd.suppressed.retain(|_, until| *until > now);
            let tree: Vec<String> = pd.tree.iter().filter(|(_, d)| **d <= now).map(|(k, _)| k.clone()).collect();
            for k in &tree {
                pd.tree.remove(k);
            }
            let keys: Vec<String> = pd.files.iter().filter(|(_, v)| v.3 <= now).map(|(k, _)| k.clone()).collect();
            let files: Vec<_> = keys.iter().filter_map(|k| pd.files.remove(k)).collect();
            (tree, files)
        };
        if tree.is_empty() && files.is_empty() {
            continue;
        }
        let active = core.watchers.active.lock().unwrap();
        for k in tree {
            if let Some(e) = active.get(&k) {
                for w in &e.windows {
                    core.host.send(*w, "files:treeChange", json!([]));
                }
            }
        }
        for (full, rel, ws, _) in files {
            if !fs::metadata(&full).map(|m| m.is_file()).unwrap_or(false) {
                continue;
            }
            if let Some(e) = active.get(&ws) {
                for w in &e.windows {
                    core.host.send(*w, "files:externalChange", json!([{ "path": full, "relativePath": rel, "eventType": "change" }]));
                }
            }
        }
    });
}

pub fn attach_window(core: &Arc<Core>, window: u32, workspace: &str) {
    if workspace.is_empty() || !exists(workspace) {
        return;
    }
    let norm = norm_ws(workspace);
    {
        let mut active = core.watchers.active.lock().unwrap();
        active.retain(|k, e| {
            if *k != norm {
                e.windows.remove(&window);
            }
            *k == norm || !e.windows.is_empty()
        });
        if let Some(e) = active.get_mut(&norm) {
            e.windows.insert(window);
        } else {
            let weak = Arc::downgrade(core);
            let (ws, norm2) = (resolve(workspace), norm.clone());
            let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                let (Ok(ev), Some(core)) = (res, weak.upgrade()) else { return };
                for path in &ev.paths {
                    on_watch_event(&core, &ws, &norm2, path);
                }
            });
            let Ok(mut watcher) = watcher else { return };
            if watcher.watch(Path::new(workspace), RecursiveMode::Recursive).is_err() {
                return;
            }
            active.insert(norm, WatchEntry { _watcher: watcher, windows: HashSet::from([window]) });
        }
    }
    trigger_tree_change(core, workspace);
}

pub fn detach_window(core: &Arc<Core>, window: u32) {
    core.watchers.active.lock().unwrap().retain(|_, e| {
        e.windows.remove(&window);
        !e.windows.is_empty()
    });
}

/// Requests from a window stay inside that window's workspace
fn safe_path(target: &str, root: &str) -> Result<String, String> {
    if root.is_empty() {
        return Err("Workspace root is not defined".into());
    }
    let root_abs = resolve(root);
    let abs = if Path::new(target).is_absolute() { resolve(target) } else { resolve(&p(&PathBuf::from(&root_abs).join(target))) };
    let (nr, nt) = (root_abs.to_lowercase(), abs.to_lowercase());
    let with_sep = if nr.ends_with('\\') || nr.ends_with('/') { nr.clone() } else { format!("{nr}{}", std::path::MAIN_SEPARATOR) };
    if nt != nr && !nt.starts_with(&with_sep) {
        return Err(format!("Access denied: path \"{target}\" resolves outside workspace root \"{root}\""));
    }
    Ok(abs)
}

fn detect_doc_tools() -> Value {
    if !cfg!(windows) {
        return json!({ "word": "", "excel": "", "powerpoint": "", "pdf": "" });
    }
    let var = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
    let pf = var("ProgramFiles", "C:\\Program Files");
    let x86 = var("ProgramFiles(x86)", "C:\\Program Files (x86)");
    let local = var("LOCALAPPDATA", "");
    let office = |exe: &str| -> Vec<String> {
        let mut v = Vec::new();
        for sub in ["Microsoft Office\\root\\Office16", "Microsoft Office\\Office16", "Microsoft Office\\Office15"] {
            v.push(format!("{pf}\\{sub}\\{exe}"));
            v.push(format!("{x86}\\{sub}\\{exe}"));
        }
        v.push(format!("{local}\\Kingsoft\\WPS Office\\ksolaunch.exe"));
        v
    };
    let first = |c: Vec<String>| c.into_iter().find(|x| exists(x)).unwrap_or_default();
    json!({
        "word": first(office("WINWORD.EXE")),
        "excel": first(office("EXCEL.EXE")),
        "powerpoint": first(office("POWERPNT.EXE")),
        "pdf": first(vec![
            format!("{pf}\\Adobe\\Acrobat DC\\Acrobat\\Acrobat.exe"),
            format!("{x86}\\Adobe\\Acrobat Reader DC\\Reader\\AcroRd32.exe"),
            format!("{pf}\\SumatraPDF\\SumatraPDF.exe"),
            format!("{local}\\SumatraPDF\\SumatraPDF.exe"),
            format!("{x86}\\Microsoft\\Edge\\Application\\msedge.exe"),
            format!("{pf}\\Microsoft\\Edge\\Application\\msedge.exe"),
            format!("{pf}\\Google\\Chrome\\Application\\chrome.exe"),
            format!("{x86}\\Google\\Chrome\\Application\\chrome.exe"),
        ]),
    })
}

fn open_external(core: &Core, target: &str, tool: Option<&str>) -> Value {
    let run = || -> Result<(), String> {
        let safe = safe_path(target, &core.workspace_root())?;
        let ext = safe.rsplit('.').next().unwrap_or("").to_lowercase();
        let chosen = tool.map(str::trim).filter(|t| !t.is_empty()).map(str::to_string).or_else(|| match ext.as_str() {
            "docx" | "doc" => core.custom_doc_tool("word"),
            "xlsx" | "xls" => core.custom_doc_tool("excel"),
            "pptx" | "ppt" => core.custom_doc_tool("powerpoint"),
            "pdf" => core.custom_doc_tool("pdf"),
            _ => None,
        });
        if let Some(exe) = chosen.filter(|e| exists(e)) {
            std::process::Command::new(exe).arg(&safe).spawn().map(|_| ()).map_err(|e| e.to_string())
        } else {
            core.host.open_path(&safe)
        }
    };
    match run() {
        Ok(()) => json!({ "ok": true }),
        Err(e) => json!({ "ok": false, "error": e }),
    }
}

pub fn call(core: &Arc<Core>, window: u32, channel: &str, args: &[Value]) -> Result<Value, String> {
    let ws = core.workspace_for(Some(window));
    let a0 = arg_str(args, 0);
    match channel {
        "files:read" => {
            let b = fs::read(safe_path(a0, &ws)?).map_err(|e| e.to_string())?;
            Ok(json!(String::from_utf8_lossy(&b)))
        }
        "files:write" => {
            let safe = safe_path(a0, &ws)?;
            let until = Instant::now() + Duration::from_secs(1);
            core.watchers.pending.lock().unwrap().suppressed.insert(safe.to_lowercase(), until);
            if let Some(d) = Path::new(&safe).parent() {
                fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            fs::write(&safe, arg_str(args, 1)).map_err(|e| e.to_string())?;
            trigger_tree_change(core, &ws);
            Ok(Value::Null)
        }
        "files:list" => {
            if a0.trim().is_empty() || ws.is_empty() {
                return Ok(json!([]));
            }
            let safe = safe_path(a0, &ws)?;
            let mut rows: Vec<(String, bool)> = fs::read_dir(&safe)
                .map_err(|e| e.to_string())?
                .flatten()
                .map(|e| (e.file_name().to_string_lossy().into_owned(), e.file_type().map(|t| t.is_dir()).unwrap_or(false)))
                .filter(|(n, _)| !IGNORED.contains(&n.as_str()))
                .collect();
            rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| locale_cmp(&a.0, &b.0)));
            Ok(rows.into_iter().map(|(name, is_dir)| json!({ "path": p(&Path::new(&safe).join(&name)), "name": name, "isDir": is_dir })).collect())
        }
        "files:exists" => Ok(json!(safe_path(a0, &ws).map(|s| exists(&s)).unwrap_or(false))),
        "files:workspaceRoot" => Ok(json!(ws)),
        "files:pickWorkspace" => {
            let Some(picked) = core.host.pick_folder(window, &ws) else { return Ok(Value::Null) };
            core.set_workspace_for_window(window, &picked);
            core.invalidate_dashboard_cache();
            trigger_tree_change(core, &picked);
            Ok(json!(picked))
        }
        "files:setWorkspaceRoot" => {
            if a0.is_empty() || !exists(a0) {
                return Ok(json!(false));
            }
            core.set_workspace_for_window(window, a0);
            core.unmark_workspace(a0);
            core.invalidate_dashboard_cache();
            trigger_tree_change(core, a0);
            Ok(json!(true))
        }
        "files:openExternal" => Ok(open_external(core, a0, args.get(1).and_then(Value::as_str))),
        // The path comes from here, never from the page
        "files:revealWorkspace" => {
            if ws.is_empty() || !exists(&ws) {
                return Ok(json!({ "ok": false, "error": "no workspace folder" }));
            }
            Ok(match core.host.open_path(&ws) {
                Ok(()) => json!({ "ok": true }),
                Err(e) => json!({ "ok": false, "error": e }),
            })
        }
        "files:showInFolder" => {
            core.host.show_in_folder(&safe_path(a0, &ws)?);
            Ok(Value::Null)
        }
        "files:stat" => {
            let m = fs::metadata(safe_path(a0, &ws)?).map_err(|e| e.to_string())?;
            let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as f64).unwrap_or(0.0);
            Ok(json!({ "size": m.len(), "mtime": crate::util::iso(mtime), "isFile": m.is_file() }))
        }
        "files:pickExecutable" => {
            let title = args.first().and_then(Value::as_str).unwrap_or("Select Application Executable");
            Ok(core.host.pick_file(title).map(Value::String).unwrap_or(Value::Null))
        }
        "files:detectDocTools" => Ok(detect_doc_tools()),
        _ => Err(format!("No handler registered for '{channel}'")),
    }
}
