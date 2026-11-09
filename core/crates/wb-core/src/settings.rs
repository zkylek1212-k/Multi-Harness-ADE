// User settings. The workspace copy travels with the repo, so it is untrusted input:
// which programs to run and whether to skip agent approvals only ever come from the user's own file.
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Map, Value};
use wb_proc::runner::run_process;
use wb_proc::{CancelToken, RunSpec};

use crate::core::{arg_str, Core};
use crate::custom_agent::sanitize_custom_agent;
use crate::paths::find_cli;
use crate::util::{basename, exists, home, p, read_text, resolve};

fn read_json(path: &Path) -> Value {
    read_text(path).and_then(|t| serde_json::from_str(&t).ok()).filter(Value::is_object).unwrap_or(json!({}))
}

fn sanitize_path(v: Option<&Value>) -> String {
    let Some(s) = v.and_then(Value::as_str) else { return String::new() };
    let t = s.trim();
    if t.is_empty() {
        return String::new();
    }
    if t.contains(['/', '\\']) {
        let found = exists(t) || (cfg!(windows) && [".cmd", ".exe", ".bat", ".ps1"].iter().any(|e| exists(&format!("{t}{e}"))));
        if !found {
            return String::new(); // moved to another machine: back to auto-detect
        }
    }
    t.to_string()
}

fn sanitize_test_results(v: Option<&Value>) -> Value {
    let mut out = Map::new();
    if let Some(o) = v.and_then(Value::as_object) {
        for (k, r) in o {
            if !r.get("ok").map(truthy).unwrap_or(false) {
                continue;
            }
            if let Some(tp) = r.get("testedPath").and_then(Value::as_str) {
                if tp.contains(['/', '\\']) && sanitize_path(Some(&json!(tp))).is_empty() {
                    continue;
                }
            }
            out.insert(k.clone(), r.clone());
        }
    }
    Value::Object(out)
}

pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

fn sanitized_paths(known: &[&str], raw: Option<&Value>) -> Value {
    let mut out = Map::new();
    let o = raw.and_then(Value::as_object);
    for k in known {
        out.insert(k.to_string(), json!(sanitize_path(o.and_then(|o| o.get(*k)))));
    }
    if let Some(o) = o {
        for (k, v) in o {
            out.insert(k.clone(), json!(sanitize_path(Some(v))));
        }
    }
    Value::Object(out)
}

