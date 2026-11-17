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

pub fn sanitize_proxy(raw: Option<&Value>) -> Result<Value, String> {
    let raw = raw.unwrap_or(&Value::Null);
    let mode = raw.get("mode").and_then(Value::as_str).unwrap_or("inherit");
    if !["inherit", "custom", "direct"].contains(&mode) {
        return Err("Invalid proxy mode".into());
    }
    let clean = |key: &str| -> Result<String, String> {
        let text = raw.get(key).and_then(Value::as_str).unwrap_or("").trim();
        if text.len() > 8192 || text.chars().any(char::is_control) {
            return Err("Invalid proxy setting".into());
        }
        Ok(text.to_string())
    };
    let address = clean("url")?;
    let address = if address.is_empty() {
        String::new()
    } else {
        // Never echo the supplied URL: it may contain credentials we must reject.
        let url = url::Url::parse(&address).map_err(|_| "Invalid proxy URL")?;
        if !["http", "https", "socks5"].contains(&url.scheme()) || url.host().is_none() || url.port_or_known_default().filter(|p| *p > 0).is_none()
            || !url.username().is_empty() || url.password().is_some() || address.contains('@') || url.path() != "/" && !url.path().is_empty() || url.query().is_some() || url.fragment().is_some()
        {
            return Err("Proxy URL must contain only an http, https or socks5 scheme, host and port".into());
        }
        url.to_string()
    };
    let mut overrides = Map::new();
    for (id, value) in raw.get("agents").and_then(Value::as_object).into_iter().flatten() {
        if id.is_empty() || id.len() > 128 || id.chars().any(char::is_control) || !matches!(value.as_str(), Some("inherit" | "custom" | "direct")) {
            return Err("Invalid agent proxy mode".into());
        }
        overrides.insert(id.clone(), value.clone());
    }
    if address.is_empty() && (mode == "custom" || overrides.values().any(|v| v == "custom")) {
        return Err("Custom proxy requires a URL".into());
    }
    Ok(json!({ "mode": mode, "url": address, "username": clean("username")?, "bypass": clean("bypass")?, "agents": overrides }))
}

pub fn effective_proxy(proxy: &Value, agent: &str) -> Value {
    let mut out = proxy.clone();
    out["mode"] = proxy.get("agents").and_then(|a| a.get(agent)).cloned().unwrap_or_else(|| proxy["mode"].clone());
    out.as_object_mut().unwrap().shift_remove("agents");
    out
}

