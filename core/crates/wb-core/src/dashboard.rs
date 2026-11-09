// The dashboard: scanned sessions, joined with live terminals and the user's archive and delete marks.
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::core::{arg_str, Core};
use crate::session_scan::{antigravity_workspace, claude_workspace, is_same_workspace, normalize_path};
use crate::usage::AGY_DIRS;
use crate::util::{basename, date_ms, exists, home, iso, now_ms, read_text, resolve, sv};

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DashboardState {
    pub archived_ids: Vec<String>,
    pub deleted_ids: Vec<String>,
    pub archived_workspaces: Vec<String>,
    pub deleted_workspaces: Vec<String>,
}

fn opt<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn last_name(path: &str) -> String {
    path.split(['\\', '/']).filter(|s| !s.is_empty()).last().unwrap_or(path).trim().to_string()
}

/// Match by full path, path tail or folder name: session logs often only carry part of it
fn target_matcher(ws_path: &str) -> impl Fn(Option<&str>, Option<&str>) -> bool + '_ {
    let norm = normalize_path(ws_path);
    let ws_name = last_name(ws_path);
    move |path: Option<&str>, name: Option<&str>| {
        if path.is_none() && name.is_none() {
            return false;
        }
        if is_same_workspace(path, name, Some(ws_path), Some(&ws_name)) {
            return true;
        }
        if let Some(pp) = path {
            let nw = normalize_path(pp);
            if nw == norm || nw.ends_with(&format!("/{norm}")) || norm.ends_with(&format!("/{nw}")) {
                return true;
            }
        }
        let n = name.filter(|n| !n.is_empty()).map(str::to_string).or_else(|| path.map(basename)).unwrap_or_default().trim().to_lowercase();
        !n.is_empty() && (n == ws_name.to_lowercase() || n == norm)
    }
}

impl Core {
    fn state_path(&self) -> std::path::PathBuf {
        self.data_dir.join("dashboard-state.json")
    }

    pub fn load_dashboard_state(&self) -> DashboardState {
        read_text(&self.state_path()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn save_dashboard_state(&self, s: &DashboardState) {
        let _ = fs::create_dir_all(&self.data_dir);
        let _ = fs::write(self.state_path(), serde_json::to_string_pretty(s).unwrap());
    }

    pub fn invalidate_dashboard_cache(&self) {
        *self.cached_sessions.lock().unwrap() = None;
        self.scanner.invalidate();
    }

    fn known_workspaces(&self) -> Vec<String> {
        let mut v = self.get_recent_workspaces();
        v.extend(self.project_workspaces().into_iter().map(|(_, ws)| ws));
        v
    }

    /// Workspaces found in past sessions, for the jump list and recent-project suggestions
    pub fn recent_workspaces_from_dashboard(&self) -> Vec<String> {
        let mut results: Vec<String> = Vec::new();
        let add = |r: &mut Vec<String>, path: String| {
            if !r.contains(&path) {
                r.push(path);
            }
        };
        for (_, s) in self.scanner.cached_entries() {
            if let Some(path) = opt(&s, "workspacePath") {
                if !self.is_protected_path(path) && fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false) {
                    add(&mut results, resolve(path));
                }
            }
            if results.len() >= 10 {
                break;
            }
        }
        if let Ok(rd) = fs::read_dir(home().join(".claude").join("projects")) {
            let mut dirs: Vec<(String, f64)> = rd
                .flatten()
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| (e.file_name().to_string_lossy().into_owned(), crate::util::mtime_ms(&e.path()).unwrap_or(0.0)))
                .filter(|(n, _)| {
                    let b = n.as_bytes();
                    b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b'-' && b[2] == b'-'
                })
                .collect();
            dirs.sort_by(|a, b| b.1.total_cmp(&a.1));
            for (name, _) in dirs {
                if let Some(path) = claude_workspace(&name, None).1 {
                    if !self.is_protected_path(&path) {
                        add(&mut results, resolve(&path));
                    }
                }
                if results.len() >= 15 {
                    break;
                }
            }
        }
        results
    }

