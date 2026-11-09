// Background tasks each agent started, read from its log.
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::session_scan::{find_codex_rollouts, normalize_for_claude};
use crate::usage::AGY_DIRS;
use crate::util::{first_line, home, js_prefix, mtime_ms, now_ms, parse_json, read_text, sv};

fn desc(s: &str) -> String {
    js_prefix(s.split('\n').next().unwrap_or(""), 120)
}

fn json_lines<'a>(text: &'a str, must: &'a [&'a str]) -> impl Iterator<Item = Value> + 'a {
    text.split('\n').filter(move |l| must.iter().any(|k| l.contains(k))).filter_map(parse_json)
}

/// A tool result is a string or [{type, text}]
fn first_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a.iter().find_map(|x| x.get("text").and_then(Value::as_str)).unwrap_or("").to_string(),
        _ => String::new(),
    }
}

macro_rules! re {
    ($name:ident, $pat:expr) => {
        fn $name() -> &'static Regex {
            static R: OnceLock<Regex> = OnceLock::new();
            R.get_or_init(|| Regex::new($pat).unwrap())
        }
    };
}

re!(claude_notify, r"<tool-use-id>([A-Za-z0-9_]+)</tool-use-id>[\s\S]*?<status>([A-Za-z0-9_]+)</status>(?:[\s\S]*?<summary>([\s\S]*?)</summary>)?");
re!(bash_launched, r"^\s*Command running in background with ID");
re!(agent_launched, r"^\s*Async agent launched");
re!(codex_cmd, r#"exec_command\(\s*\{\s*cmd\s*:\s*("(?:[^"\\]|\\.)*")"#);
re!(script_failed, r"^Script (failed|error)");
re!(ag_launched, r"^Created At: [^\n]*\nTool is running as a background task with task id: (\S+)\nTask Description: ([^\n]*)");
re!(ag_done, r#"Task id "([^"]+)" (finished|was canceled) with result:\s*(?:The command exited with code (-?\d+))?"#);

fn task(agent: &str, id: &str, d: &str, started: &str) -> Map<String, Value> {
    let v = json!({ "agent": agent, "id": id, "desc": d, "startedAt": started, "status": "running" });
    v.as_object().unwrap().clone()
}

/// Claude: a tool_result that starts with the launch line, and a <task-notification> with the same tool-use-id at the end.
/// Only the start counts, and it must match the tool: command output that merely prints that sentence is not a launch.
pub fn claude_bg_tasks(text: &str) -> Vec<Value> {
    let mut uses: HashMap<String, (String, String, String)> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut tasks: HashMap<String, Map<String, Value>> = HashMap::new();
    for o in json_lines(text, &["tool_", "task-notification"]) {
        let ts = sv(&o, "timestamp").to_string();
        if let Some(content) = o.pointer("/message/content").and_then(Value::as_array) {
            for c in content {
                match c.get("type").and_then(Value::as_str) {
                    Some("tool_use") => {
                        let input = c.get("input").cloned().unwrap_or(Value::Null);
                        let name = sv(c, "name");
                        let d = [sv(&input, "description"), sv(&input, "command"), name].into_iter().find(|s| !s.is_empty()).unwrap_or("");
                        uses.insert(sv(c, "id").to_string(), (name.to_string(), desc(d), ts.clone()));
                    }
                    Some("tool_result") => {
                        let id = sv(c, "tool_use_id");
                        let u = uses.get(id);
                        let body = first_text(c.get("content"));
                        let launched = if u.map(|u| u.0.as_str()) == Some("Bash") { bash_launched().is_match(&body) } else { agent_launched().is_match(&body) };
                        if launched {
                            let d = u.map(|u| u.1.as_str()).filter(|s| !s.is_empty()).unwrap_or(id).to_string();
                            let started = u.map(|u| u.2.as_str()).filter(|s| !s.is_empty()).unwrap_or(&ts).to_string();
                            if !tasks.contains_key(id) {
                                order.push(id.to_string());
                            }
                            tasks.insert(id.to_string(), task("claude", id, &d, &started));
                        }
                    }
                    _ => {}
                }
            }
        }
        // The notice can sit in a user string or a queue-operation field, so scan the whole record
        let whole = o.to_string();
        for m in claude_notify().captures_iter(&whole) {
            if let Some(t) = tasks.get_mut(&m[1]) {
                t.insert("status".into(), json!(&m[2]));
                match m.get(3) {
                    Some(s) => t.insert("summary".into(), json!(s.as_str().replace("\\\"", "\""))),
                    None => t.remove("summary"),
                };
                if o.get("timestamp").is_some() {
                    t.insert("endedAt".into(), o["timestamp"].clone());
                }
            }
        }
    }
    order.into_iter().filter_map(|id| tasks.remove(&id)).map(Value::Object).collect()
}

/// Codex: an exec call has returned but its exec_command has not ended, so it runs in the background.
/// Codex only logs a process when it ends, so calls and completions have to be paired.
pub fn codex_bg_tasks(text: &str) -> Vec<Value> {
    struct Call {
        cmds: Vec<String>,
        started: String,
    }
    let mut pending: Vec<(String, Call)> = Vec::new();
    let mut tasks: Vec<(String, Map<String, Value>)> = Vec::new();
    for o in json_lines(text, &["custom_tool_call", "CommandExecution"]) {
        let empty = Value::Null;
        let pl = o.get("payload").unwrap_or(&empty);
        let ty = sv(pl, "type");
        if ty == "custom_tool_call" && sv(pl, "name") == "exec" {
            let cmds: Vec<String> = codex_cmd()
                .captures_iter(sv(pl, "input"))
                .filter_map(|m| serde_json::from_str::<String>(&m[1]).ok()) // JS-only escapes do not decode: skip that one
                .collect();
            if !cmds.is_empty() {
                pending.retain(|(id, _)| id != sv(pl, "call_id"));
                pending.push((sv(pl, "call_id").to_string(), Call { cmds, started: sv(&o, "timestamp").to_string() }));
            }
        } else if ty == "custom_tool_call_output" {
            let call_id = sv(pl, "call_id");
            let call = pending.iter().position(|(id, _)| id == call_id).map(|i| pending.remove(i).1);
            // The script itself failed, so the command never really ran
            if script_failed().is_match(&first_text(pl.get("output"))) {
                continue;
            }
            if let Some(call) = call {
                for (i, cmd) in call.cmds.iter().enumerate() {
                    tasks.push((cmd.clone(), task("codex", &format!("{call_id}-{i}"), &desc(cmd), &call.started)));
                }
            }
        } else if ty == "item_completed" && pl.pointer("/item/type").and_then(Value::as_str) == Some("CommandExecution") {
            let item = &pl["item"];
            let cmd = item.get("command").and_then(Value::as_array).and_then(|a| a.last()).and_then(Value::as_str).unwrap_or("");
            // A foreground command first; only otherwise does this end a background task
            if let Some((_, call)) = pending.iter_mut().find(|(_, c)| c.cmds.iter().any(|x| x == cmd)) {
                let i = call.cmds.iter().position(|x| x == cmd).unwrap();
                call.cmds.remove(i);
                continue;
            }
            if let Some((_, t)) = tasks.iter_mut().find(|(c, t)| c == cmd && t.get("status").and_then(Value::as_str) == Some("running")) {
                let code = item.get("exit_code").filter(|c| !c.is_null());
                let clean = code.map(|c| c.as_i64() == Some(0)).unwrap_or(true);
                t.insert("status".into(), json!(if sv(item, "status") == "completed" && clean { "completed" } else { "failed" }));
                let shown = code.map(|c| c.to_string()).unwrap_or_else(|| "?".into());
                t.insert("summary".into(), json!(format!("exit code {shown}")));
                if o.get("timestamp").is_some() {
                    t.insert("endedAt".into(), o["timestamp"].clone());
                }
            }
        }
    }
    tasks.into_iter().map(|(_, t)| Value::Object(t)).collect()
}

/// Antigravity: launch is a GENERIC step with a fixed opening; the end is a SYSTEM_MESSAGE
pub fn antigravity_bg_tasks(text: &str) -> Vec<Value> {
    let mut order = Vec::new();
    let mut tasks: HashMap<String, Map<String, Value>> = HashMap::new();
    for o in json_lines(text, &["background task", "with result"]) {
        let content = sv(&o, "content");
        let ty = sv(&o, "type");
        if ty == "GENERIC" {
            if let Some(m) = ag_launched().captures(content) {
                // "Timer: 2s, ..." is the agent's own wait, not work, and it never gets an end message
                if m[2].starts_with("Timer:") {
                    continue;
                }
                if !tasks.contains_key(&m[1]) {
                    order.push(m[1].to_string());
                }
                tasks.insert(m[1].to_string(), task("antigravity", &m[1], &desc(&m[2]), sv(&o, "created_at")));
                continue;
            }
        }
        if ty == "SYSTEM_MESSAGE" {
            if let Some(m) = ag_done().captures(content) {
                if let Some(t) = tasks.get_mut(&m[1]) {
                    let code = m.get(3).map(|c| c.as_str());
                    let status = if &m[2] != "finished" {
                        "killed"
                    } else if code.is_none() || code == Some("0") {
                        "completed"
                    } else {
                        "failed"
                    };
                    t.insert("status".into(), json!(status));
                    if let Some(c) = code {
                        t.insert("summary".into(), json!(format!("exit code {c}")));
                    }
                    if o.get("created_at").is_some() {
                        t.insert("endedAt".into(), o["created_at"].clone());
                    }
                }
            }
        }
    }
    order.into_iter().filter_map(|id| tasks.remove(&id)).map(Value::Object).collect()
}

/// Slashes unified, lowercase, no trailing slash; a JSON-escaped \\\\ collapses too
fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut sep = false;
    for c in s.chars() {
        if c == '\\' || c == '/' {
            if !sep {
                out.push('/');
            }
            sep = true;
        } else {
            sep = false;
            out.push(c);
        }
    }
    let lower = out.to_lowercase();
    lower.strip_suffix('/').map(str::to_string).unwrap_or(lower)
}

fn list(dir: &Path, want: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    fs::read_dir(dir).map(|r| r.flatten().filter(|e| want(&e.file_name().to_string_lossy())).map(|e| e.path()).collect()).unwrap_or_default()
}

type Belongs = Box<dyn Fn(&str) -> bool>;

#[derive(Default)]
pub struct BgCache(Mutex<HashMap<String, (f64, Vec<Value>)>>);

impl BgCache {
    /// Background tasks in this workspace's sessions written to in the last 12 hours, newest first.
    // ponytail: a task whose session was closed never gets an end notice and shows running for up to 12 hours; compare live session ids if that matters
    pub fn scan(&self, workspace: &str) -> Vec<Value> {
        let root = norm(workspace);
        if root.is_empty() {
            return Vec::new();
        }
        let since = now_ms() - 12.0 * 3_600_000.0;
        let h = home();
        let mut sources: Vec<(Vec<PathBuf>, fn(&str) -> Vec<Value>, Belongs)> = Vec::new();

        // Claude: the project folder itself says which workspace
        let slug = normalize_for_claude(workspace.trim_end_matches(['\\', '/'])).to_lowercase();
        if let Some(dir) = list(&h.join(".claude").join("projects"), |n| n.to_lowercase() == slug).into_iter().next() {
            sources.push((list(&dir, |n| n.ends_with(".jsonl")), claude_bg_tasks, Box::new(|_| true)));
        }

        // Codex: session_meta's cwd on the first line
        let root2 = root.clone();
        sources.push((
            find_codex_rollouts(30).into_iter().filter(|f| f.2 >= since).map(|f| f.1).collect(),
            codex_bg_tasks,
            Box::new(move |text| {
                let Some(line) = text.split('\n').next() else { return false };
                let Some(v) = parse_json(first_line(line)) else { return false };
                let cwd = norm(v.pointer("/payload/cwd").and_then(Value::as_str).unwrap_or(""));
                cwd == root2 || cwd.starts_with(&format!("{root2}/"))
            }),
        ));

        // Antigravity has no cwd field: the path appearing in tool arguments decides
        let root3 = root.clone();
        let agy: Vec<PathBuf> = AGY_DIRS
            .iter()
            .flat_map(|d| list(&h.join(".gemini").join(d).join("brain"), |n| !n.starts_with('.')))
            .map(|d| d.join(".system_generated").join("logs").join("transcript.jsonl"))
            .collect();
        sources.push((agy, antigravity_bg_tasks, Box::new(move |text| norm(text).contains(&root3))));

        let mut out = Vec::new();
        for (files, parse, belongs) in sources {
            for path in files {
                let Some(mtime) = mtime_ms(&path) else { continue };
                if mtime < since {
                    continue;
                }
                let key = format!("{root}|{}", path.display());
                let mut cache = self.0.lock().unwrap();
                match cache.get(&key) {
                    Some((m, tasks)) if *m == mtime => out.extend(tasks.iter().cloned()),
                    _ => {
                        let Some(text) = read_text(&path) else { continue };
                        let tasks = if belongs(&text) { parse(&text) } else { Vec::new() };
                        out.extend(tasks.iter().cloned());
                        cache.insert(key, (mtime, tasks));
                    }
                }
            }
        }
        out.sort_by(|a, b| sv(b, "startedAt").cmp(sv(a, "startedAt")));
        out
    }
}
