// The Git panel. Same answers as the panel always got; the Cowork git work lives in wb-git.
use std::fs;
use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};
use wb_git::git;

use crate::core::{arg_str, Core};
use crate::util::resolve;

fn rel_path(ws: &str, file: &str) -> String {
    let path = Path::new(file);
    let rel = if path.is_absolute() {
        path.strip_prefix(ws).map(|r| r.to_string_lossy().into_owned()).unwrap_or_else(|_| file.to_string())
    } else {
        file.to_string()
    };
    rel.replace('\\', "/")
}

fn is_repo(ws: &Path) -> bool {
    !ws.as_os_str().is_empty() && git(ws, &["rev-parse", "--is-inside-work-tree"]).map(|o| o.trim() == "true").unwrap_or(false)
}

/// Binary content shows as a note instead of a garbled diff
fn looks_binary(s: &str) -> bool {
    s.chars().take(1000).any(|c| matches!(c, '\x00'..='\x08' | '\x0e'..='\x1f'))
}

fn status(ws: &Path) -> Value {
    let empty = json!({ "isRepo": false, "current": "", "staged": [], "unstaged": [], "untracked": [] });
    if !is_repo(ws) {
        return empty;
    }
    let Ok(out) = git(ws, &["status", "--porcelain", "-b", "-u", "--null"]) else { return empty };
    let (mut current, mut staged, mut unstaged, mut untracked) = (String::new(), vec![], vec![], vec![]);
    let mut parts = out.split('\0');
    while let Some(entry) = parts.next() {
        if entry.is_empty() {
            continue;
        }
        if let Some(rest) = entry.strip_prefix("## ") {
            let rest = rest.strip_prefix("No commits yet on ").or(rest.strip_prefix("Initial commit on ")).unwrap_or(rest);
            current = if rest.starts_with("HEAD (no branch)") { "HEAD".to_string() } else { rest.split("...").next().unwrap_or(rest).to_string() };
            continue;
        }
        if entry.len() < 4 {
            continue;
        }
        let (index, work, path) = (&entry[..1], &entry[1..2], &entry[3..]);
        // A rename's next field is the old name
        if index == "R" || index == "C" {
            parts.next();
        }
        let row = json!({ "path": path, "index": index, "working_dir": work });
        if !index.trim().is_empty() && index != "?" {
            staged.push(row.clone());
        }
        if !work.trim().is_empty() {
            unstaged.push(row);
        }
        if index == "?" && work == "?" {
            untracked.push(path.to_string());
        }
    }
    json!({ "isRepo": true, "current": current, "staged": staged, "unstaged": unstaged, "untracked": untracked })
}

fn log(ws: &Path, limit: u64) -> Result<Value, String> {
    if !is_repo(ws) {
        return Ok(json!([]));
    }
    match git(ws, &["log", &format!("--max-count={limit}"), "--format=%H%x00%aI%x00%aN%x00%s"]) {
        Ok(out) => Ok(out
            .lines()
            .filter_map(|l| {
                let mut f = l.split('\0');
                let (h, d, a, m) = (f.next()?, f.next()?, f.next()?, f.next()?);
                Some(json!({ "hash": &h[..7.min(h.len())], "date": d, "message": m, "author": a }))
            })
            .collect()),
        Err(e) if e.contains("does not have any commits yet") || e.contains("bad default revision") => Ok(json!([])),
        Err(e) => Err(format!("git:log failed: {e}")),
    }
}

fn graph(ws: &Path, limit: u64) -> Value {
    if !is_repo(ws) {
        return json!([]);
    }
    let Ok(raw) = git(ws, &["log", "--date=short", "--format=%h%x09%p%x09%an%x09%ad%x09%d%x09%s", "-n", &limit.to_string()]) else { return json!([]) };
    raw.trim()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let p: Vec<&str> = line.split('\t').collect();
            let get = |i: usize| p.get(i).copied().unwrap_or("");
            let raw_refs = get(4).trim();
            let raw_refs = raw_refs.strip_prefix('(').unwrap_or(raw_refs);
            let raw_refs = raw_refs.strip_suffix(')').unwrap_or(raw_refs);
            json!({
                "hash": get(0),
                "parents": get(1).split_whitespace().collect::<Vec<_>>(),
                "author": get(2),
                "date": get(3),
                "refs": raw_refs.split(',').map(str::trim).filter(|r| !r.is_empty()).collect::<Vec<_>>(),
                "message": p.get(5..).map(|s| s.join("\t")).unwrap_or_default(),
            })
        })
        .collect()
}

fn branches(ws: &Path) -> Result<Value, String> {
    let fail = |e: String| format!("git:branches failed: {e}");
    let mut all: Vec<String> = git(ws, &["for-each-ref", "--format=%(refname:short)", "refs/heads"]).map_err(fail)?.lines().map(str::to_string).collect();
    let current = match git(ws, &["symbolic-ref", "--short", "-q", "HEAD"]) {
        Ok(b) => b.trim().to_string(),
        Err(_) => {
            let short = git(ws, &["rev-parse", "--short", "HEAD"]).unwrap_or_default().trim().to_string();
            if !short.is_empty() {
                all.push(short.clone());
            }
            short
        }
    };
    Ok(json!({ "current": if all.is_empty() { String::new() } else { current }, "all": all }))
}