    /// Opening a workspace again lifts its archive and delete marks, and those of its sessions
    pub fn unmark_workspace(&self, dir: &str) {
        if dir.is_empty() {
            return;
        }
        let norm = normalize_path(dir);
        let ws_name = last_name(dir).to_lowercase();
        let mut state = self.load_dashboard_state();
        let mut changed = false;
        let is_match = |target: Option<&str>| -> bool {
            let Some(t) = target.filter(|t| !t.is_empty()) else { return false };
            let nt = normalize_path(t);
            if nt == norm || nt.ends_with(&format!("/{norm}")) || norm.ends_with(&format!("/{nt}")) {
                return true;
            }
            let base = last_name(t).to_lowercase();
            !base.is_empty() && !ws_name.is_empty() && (base == ws_name || nt == ws_name)
        };
        let before = (state.deleted_workspaces.len(), state.archived_workspaces.len());
        state.deleted_workspaces.retain(|w| !is_match(Some(w)));
        state.archived_workspaces.retain(|w| !is_match(Some(w)));
        changed |= before != (state.deleted_workspaces.len(), state.archived_workspaces.len());

        let mut ids: HashSet<String> = HashSet::new();
        let same = |path: Option<&str>, name: Option<&str>| is_same_workspace(path, name, Some(dir), Some(&ws_name)) || is_match(path) || is_match(name);
        if let Some(sessions) = self.cached_sessions.lock().unwrap().as_ref() {
            for s in sessions {
                if same(opt(s, "workspacePath"), opt(s, "workspace")) {
                    ids.insert(sv(s, "id").to_string());
                }
            }
        }
        if let Ok(rd) = fs::read_dir(home().join(".claude").join("projects")) {
            for e in rd.flatten().filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false)) {
                let (name, path) = claude_workspace(&e.file_name().to_string_lossy(), None);
                if same(path.as_deref(), name.as_deref()) {
                    for f in fs::read_dir(e.path()).into_iter().flatten().flatten() {
                        let n = f.file_name().to_string_lossy().into_owned();
                        if let Some(id) = n.strip_suffix(".jsonl") {
                            ids.insert(id.to_string());
                        }
                    }
                }
            }
        }
        let known = self.known_workspaces();
        for base in AGY_DIRS {
            for e in fs::read_dir(home().join(".gemini").join(base).join("brain")).into_iter().flatten().flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                let log = e.path().join(".system_generated").join("logs").join("transcript.jsonl");
                if log.exists() {
                    let (n, pth) = antigravity_workspace(&log, &known);
                    if same(pth.as_deref(), n.as_deref()) {
                        ids.insert(name);
                    }
                }
            }
        }
        if !ids.is_empty() {
            let before = (state.deleted_ids.len(), state.archived_ids.len());
            state.deleted_ids.retain(|i| !ids.contains(i));
            state.archived_ids.retain(|i| !ids.contains(i));
            changed |= before != (state.deleted_ids.len(), state.archived_ids.len());
        }
        if changed {
            self.save_dashboard_state(&state);
        }
        self.invalidate_dashboard_cache();
        self.notify_jump_list();
    }

    fn dashboard_data(&self, force: bool) -> Value {
        if force {
            self.invalidate_dashboard_cache();
        }
        let scan = self.scanner.scan_dashboard(self.is_cli_enabled("claude"), self.is_cli_enabled("codex"), &self.known_workspaces());
        *self.cached_sessions.lock().unwrap() = self.scanner.cached_sessions();
        // Marks and live terminals are read after the scan: what the user did during it counts
        let state = self.load_dashboard_state();
        let deleted: HashSet<&String> = state.deleted_ids.iter().collect();
        let archived: HashSet<&String> = state.archived_ids.iter().collect();
        let mark = |list: &[Value]| -> Vec<Value> {
            list.iter()
                .filter(|s| !deleted.contains(&sv(s, "id").to_string()))
                .map(|s| {
                    let mut c = s.clone();
                    c["isArchived"] = json!(archived.contains(&sv(s, "id").to_string()));
                    c
                })
                .collect()
        };
        // Brain sessions have logs and token detail; CLI conversations only fill in what is missing
        let mut agy_order: Vec<String> = Vec::new();
        let mut agy_map: HashMap<String, Value> = HashMap::new();
        for s in scan["agyBrain"].as_array().unwrap() {
            agy_order.push(sv(s, "id").to_string());
            agy_map.insert(sv(s, "id").to_string(), s.clone());
        }
        for s in scan["agyCli"].as_array().unwrap() {
            let id = sv(s, "id").to_string();
            match agy_map.get_mut(&id) {
                Some(e) => {
                    if date_ms(sv(s, "lastActiveTime")) > date_ms(sv(e, "lastActiveTime")) {
                        e["lastActiveTime"] = s["lastActiveTime"].clone();
                        e["status"] = s["status"].clone();
                    }
                }
                None => {
                    agy_order.push(id.clone());
                    agy_map.insert(id, s.clone());
                }
            }
        }
        let agy_on = self.is_cli_enabled("antigravity");
        let mut agy: Vec<Value> = if agy_on { mark(&agy_order.iter().filter_map(|i| agy_map.get(i).cloned()).collect::<Vec<_>>()) } else { Vec::new() };
        let mut claude = mark(scan["claude"].as_array().unwrap());
        let mut codex = mark(scan["codex"].as_array().unwrap());

        // Tie live agent terminals to real sessions. Plain shells and disabled agents never become cards.
        let mut standalone: Vec<Value> = Vec::new();
        let mut matched: HashSet<String> = HashSet::new();
        let now = now_ms();
        let enabled = |a: &str| self.is_cli_enabled(a);
        for pt in self.active_session_metas() {
            let cmd = sv(&pt, "command").to_lowercase();
            let lid = sv(&pt, "launcherId").to_lowercase();
            let is_claude = enabled("claude") && (cmd.contains("claude") || lid.contains("claude"));
            let is_agy = enabled("antigravity") && (cmd.contains("agy") || cmd.contains("antigravity") || lid.contains("antigravity"));
            let is_codex = enabled("codex") && (cmd.contains("codex") || lid.contains("codex"));
            if !is_claude && !is_agy && !is_codex {
                continue;
            }
            let agent = if is_claude { "claude" } else if is_agy { "antigravity" } else { "codex" };
            let candidates: &mut Vec<Value> = if is_claude { &mut claude } else if is_agy { &mut agy } else { &mut codex };
            let target = sv(&pt, "cwd").to_string();
            let start_time = pt["startTime"].as_f64().unwrap_or(0.0);
            let args: Vec<&str> = pt["args"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            let mut session_id = opt(&pt, "sessionId").map(str::to_string);
            if session_id.is_none() {
                let after = |i: usize| args.get(i + 1).filter(|a| !a.starts_with('-')).map(|a| a.to_string());
                session_id = args.iter().position(|a| *a == "--resume" || *a == "-r" || *a == "--conversation").and_then(after).or_else(|| args.iter().position(|a| *a == "resume").and_then(after));
            }
            let same_ws = |s: &Value| is_same_workspace(opt(s, "workspacePath"), opt(s, "workspace"), Some(&target), None);
            let open = |s: &Value, m: &HashSet<String>| s["isArchived"] != json!(true) && !m.contains(sv(s, "id"));
            let newest = |v: Vec<usize>, c: &Vec<Value>| v.into_iter().max_by(|a, b| date_ms(sv(&c[*a], "lastActiveTime")).total_cmp(&date_ms(sv(&c[*b], "lastActiveTime"))));
            let mut hit: Option<usize> = session_id.as_ref().and_then(|sid| candidates.iter().position(|s| sv(s, "id") == sid && !matched.contains(sid)));
            // A pty opened from a card only answers to that session: it must not light up someone else's card
            if hit.is_none() && session_id.is_none() {
                hit = candidates.iter().position(|s| open(s, &matched) && same_ws(s) && date_ms(sv(s, "lastActiveTime")) >= start_time - 60_000.0);
                if hit.is_none() {
                    hit = newest((0..candidates.len()).filter(|i| open(&candidates[*i], &matched) && same_ws(&candidates[*i])).collect(), candidates);
                }
                // Moved folders inside the terminal: the agent's newest session in the last 30 minutes
                if hit.is_none() {
                    hit = newest((0..candidates.len()).filter(|i| open(&candidates[*i], &matched) && now - date_ms(sv(&candidates[*i], "lastActiveTime")) < 1_800_000.0).collect(), candidates);
                }
            }
            if let Some(i) = hit {
                matched.insert(sv(&candidates[i], "id").to_string());
                candidates[i]["status"] = json!("active");
                candidates[i]["lastActiveTime"] = json!(iso(now));
            } else if !deleted.contains(&sv(&pt, "id").to_string()) {
                let nt = normalize_path(&target);
                if !nt.is_empty() && state.deleted_workspaces.iter().any(|w| normalize_path(w) == nt) {
                    continue;
                }
                let is_arch = archived.contains(&sv(&pt, "id").to_string()) || (!nt.is_empty() && state.archived_workspaces.iter().any(|w| normalize_path(w) == nt));
                let (label, model) = match agent {
                    "claude" => ("Claude Code", "Claude 3.7 Sonnet"),
                    "antigravity" => ("Antigravity", "Gemini 3.8 Flash"),
                    _ => ("Codex", "Codex CLI"),
                };
                standalone.push(json!({
                    "id": sv(&pt, "id"), "agent": agent, "title": format!("{label} (Active Session)"), "status": "active",
                    "startTime": iso(start_time), "lastActiveTime": iso(now), "totalTokens": 0, "model": model, "isArchived": is_arch,
                    "workspace": if target.is_empty() { "Workspace".to_string() } else { basename(&target) }, "workspacePath": target,
                    "tokenBreakdown": { "promptTokens": 0, "toolReadTokens": 0, "completionTokens": 0, "details": [
                        { "category": "Context & System Prompt", "tokens": 0, "percentage": 0 },
                        { "category": "Tool Execution & Files", "tokens": 0, "percentage": 0 },
                        { "category": "Thinking & Generation", "tokens": 0, "percentage": 0 }
                    ] }
                }));
            }
        }

        // Workspaces the user really opened: recent, plus each open window's
        let mut user_paths: Vec<String> = Vec::new();
        for pth in self.known_workspaces() {
            if !pth.is_empty() && !self.is_protected_path(&pth) {
                let r = resolve(&pth);
                if !user_paths.contains(&r) {
                    user_paths.push(r);
                }
            }
        }
        let deleted_ws: HashSet<String> = state.deleted_workspaces.iter().map(|w| normalize_path(w)).collect();
        let archived_ws: HashSet<String> = state.archived_workspaces.iter().map(|w| normalize_path(w)).collect();
        let active_paths: Vec<String> = user_paths.into_iter().filter(|x| !deleted_ws.contains(&normalize_path(x)) && !archived_ws.contains(&normalize_path(x))).collect();
        let archived_paths: Vec<String> = state.archived_workspaces.iter().filter(|x| !deleted_ws.contains(&normalize_path(x))).cloned().collect();
        let allowed: Vec<String> = active_paths.iter().chain(archived_paths.iter()).cloned().collect();
        // "Current workspace" is decided per window in the page
        let info = |x: &String, a: bool| {
            let n = basename(x);
            json!({ "path": x, "name": if n.is_empty() { x.clone() } else { n }, "isArchived": a })
        };
        let mut user_workspaces: Vec<Value> = active_paths.iter().map(|x| info(x, false)).collect();
        user_workspaces.extend(archived_paths.iter().map(|x| info(x, true)));

        let standalone_ids: HashSet<String> = standalone.iter().map(|s| sv(s, "id").to_string()).collect();
        let keep = |list: Vec<Value>| -> Vec<Value> {
            let mut res = Vec::new();
            for s in list {
                let mut m: Option<(bool, Option<String>)> = None;
                if standalone_ids.contains(sv(&s, "id")) {
                    m = Some((false, None));
                }
                if m.is_none() {
                    for x in &allowed {
                        if is_same_workspace(opt(&s, "workspacePath"), opt(&s, "workspace"), Some(x), Some(&basename(x))) {
                            m = Some((archived_ws.contains(&normalize_path(x)) || archived.contains(&sv(&s, "id").to_string()), Some(x.clone())));
                            break;
                        }
                    }
                }
                if m.is_none() {
                    // A CLI run outside this app still shows, unless the user hid its workspace
                    let wp = opt(&s, "workspacePath");
                    if sv(&s, "status") == "active" || wp.map(|w| exists(w) && !self.is_protected_path(w)).unwrap_or(false) {
                        let nw = wp.map(normalize_path).unwrap_or_default();
                        if nw.is_empty() || !deleted_ws.contains(&nw) {
                            m = Some((archived.contains(&sv(&s, "id").to_string()) || (!nw.is_empty() && archived_ws.contains(&nw)), wp.map(str::to_string)));
                        }
                    }
                }
                if let Some((arch, matched_path)) = m {
                    let mut c = s.clone();
                    c["isArchived"] = json!(arch);
                    // A session's own real path is never overwritten, only filled in when missing
                    if opt(&c, "workspacePath").is_none() {
                        if let Some(mp) = &matched_path {
                            c["workspacePath"] = json!(mp);
                        }
                    }
                    if opt(&c, "workspace").is_none() {
                        if let Some(mp) = &matched_path {
                            c["workspace"] = json!(basename(mp));
                        }
                    }
                    res.push(c);
                }
            }
            res
        };
        let mut all: Vec<Value> = standalone.clone();
        all.extend(keep(agy));
        all.extend(keep(claude));
        all.extend(keep(codex));
        all.sort_by(|a, b| {
            let (x, y) = (sv(a, "status") == "active", sv(b, "status") == "active");
            y.cmp(&x).then_with(|| date_ms(sv(b, "lastActiveTime")).total_cmp(&date_ms(sv(a, "lastActiveTime"))))
        });
        let usage = &scan["usage"];
        let summary = |id: &str, label: &str, estimated: bool| {
            let list: Vec<&Value> = all.iter().filter(|s| sv(s, "agent") == id).collect();
            let u = &usage[id]["all"];
            let n = |k: &str| u[k].as_u64().unwrap_or(0);
            let mut o = json!({ "agent": id, "label": label });
            if estimated {
                o["estimated"] = json!(true);
            }
            o["totalSessions"] = json!(list.len());
            o["activeSessions"] = json!(list.iter().filter(|s| sv(s, "status") == "active").count());
            o["totalTokens"] = json!(n("input") + n("cacheRead") + n("output"));
            o["promptTokens"] = json!(n("input"));
            o["toolTokens"] = json!(n("cacheRead"));
            o["completionTokens"] = json!(n("output"));
            o
        };
        json!({
            "agents": { "claude": summary("claude", "Claude Code", false), "antigravity": summary("antigravity", "Antigravity", true), "codex": summary("codex", "Codex", false) },
            "usageByRange": usage,
            "sessions": all,
            "userWorkspaces": user_workspaces,
            "archivedWorkspaces": state.archived_workspaces,
            "deletedWorkspaces": state.deleted_workspaces,
        })
    }

    fn sessions_of(&self, is_target: &dyn Fn(Option<&str>, Option<&str>) -> bool) -> Vec<String> {
        self.cached_sessions.lock().unwrap().as_ref().map(|c| c.iter().filter(|s| is_target(opt(s, "workspacePath"), opt(s, "workspace"))).map(|s| sv(s, "id").to_string()).collect()).unwrap_or_default()
    }

    pub fn dashboard_call(self: &Arc<Self>, _window: u32, channel: &str, args: &[Value]) -> Result<Value, String> {
        let a0 = arg_str(args, 0);
        let flag = |i: usize| args.get(i).and_then(Value::as_bool).unwrap_or(false);
        let ids = |i: usize| -> Vec<String> { args.get(i).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect() };
        let add = |list: &mut Vec<String>, id: &str| {
            if !list.iter().any(|x| x == id) {
                list.push(id.to_string());
            }
        };
        match channel {
            "dashboard:bgTasks" => Ok(json!(self.bg.scan(a0))),
            // Just token totals, for the status bar above the Vibe terminal
            "dashboard:usage" => Ok(self.scanner.usage.scan_all()),
            "dashboard:data" => Ok(self.dashboard_data(flag(0))),
            "dashboard:archiveSession" | "dashboard:archiveSessions" => {
                let list = if channel.ends_with('s') { ids(0) } else { vec![a0.to_string()] };
                let mut st = self.load_dashboard_state();
                if flag(1) {
                    for id in &list {
                        add(&mut st.archived_ids, id);
                    }
                } else {
                    st.archived_ids.retain(|x| !list.contains(x));
                }
                self.save_dashboard_state(&st);
                Ok(json!(true))
            }
            "dashboard:deleteSession" | "dashboard:deleteSessions" => {
                let list = if channel.ends_with('s') { ids(0) } else { vec![a0.to_string()] };
                let mut st = self.load_dashboard_state();
                for id in &list {
                    add(&mut st.deleted_ids, id);
                }
                st.archived_ids.retain(|x| !list.contains(x));
                self.save_dashboard_state(&st);
                Ok(json!(true))
            }
            "dashboard:archiveWorkspace" => {
                if a0.is_empty() {
                    return Ok(json!(false));
                }
                let archive = flag(1);
                let session_ids = ids(2);
                let mut st = self.load_dashboard_state();
                let is_target = target_matcher(a0);
                let owned = self.sessions_of(&is_target);
                if archive {
                    if !st.archived_workspaces.iter().any(|w| is_target(Some(w), Some(&basename(w)))) {
                        st.archived_workspaces.push(a0.to_string());
                    }
                    st.deleted_workspaces.retain(|w| !is_target(Some(w), Some(&basename(w))));
                    for id in session_ids.iter().chain(owned.iter()) {
                        add(&mut st.archived_ids, id);
                    }
                } else {
                    st.archived_workspaces.retain(|w| !is_target(Some(w), Some(&basename(w))));
                    st.archived_ids.retain(|id| !session_ids.contains(id) && !owned.contains(id));
                }
                // Before reopening: adding it back reads this state and must see the new marks
                self.save_dashboard_state(&st);
                if !archive && exists(a0) {
                    self.add_recent_workspace(a0);
                }
                self.invalidate_dashboard_cache();
                self.notify_jump_list();
                Ok(json!(true))
            }
            "dashboard:deleteWorkspace" => {
                if a0.is_empty() {
                    return Ok(json!(false));
                }
                let session_ids = ids(1);
                let mut st = self.load_dashboard_state();
                let is_target = target_matcher(a0);
                st.archived_workspaces.retain(|w| !is_target(Some(w), Some(&basename(w))));
                if !st.deleted_workspaces.iter().any(|w| is_target(Some(w), Some(&basename(w)))) {
                    st.deleted_workspaces.push(a0.to_string());
                }
                let owned = self.sessions_of(&is_target);
                for id in session_ids.iter().chain(owned.iter()) {
                    add(&mut st.deleted_ids, id);
                }
                st.archived_ids.retain(|id| !session_ids.contains(id) && !owned.contains(id));
                if Path::new(a0).exists() {
                    self.remove_recent_workspace(a0);
                }
                self.invalidate_dashboard_cache();
                self.save_dashboard_state(&st);
                self.notify_jump_list();
                Ok(json!(true))
            }
            "dashboard:unmarkWorkspace" => {
                if a0.is_empty() {
                    return Ok(json!(false));
                }
                self.unmark_workspace(a0);
                Ok(json!(true))
            }
            _ => Err(format!("No handler registered for '{channel}'")),
        }
    }
}