pub fn proxy_snapshot(proxy: &Value, sealed: &[u8]) -> Value {
    use sha2::{Digest, Sha256};
    let mode = proxy["mode"].as_str().unwrap_or("inherit");
    let mut out = json!({ "mode": mode });
    if mode == "custom" {
        out["url"] = proxy["url"].clone();
        // An opaque revision detects credential changes without putting them in session metadata.
        let mut hash = Sha256::new();
        hash.update(proxy.to_string().as_bytes());
        hash.update(sealed);
        out["revision"] = json!(format!("{:x}", hash.finalize()));
    }
    out
}

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
        out.insert("proxy".into(), sanitize_proxy(global.get("proxy")).unwrap_or_else(|_| sanitize_proxy(None).unwrap()));
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
        // Where node worktrees go comes only from the user's file: a repo cannot point it elsewhere.
        // Cowork 2.0 keeps only this key; the old planning settings are dropped (the legacy inventory reads the raw file).
        let root = global.pointer("/cowork/worktreeRoot").and_then(Value::as_str).map(|s| crate::util::js_prefix(s.trim(), 500)).unwrap_or_default();
        // Structured approval per CLI (§3.4): Claude is session-only so on by default, the other two change the CLI's user setup
        let hook = |agent: &str| global.pointer(&format!("/cowork/structuredApproval/{agent}")).and_then(Value::as_bool).unwrap_or(agent == "claude");
        out.insert("cowork".into(), json!({ "worktreeRoot": root, "structuredApproval": { "claude": hook("claude"), "codex": hook("codex"), "antigravity": hook("antigravity") } }));
        Value::Object(out)
    }

    pub fn save_settings(&self, s: &Value) -> Result<(), String> {
        let mut s = s.clone();
        s["proxy"] = sanitize_proxy(s.get("proxy"))?;
        let gp = self.global_settings_path();
        if let Some(d) = gp.parent() {
            fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        fs::write(&gp, serde_json::to_string_pretty(&s).unwrap()).map_err(|e| e.to_string())?;
        if let Some(wp) = self.workspace_settings_path() {
            let mut copy = s.clone();
            if let Some(o) = copy.as_object_mut() {
                o.shift_remove("customAgent");
                o.shift_remove("proxy");
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
        updated["proxy"] = sanitize_proxy(patch.get("proxy").or(current.get("proxy")))?;
        updated["customAgent"] = agent;
        for key in ["cliPaths", "cliEnabled", "docToolPaths"] {
            updated[key] = merge(current.get(key), patch.get(key));
        }
        self.save_settings(&updated)?;
        // agy's hook lives in its user-level hooks.json: add or remove only our entry, right when the switch changes
        if patch.get("cowork").is_some() || patch.get("cliBypassPermissions").is_some() {
            let _ = crate::approval_hook::sync_agy(self);
        }
        if patch.get("customAgent").is_some() || patch.get("proxy").is_some() {
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
            "settings:proxyPassword" => {
                let password = args.first().and_then(Value::as_str).ok_or("Invalid proxy password")?;
                if password.len() > 8192 || password.contains('\0') {
                    return Err("Invalid proxy password".into());
                }
                let path = self.proxy_secret_path();
                if password.is_empty() {
                    match fs::remove_file(path) {
                        Ok(()) => {},
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
                        Err(_) => return Err("Could not clear proxy password".into()),
                    }
                } else {
                    let sealed = wb_secrets::protect(password.as_bytes()).map_err(|_| "Could not encrypt proxy password")?;
                    fs::write(path, sealed).map_err(|_| "Could not save proxy password")?;
                }
                self.host.send_all("settings:changed", json!([]));
                Ok(Value::Null)
            }
            "settings:proxyStatus" => {
                let proxy = self.load_settings()["proxy"].clone();
                let sealed = self.proxy_sealed()?;
                let sessions: Vec<Value> = self.active_session_metas().iter().map(|meta| {
                    let agent = meta.get("launcherId").and_then(Value::as_str).unwrap_or_else(|| meta["command"].as_str().unwrap_or("shell"));
                    let current = proxy_snapshot(&effective_proxy(&proxy, agent), &sealed);
                    json!({ "id": meta["id"], "agent": agent, "started": meta["proxy"], "restartNeeded": meta["proxy"] != current })
                }).collect();
                Ok(json!({ "passwordSaved": !sealed.is_empty(), "sessions": sessions }))
            }
            "settings:testProxy" => {
                use std::net::TcpStream;
                use std::time::{Duration, Instant};
                let proxy = sanitize_proxy(Some(&json!({ "mode": "custom", "url": arg_str(args, 0) })))?;
                let url = url::Url::parse(proxy["url"].as_str().unwrap()).map_err(|_| "Invalid proxy URL")?;
                let addresses = url.socket_addrs(|| None).map_err(|_| "Proxy host could not be resolved")?;
                let deadline = Instant::now() + Duration::from_secs(3);
                for address in addresses {
                    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else { break };
                    if TcpStream::connect_timeout(&address, remaining).is_ok() {
                        return Ok(json!({ "ok": true }));
                    }
                }
                Ok(json!({ "ok": false }))
            }
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

    pub fn proxy_secret_path(&self) -> PathBuf {
        self.data_dir.join("proxy-password.enc")
    }

    pub fn proxy_sealed(&self) -> Result<Vec<u8>, String> {
        match fs::read(self.proxy_secret_path()) {
            Ok(sealed) => Ok(sealed),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(_) => Err("Could not read proxy password".into()),
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

#[cfg(test)]
mod proxy_tests {
    use super::*;

    struct QuietHost;
    impl crate::Host for QuietHost {
        fn send(&self, _: u32, _: &str, _: Value) {}
        fn send_all(&self, _: &str, _: Value) {}
        fn focused_window(&self) -> Option<u32> { None }
        fn windows(&self) -> Vec<u32> { Vec::new() }
        fn pick_folder(&self, _: u32, _: &str) -> Option<String> { None }
        fn pick_file(&self, _: &str) -> Option<String> { None }
        fn open_path(&self, _: &str) -> Result<(), String> { Ok(()) }
        fn show_in_folder(&self, _: &str) {}
        fn open_url(&self, _: &str) {}
        fn notify(&self, _: &str, _: &str) {}
        fn set_title(&self, _: u32, _: &str) {}
        fn open_project_window(&self, _: Option<&str>) -> bool { false }
        fn open_terminal_window(&self) -> bool { false }
        fn close_terminal_window(&self) -> bool { false }
        fn set_title_bar_theme(&self, _: u32, _: &str) -> bool { false }
    }

    #[test]
    fn proxy_is_global_only_and_password_never_enters_settings() {
        let dir = std::env::temp_dir().join(format!("wb-proxy-settings-{}", crate::util::uuid()));
        let workspace = dir.join("repo");
        fs::create_dir_all(workspace.join(".workbench")).unwrap();
        let core = Arc::new(Core {
            host: Arc::new(QuietHost), data_dir: dir.join("user"), info: Default::default(),
            windows: std::sync::Mutex::new(crate::core::WindowState { default_workspace: workspace.to_str().unwrap().into(), ..Default::default() }),
            scanner: crate::session_scan::Scanner::new(dir.join("user")), bg: Default::default(), watchers: Default::default(),
            pty: Default::default(), cached_sessions: Default::default(), features: Default::default(),
        });
        let ws_path = workspace.join(".workbench/settings.json");
        fs::write(&ws_path, json!({ "proxy": { "mode": "custom", "url": "http://evil:8080", "agents": { "claude": "direct" } } }).to_string()).unwrap();
        assert_eq!(core.load_settings()["proxy"]["mode"], "inherit");
        assert_eq!(core.load_settings()["proxy"]["agents"], json!({}));
        let settings = core.settings_call("settings:set", &[json!({ "proxy": { "mode": "direct", "password": "never-save-me" } })]).unwrap();
        assert!(!settings.to_string().contains("never-save-me"));
        assert!(!fs::read_to_string(core.global_settings_path()).unwrap().contains("never-save-me"));
        assert!(read_json(&ws_path).get("proxy").is_none());
        assert_eq!(core.load_settings()["proxy"]["mode"], "direct");
        #[cfg(windows)] {
            core.settings_call("settings:proxyPassword", &[json!("encrypted-only")]).unwrap();
            let sealed = core.proxy_sealed().unwrap();
            assert_ne!(sealed, b"encrypted-only");
            assert_eq!(wb_secrets::unprotect(&sealed).unwrap(), b"encrypted-only");
            let status = core.settings_call("settings:proxyStatus", &[]).unwrap();
            assert_eq!(status["passwordSaved"], true);
            assert!(!status.to_string().contains("encrypted-only"));
            assert!(!core.load_settings().to_string().contains("encrypted-only"));
            assert!(!fs::read_to_string(core.global_settings_path()).unwrap().contains("encrypted-only"));
            core.settings_call("settings:proxyPassword", &[json!("")]).unwrap();
            assert!(core.proxy_sealed().unwrap().is_empty());
        }
        drop(core);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn proxy_settings_reject_credentials_and_invalid_addresses() {
        for address in ["http://u:secret@proxy:80", "http://u@proxy:80", "file:///proxy", "socks5://proxy", "http://proxy:0", "http://proxy:99999", "https://proxy:443/path", "https://proxy:443?q=secret", "http://proxy:80/#secret"] {
            let error = sanitize_proxy(Some(&json!({ "url": address }))).unwrap_err();
            assert!(!error.contains("secret"));
        }
        for address in ["http://localhost:7890", "https://proxy:443", "socks5://[::1]:1080"] {
            assert!(sanitize_proxy(Some(&json!({ "mode": "custom", "url": address }))).is_ok());
        }
        assert!(sanitize_proxy(Some(&json!({ "mode": "bad" }))).is_err());
        assert!(sanitize_proxy(Some(&json!({ "agents": { "claude": { "mode": "custom", "url": "http://evil:80" } } }))).is_err());
        assert!(sanitize_proxy(Some(&json!({ "agents": { "claude": "custom" } }))).is_err());
        let config = sanitize_proxy(Some(&json!({ "password": "secret", "mode": "inherit", "agents": { "claude": "direct" } }))).unwrap();
        assert!(!config.to_string().contains("secret"));
        assert_eq!(effective_proxy(&config, "claude")["mode"], "direct");
        assert_eq!(effective_proxy(&config, "codex")["mode"], "inherit");
    }

    #[test]
    fn proxy_snapshot_tracks_only_effective_changes_and_never_credentials() {
        let proxy = sanitize_proxy(Some(&json!({ "mode": "custom", "url": "http://proxy:8080", "username": "private-user", "bypass": "internal" }))).unwrap();
        let start = proxy_snapshot(&proxy, b"encrypted-password");
        assert!(!start.to_string().contains("private-user"));
        assert!(!start.to_string().contains("encrypted-password"));
        assert_ne!(start, proxy_snapshot(&proxy, b"new-encrypted-password"));
        let mut changed = proxy.clone();
        changed["bypass"] = json!("other");
        assert_ne!(start, proxy_snapshot(&changed, b"encrypted-password"));
        changed["mode"] = json!("inherit");
        assert_eq!(proxy_snapshot(&changed, b"one"), proxy_snapshot(&changed, b"two"));
    }
}