fn merge(a: Option<&Value>, b: Option<&Value>) -> Value {
    let mut out = a.and_then(Value::as_object).cloned().unwrap_or_default();
    if let Some(b) = b.and_then(Value::as_object) {
        for (k, v) in b {
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}

impl Core {
    /// A system folder or the install folder is never treated as a project
    pub fn is_protected_path(&self, target: &str) -> bool {
        if target.is_empty() {
            return true;
        }
        let norm = resolve(target).to_lowercase();
        let under = |root: &str| {
            let r = root.to_lowercase();
            norm == r || norm.starts_with(&format!("{r}\\"))
        };
        if cfg!(windows) {
            let var = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
            if under(&var("ProgramFiles", "C:\\Program Files")) || under(&var("ProgramFiles(x86)", "C:\\Program Files (x86)")) || under(&var("SystemRoot", "C:\\Windows")) {
                return true;
            }
        }
        if self.info.is_packaged {
            let dir = crate::util::dirname(&self.info.exe_path).to_lowercase();
            if norm == dir || norm.starts_with(&format!("{dir}\\")) || norm.starts_with(&format!("{dir}/")) {
                return true;
            }
        }
        false
    }

    pub fn global_settings_path(&self) -> PathBuf {
        self.data_dir.join("settings.json")
    }

    pub fn workspace_settings_path(&self) -> Option<PathBuf> {
        let root = self.workspace_root();
        if root.is_empty() || self.is_protected_path(&root) {
            return None;
        }
        Some(Path::new(&root).join(".workbench").join("settings.json"))
    }

    pub fn load_settings(&self) -> Value {
        let global = read_json(&self.global_settings_path());
        let ws = self.workspace_settings_path().map(|f| read_json(&f)).unwrap_or(json!({}));
        let mut parsed = merge(Some(&global), Some(&ws));
        for key in ["cliPaths", "docToolPaths", "cliTestResults", "docToolTestResults", "cliBypassPermissions"] {
            parsed[key] = global.get(key).cloned().unwrap_or(Value::Null);
        }
        let enabled = merge(global.get("cliEnabled"), ws.get("cliEnabled"));

        let mut cli_enabled = Map::new();
        for k in ["claude", "antigravity", "codex", "powershell", "cmd"] {
            cli_enabled.insert(k.to_string(), enabled.get(k).filter(|v| !v.is_null()).cloned().unwrap_or(json!(true)));
        }
        for (k, v) in enabled.as_object().unwrap() {
            cli_enabled.insert(k.clone(), v.clone());
        }

        let mut out = Map::new();
        // A repository cannot pick a program or extension paths on this machine
        out.insert("customAgent".into(), sanitize_custom_agent(global.get("customAgent")));
        out.insert("cliPaths".into(), sanitized_paths(&["claude", "antigravity", "codex", "powershell", "cmd"], parsed.get("cliPaths")));
        out.insert("cliEnabled".into(), Value::Object(cli_enabled));
        out.insert("cliTestResults".into(), sanitize_test_results(parsed.get("cliTestResults")));
        out.insert("docToolTestResults".into(), sanitize_test_results(parsed.get("docToolTestResults")));
        out.insert("cliBypassPermissions".into(), parsed.get("cliBypassPermissions").filter(|v| !v.is_null()).cloned().unwrap_or(json!(false)));
        out.insert("docToolPaths".into(), sanitized_paths(&["word", "excel", "powerpoint", "pdf"], parsed.get("docToolPaths")));
        out.insert("autoOpenAgentModifiedFiles".into(), parsed.get("autoOpenAgentModifiedFiles").filter(|v| !v.is_null()).cloned().unwrap_or(json!(true)));
        if let Some(l) = parsed.get("language").and_then(Value::as_str).filter(|l| *l == "en" || *l == "zh-TW") {
            out.insert("language".into(), json!(l));
        }
        if let Some(l) = parsed.get("lastWorkspace").and_then(Value::as_str) {
            out.insert("lastWorkspace".into(), json!(l));
        }
        if let Some(list) = parsed.get("recentWorkspaces").and_then(Value::as_array) {
            let kept: Vec<String> = list.iter().filter_map(Value::as_str).filter(|s| !s.trim().is_empty() && !self.is_protected_path(s)).map(resolve).collect();
            out.insert("recentWorkspaces".into(), json!(kept));
        }
        out.insert("autoCheckUpdates".into(), parsed.get("autoCheckUpdates").filter(|v| !v.is_null()).cloned().unwrap_or(json!(true)));
        out.insert("autoDownloadUpdates".into(), parsed.get("autoDownloadUpdates").filter(|v| !v.is_null()).cloned().unwrap_or(json!(false)));
        // Budget limits only come from the user's file: a repo cannot relax them
        out.insert("cowork".into(), crate::cowork_shared::sanitize_cowork_settings(global.get("cowork")));
        Value::Object(out)
    }

    pub fn save_settings(&self, s: &Value) -> Result<(), String> {
        let gp = self.global_settings_path();
        if let Some(d) = gp.parent() {
            fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        fs::write(&gp, serde_json::to_string_pretty(s).unwrap()).map_err(|e| e.to_string())?;
        if let Some(wp) = self.workspace_settings_path() {
            let mut copy = s.clone();
            if let Some(o) = copy.as_object_mut() {
                o.shift_remove("customAgent");
            }
            // Failing to sync into the project is not fatal: the user's own file is already written
            if let Some(d) = wp.parent() {
                let _ = fs::create_dir_all(d);
            }
            let _ = fs::write(&wp, serde_json::to_string_pretty(&copy).unwrap());
        }
        Ok(())
    }

    pub fn get_last_workspace(&self) -> Option<String> {
        let s = self.load_settings();
        let last = s.get("lastWorkspace").and_then(Value::as_str)?;
        (exists(last) && !self.is_protected_path(last)).then(|| last.to_string())
    }

    pub fn save_last_workspace(&self, dir: &str) {
        if dir.is_empty() || self.is_protected_path(dir) || !exists(dir) {
            return;
        }
        let mut s = self.load_settings();
        s["lastWorkspace"] = json!(dir);
        let _ = self.save_settings(&s);
    }

    pub fn notify_jump_list(&self) {
        let s = self.load_settings();
        self.host.update_jump_list(&self.get_recent_workspaces(), s.get("language").and_then(Value::as_str));
    }

    pub fn get_recent_workspaces(&self) -> Vec<String> {
        let s = self.load_settings();
        let mut results: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let is_dir = |d: &str| fs::metadata(d).map(|m| m.is_dir()).unwrap_or(false);
        for dir in s.get("recentWorkspaces").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
            let norm = resolve(dir);
            if !seen.contains(&norm.to_lowercase()) && !self.is_protected_path(&norm) && is_dir(&norm) {
                seen.insert(norm.to_lowercase());
                results.push(norm);
            }
        }
        if let Some(last) = s.get("lastWorkspace").and_then(Value::as_str) {
            let norm = resolve(last);
            if !self.is_protected_path(last) && !seen.contains(&norm.to_lowercase()) && is_dir(&norm) {
                results.push(norm);
            }
        }
        results
    }

    /// To the top of the recent list, at most 20. Opening a workspace again also lifts any archive or delete mark.
    pub fn add_recent_workspace(self: &Arc<Self>, dir: &str) {
        if dir.is_empty() || self.is_protected_path(dir) || !fs::metadata(dir).map(|m| m.is_dir()).unwrap_or(false) {
            return;
        }
        let norm = resolve(dir);
        let mut s = self.load_settings();
        let existing = s.get("recentWorkspaces").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut updated = vec![norm.clone()];
        for e in existing.iter().filter_map(Value::as_str) {
            if resolve(e).to_lowercase() != norm.to_lowercase() && exists(e) && !self.is_protected_path(e) && updated.len() < 20 {
                updated.push(e.to_string());
            }
        }
        s["recentWorkspaces"] = json!(updated);
        s["lastWorkspace"] = json!(norm);
        let _ = self.save_settings(&s);
        self.unmark_workspace(&norm);
        self.notify_jump_list();
    }

    pub fn remove_recent_workspace(&self, dir: &str) -> Vec<String> {
        let norm = resolve(dir).to_lowercase();
        let mut s = self.load_settings();
        let updated: Vec<String> = s.get("recentWorkspaces").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).filter(|e| resolve(e).to_lowercase() != norm).map(str::to_string).collect();
        s["recentWorkspaces"] = json!(updated);
        let _ = self.save_settings(&s);
        self.notify_jump_list();
        updated
    }

    pub fn custom_doc_tool(&self, tool: &str) -> Option<String> {
        let s = self.load_settings();
        s.pointer(&format!("/docToolPaths/{tool}")).and_then(Value::as_str).map(str::trim).filter(|c| !c.is_empty() && exists(c)).map(str::to_string)
    }

    pub fn custom_cli_path(&self, id: &str) -> Option<String> {
        let s = self.load_settings();
        let custom = s.get("cliPaths").and_then(|c| c.get(id)).and_then(Value::as_str)?.trim().to_string();
        if custom.is_empty() {
            return None;
        }
        if exists(&custom) {
            return Some(custom);
        }
        if cfg!(windows) {
            for e in [".cmd", ".exe", ".bat", ".ps1"] {
                let c = format!("{custom}{e}");
                if exists(&c) {
                    return Some(c);
                }
            }
        }
        (!custom.contains(['/', '\\'])).then_some(custom)
    }

    pub fn is_cli_enabled(&self, id: &str) -> bool {
        self.load_settings().get("cliEnabled").and_then(|c| c.get(id)).and_then(Value::as_bool) != Some(false)
    }

    pub fn is_cli_bypass(&self) -> bool {
        self.load_settings().get("cliBypassPermissions").map(truthy).unwrap_or(false)
    }

    /// From the phone's settings page. Only affects agents started afterwards.
    pub fn set_cli_bypass(&self, enabled: bool) {
        let mut s = self.load_settings();
        s["cliBypassPermissions"] = json!(enabled);
        let _ = self.save_settings(&s);
        self.host.send_all("settings:changed", json!([]));
    }

    fn settings_set(self: &Arc<Self>, patch: &Value) -> Result<Value, String> {
        let current = self.load_settings();
        let agent = sanitize_custom_agent(patch.get("customAgent").filter(|v| !v.is_null()).or(current.get("customAgent")));
        let cmd = agent["command"].as_str().unwrap_or("");
        if agent["enabled"] == json!(true) && cmd.is_empty() {
            return Err("Custom Agent: executable is required".into());
        }
        if cmd.contains('\0') || agent["args"].as_array().unwrap().iter().any(|a| a.as_str().unwrap_or("").contains('\0')) {
            return Err("Custom Agent: command and arguments cannot contain NUL".into());
        }
        for r in agent["resources"].as_object().unwrap().values() {
            let r = r.as_str().unwrap_or("");
            if !r.is_empty() && (!Path::new(r).is_absolute() || r.contains('\0')) {
                return Err("Custom Agent: extension paths must be absolute".into());
            }
        }
        let mut updated = merge(Some(&current), Some(patch));
        updated["customAgent"] = agent;
        for key in ["cliPaths", "cliEnabled", "docToolPaths"] {
            updated[key] = merge(current.get(key), patch.get(key));
        }
        self.save_settings(&updated)?;
        if patch.get("customAgent").is_some() {
            self.host.send_all("settings:changed", json!([]));
            crate::remote::on_settings_changed(self);
        }
        if patch.get("language").is_some() || patch.get("recentWorkspaces").is_some() {
            self.notify_jump_list();
        }
        Ok(updated)
    }

    pub fn settings_call(self: &Arc<Self>, channel: &str, args: &[Value]) -> Result<Value, String> {
        match channel {
            "settings:get" => Ok(self.load_settings()),
            "settings:set" => self.settings_set(args.first().unwrap_or(&Value::Null)),
            "settings:getRecentWorkspaces" => Ok(json!(self.get_recent_workspaces())),
            "settings:removeRecentWorkspace" => Ok(json!(self.remove_recent_workspace(arg_str(args, 0)))),
            "settings:clearRecentWorkspaces" => {
                let mut s = self.load_settings();
                s["recentWorkspaces"] = json!([]);
                let _ = self.save_settings(&s);
                self.notify_jump_list();
                Ok(json!(true))
            }
            "settings:testCliPath" => Ok(test_cli_path(arg_str(args, 0))),
            "settings:testDocToolPath" => Ok(test_doc_tool(arg_str(args, 0))),
            _ => Err(format!("No handler registered for '{channel}'")),
        }
    }
}

fn run_version(command: &str, args: &[&str]) -> Result<String, String> {
    let spec = RunSpec {
        command: command.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        cwd: p(&home()),
        stdin: String::new(),
        timeout_ms: 6000,
        max_bytes: 1 << 20,
        env: None,
    };
    let r = run_process(&spec, &CancelToken::default());
    if let Some(e) = r.spawn_error {
        return Err(e);
    }
    if r.timed_out {
        return Err(format!("{command} timed out"));
    }
    if r.code != Some(0) {
        return Err(format!("Command failed: {command}\n{}", r.stderr));
    }
    let out = if r.stdout.trim().is_empty() { r.stderr } else { r.stdout };
    Ok(out.trim().lines().next().unwrap_or("").to_string())
}

fn test_cli_path(raw: &str) -> Value {
    let clean = raw.trim();
    if clean.is_empty() {
        return json!({ "ok": false, "error": "Path cannot be empty" });
    }
    let done = |version: Result<String, String>, target: &str| match version {
        Ok(v) => json!({ "ok": true, "version": if v.is_empty() { "Ready".to_string() } else { v }, "resolvedPath": target }),
        Err(e) => json!({ "ok": false, "error": e }),
    };
    let mut target = clean.to_string();
    let lower = target.to_lowercase();
    // Desktop document tools open a window on --version, so just check the file
    let doc_gui = ["winword", "excel", "powerpnt", "acrobat", "acrord32", "msedge", "sumatrapdf", "wps"].iter().any(|k| lower.contains(k));
    if doc_gui && fs::metadata(&target).map(|m| m.is_file()).unwrap_or(false) {
        return json!({ "ok": true, "version": format!("Ready ({})", basename(&target)), "resolvedPath": target });
    }
    if cfg!(windows) {
        if lower == "powershell" || lower.ends_with("powershell.exe") {
            return done(run_version("powershell.exe", &["-NoProfile", "-Command", "Write-Output $PSVersionTable.PSVersion.ToString()"]), "powershell.exe");
        }
        if lower == "cmd" || lower.ends_with("cmd.exe") {
            return done(run_version("cmd.exe", &["/c", "ver"]), "cmd.exe");
        }
        if lower == "pwsh" || lower.ends_with("pwsh.exe") {
            return done(run_version("pwsh", &["--version"]), "pwsh");
        }
        if !target.contains(['/', '\\']) {
            if let Some(found) = find_cli(&target, true) {
                target = found;
            }
        }
        let t = target.to_lowercase();
        if ![".exe", ".cmd", ".bat", ".ps1"].iter().any(|e| t.ends_with(e)) {
            for e in [".cmd", ".exe", ".bat", ".ps1"] {
                if exists(&format!("{target}{e}")) {
                    target = format!("{target}{e}");
                    break;
                }
            }
        }
        if target.to_lowercase().ends_with(".ps1") {
            return done(run_version("powershell.exe", &["-ExecutionPolicy", "Bypass", "-File", &target, "--version"]), &target);
        }
        if target.to_lowercase().ends_with(".cmd") || target.to_lowercase().ends_with(".bat") {
            return done(run_version("cmd.exe", &["/c", &target, "--version"]), &target);
        }
    }
    done(run_version(&target, &["--version"]), &target)
}

fn test_doc_tool(raw: &str) -> Value {
    let clean = raw.trim();
    if clean.is_empty() {
        return json!({ "ok": true, "version": "Will use System Default Application" });
    }
    let mut target = clean.to_string();
    if cfg!(windows) {
        let lower = target.to_lowercase();
        if ![".exe", ".cmd", ".bat"].iter().any(|e| lower.ends_with(e)) {
            for e in [".exe", ".cmd", ".bat"] {
                if exists(&format!("{target}{e}")) {
                    target = format!("{target}{e}");
                    break;
                }
            }
        }
    }
    let Ok(m) = fs::metadata(&target) else { return json!({ "ok": false, "error": "File not found at specified path" }) };
    if !m.is_file() && !(cfg!(target_os = "macos") && target.ends_with(".app")) {
        return json!({ "ok": false, "error": "Specified path is a directory, not an executable application" });
    }
    json!({ "ok": true, "version": format!("Valid executable ({})", basename(&target)) })
}