fn commit_details(ws: &Path, hash: &str) -> Result<Value, String> {
    let raw = git(ws, &["show", "--name-status", "--format=%H%x09%P%x09%an%x09%ad%x09%s", "-n", "1", hash]).map_err(|e| format!("git:commitDetails failed: {e}"))?;
    let raw = raw.trim();
    let mut lines = raw.lines();
    let head: Vec<&str> = lines.next().unwrap_or("").split('\t').collect();
    let full = head.first().copied().filter(|s| !s.is_empty()).unwrap_or(hash);
    let files: Vec<Value> = lines
        .filter_map(|line| {
            let line = line.trim();
            let p: Vec<&str> = line.split('\t').collect();
            if line.is_empty() || p.len() < 2 {
                return None;
            }
            let status = p[0].trim().chars().next().map(String::from).unwrap_or_else(|| "M".into());
            Some(json!({ "path": p.get(2).copied().unwrap_or(p[1]).replace('\\', "/"), "status": status }))
        })
        .collect();
    Ok(json!({
        "hash": &full[..7.min(full.len())],
        "fullHash": full,
        "parents": head.get(1).unwrap_or(&"").split_whitespace().collect::<Vec<_>>(),
        "author": head.get(2).unwrap_or(&""),
        "date": head.get(3).unwrap_or(&""),
        "message": head.get(4..).map(|s| s.join("\t")).unwrap_or_default(),
        "files": files,
    }))
}

fn commit_file_diff(ws_s: &str, ws: &Path, hash: &str, file: &str, parent: Option<&str>) -> Value {
    let rel = rel_path(ws_s, file);
    let parent: Option<String> = parent.filter(|p| !p.is_empty()).map(str::to_string).or_else(|| {
        git(ws, &["rev-parse", &format!("{hash}^@")]).ok().and_then(|o| o.split_whitespace().next().map(str::to_string))
    });
    let show = |r: &str| git(ws, &["show", &format!("{r}:{rel}")]).unwrap_or_default();
    let original = parent.map(|p| show(&p)).unwrap_or_default();
    let modified = show(hash);
    if looks_binary(&original) || looks_binary(&modified) {
        let note = "[Binary file content cannot be displayed in text diff]";
        return json!({ "original": note, "modified": note });
    }
    json!({ "original": original, "modified": modified })
}

fn clone(url: &str, target: &str) -> Value {
    let (url, target) = (url.trim(), target.trim());
    if url.is_empty() {
        return json!({ "success": false, "error": "Repository URL is required" });
    }
    if target.is_empty() {
        return json!({ "success": false, "error": "Target directory is required" });
    }
    // ext:: runs an arbitrary command, and a leading dash would be read as an option
    if url.starts_with('-') || url.to_lowercase().starts_with("ext::") {
        return json!({ "success": false, "error": "Unsupported repository URL" });
    }
    let resolved = resolve(target);
    match git(Path::new(&crate::util::p(&crate::util::home())), &["clone", "--", url, &resolved]) {
        Ok(_) => json!({ "success": true, "targetDir": resolved }),
        Err(e) => json!({ "success": false, "error": e }),
    }
}

pub fn call(core: &Arc<Core>, window: u32, channel: &str, args: &[Value]) -> Result<Value, String> {
    let ws_s = core.workspace_for(Some(window));
    let ws = Path::new(&ws_s);
    let limit = |d: u64| args.first().and_then(Value::as_u64).filter(|n| *n > 0).unwrap_or(d);
    let wrap = |name: &str, r: Result<String, String>| r.map(|_| Value::Null).map_err(|e| format!("git:{name} failed: {e}"));
    let path_arg = rel_path(&ws_s, arg_str(args, 0));
    match channel {
        "git:status" => Ok(status(ws)),
        "git:log" => log(ws, limit(50)),
        "git:graph" => Ok(graph(ws, limit(60))),
        "git:diff" => {
            let file = arg_str(args, 0);
            let abs = if Path::new(file).is_absolute() { file.to_string() } else { crate::util::p(&ws.join(file)) };
            let head = git(ws, &["show", &format!("HEAD:{path_arg}")]).unwrap_or_default();
            let work = fs::read(&abs).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
            Ok(json!({ "head": head, "work": work }))
        }
        "git:stage" => wrap("stage", git(ws, &["add", "--", &path_arg])),
        "git:unstage" => wrap("unstage", git(ws, &["reset", "--", &path_arg])),
        "git:restore" => wrap("restore", git(ws, &["checkout", "--", &path_arg])),
        "git:commit" => {
            let m = arg_str(args, 0);
            if m.trim().is_empty() {
                return Err("git:commit failed: Commit message cannot be empty".into());
            }
            wrap("commit", git(ws, &["commit", "-m", m]))
        }
        "git:branches" => branches(ws),
        "git:checkout" => wrap("checkout", git(ws, &["checkout", arg_str(args, 0)])),
        "git:commitDetails" => commit_details(ws, arg_str(args, 0)),
        "git:commitFileDiff" => Ok(commit_file_diff(&ws_s, ws, arg_str(args, 0), arg_str(args, 1), args.get(2).and_then(Value::as_str))),
        "git:clone" => Ok(clone(arg_str(args, 0), arg_str(args, 1))),
        _ => Err(format!("No handler registered for '{channel}'")),
    }
}
