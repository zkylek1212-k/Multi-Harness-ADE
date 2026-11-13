//! Structured approval, layer 2 (cowork2.0-implement.md §3.4, Phase 0 results): the named-pipe endpoint wb-hook talks
//! to, the three CLIs' inputs turned into one card, and how each CLI gets the hook for a canvas session.
//! Every failure answers "pass", which hands the question back to the CLI's own prompt; nothing here ever allows.
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};

use crate::canvas::HookDecision;
use crate::core::Core;
use crate::util::{home, js_prefix, mask_secrets, p, read_text, uuid, write_json_atomic};

/// wb-hook gives up at 590 s; the CLIs are told 600 s, so wb-hook always answers before the CLI's own timeout
const HOOK_TIMEOUT_S: u64 = 600;
const WAIT: Duration = Duration::from_secs(580);
const PIPE_PREFIX: &str = r"\\.\pipe\agent-workbench-hook-";
const MAX_INPUT: usize = 4 * 1024 * 1024;
const AGY_ENTRY: &str = "agent-workbench";

pub struct Mount {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub token: Option<String>,
}

impl Mount {
    fn none() -> Self {
        Mount { args: vec![], env: vec![], token: None }
    }
}

/// Per CLI, from the user's own settings. Claude is session-only, so it is on by default; the other two touch the
/// CLI's user-level setup (Codex asks for a one-time trust, agy needs a global hooks.json entry), so they are opt-in.
pub fn enabled(core: &Core, agent: &str) -> bool {
    let v = core.load_settings().pointer(&format!("/cowork/structuredApproval/{agent}")).and_then(Value::as_bool);
    v.unwrap_or(agent == "claude")
}

/// Next to the app in development (cargo builds it there), `bin\` in an install or the portable zip (release.ps1)
pub fn hook_exe() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    [dir.join("wb-hook.exe"), dir.join("bin").join("wb-hook.exe")].into_iter().find(|p| p.exists())
}

/// `cmd /c call` (how npm .cmd shims run) mangles quotes inside an argument, so a command embedded in an argument
/// must need none: the 8.3 path when the real one has spaces, or no hook at all.
fn plain_path(path: &Path) -> Option<String> {
    let ok = |s: &str| s.bytes().all(|b| b.is_ascii_alphanumeric() || b"\\/:._-~".contains(&b));
    let s = p(path);
    if ok(&s) {
        return Some(s);
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        extern "system" {
            fn GetShortPathNameW(long: *const u16, short: *mut u16, len: u32) -> u32;
        }
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut buf = vec![0u16; 1024];
        // SAFETY: both buffers are valid for the lengths given; the call writes at most `len` units
        let n = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
        if n > 0 && n < buf.len() {
            let short = String::from_utf16_lossy(&buf[..n]);
            if ok(&short) {
                return Some(short);
            }
        }
    }
    None
}

static PIPE: OnceLock<Option<String>> = OnceLock::new();

/// Team tools (the `workbench` MCP server) per CLI; on by default for all three (cowork-team-implement.md §4.5).
/// Independent of Bypass: a team needs its channel whether or not the CLI asks before editing.
pub fn mcp_enabled(core: &Core, agent: &str) -> bool {
    core.load_settings().pointer(&format!("/cowork/teamMcp/{agent}")).and_then(Value::as_bool).unwrap_or(true)
}

/// The MCP server's name in every CLI: Claude's tools become mcp__workbench__*, agy's rule mcp(workbench/*)
const MCP_NAME: &str = "workbench";

/// What one new canvas session gets: the approval hook (Phase 6) and the team MCP server, or neither.
/// Without them the CLI asks in its own terminal (layer 1) and the node has no team tools.
pub fn mount(core: &Arc<Core>, agent: &str) -> Mount {
    let cli = match agent {
        "claude" => "claude",
        "codex" => "codex",
        "antigravity" => "agy",
        _ => return Mount::none(),
    };
    if cli == "agy" {
        let _ = sync_agy(core);
    }
    // With permissions bypassed the CLI never asks, so there is nothing to hook
    let hook = !core.is_cli_bypass() && enabled(core, agent);
    let mcp = mcp_enabled(core, agent);
    if !hook && !mcp {
        return Mount::none();
    }
    let Some(exe) = hook_exe() else { return Mount::none() };
    let Some(pipe) = PIPE.get_or_init(|| start_server(core)).clone() else { return Mount::none() };
    let token = uuid();
    let mut args: Vec<String> = Vec::new();
    match cli {
        "claude" => {
            let dir = core.data_dir.join("canvas");
            let _ = std::fs::create_dir_all(&dir);
            if hook {
                let file = dir.join("claude-hooks.json");
                let settings = json!({ "hooks": { "PermissionRequest": [{ "matcher": "*", "hooks": [{ "type": "command", "command": format!("\"{}\" claude", p(&exe)), "timeout": HOOK_TIMEOUT_S }] }] } });
                if write_json_atomic(&file, &settings).is_ok() {
                    args.extend(["--settings".to_string(), plain_path(&file).unwrap_or_else(|| p(&file))]);
                }
            }
            if mcp {
                // Claude starts the server itself, with no shell in between: the path may have spaces
                let file = dir.join("claude-mcp.json");
                let config = json!({ "mcpServers": { MCP_NAME: { "command": p(&exe), "args": ["mcp"] } } });
                if write_json_atomic(&file, &config).is_ok() {
                    args.extend(["--mcp-config".to_string(), plain_path(&file).unwrap_or_else(|| p(&file)), "--allowedTools".to_string(), format!("mcp__{MCP_NAME}__*")]);
                }
            }
        }
        "codex" => {
            // codex.cmd runs through cmd /c call: no double quotes in any value, so TOML single-quoted strings only
            if let Some(cmd) = plain_path(&exe) {
                if hook {
                    // Codex trusts a hook by the hash of exactly this text: it must never change between launches
                    args.extend(["-c".into(), format!("hooks.PermissionRequest=[{{matcher='.*',hooks=[{{type='command',command='{cmd} codex',timeout={HOOK_TIMEOUT_S}}}]}}]")]);
                }
                if mcp {
                    // Codex hands MCP servers only the variables listed in env_vars (Phase 0)
                    for kv in [format!("mcp_servers.{MCP_NAME}.command='{cmd}'"), format!("mcp_servers.{MCP_NAME}.args=['mcp']"), format!("mcp_servers.{MCP_NAME}.env_vars=['WB_HOOK_PIPE','WB_HOOK_TOKEN']"), format!("mcp_servers.{MCP_NAME}.default_tools_approval_mode='approve'")] {
                        args.extend(["-c".to_string(), kv]);
                    }
                }
            }
        }
        // agy: both are user-level entries kept by sync_agy; the session only needs the environment
        _ => {}
    }
    let env = vec![("WB_HOOK_PIPE".into(), pipe), ("WB_HOOK_TOKEN".into(), token.clone()), ("WB_HOOK_TIMEOUT_MS".into(), "590000".into())];
    Mount { args, env, token: Some(token) }
}

fn agy_hooks_file() -> PathBuf {
    home().join(".gemini").join("config").join("hooks.json")
}

fn agy_mcp_file() -> PathBuf {
    home().join(".gemini").join("config").join("mcp_config.json")
}

fn agy_settings_file() -> PathBuf {
    home().join(".gemini").join("antigravity-cli").join("settings.json")
}

/// Reads a user JSON file for an in-place edit; a file that is not a JSON object is never touched
fn read_obj(file: &Path, create: bool) -> Result<Option<Value>, String> {
    match read_text(file) {
        None if !create => Ok(None),
        Some(t) if !t.trim().is_empty() => {
            let v: Value = serde_json::from_str(&t).map_err(|_| format!("{} is not valid JSON; left untouched", p(file)))?;
            if v.is_object() {
                Ok(Some(v))
            } else {
                Err(format!("{} is not an object; left untouched", p(file)))
            }
        }
        _ => Ok(Some(json!({}))),
    }
}

fn write_obj(file: &Path, v: &Value) -> Result<(), String> {
    if let Some(d) = file.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    write_json_atomic(file, v)
}

/// agy's setup lives in its user files, so this adds or removes only our own named pieces and keeps the rest:
/// the approval hook in hooks.json, the `workbench` server in mcp_config.json and its `mcp(workbench/*)` allow
/// rule in settings.json. agy's `--settings` cannot carry MCP servers (Phase 0), and a session `permissions` would
/// replace the user's own allow list for that session, so the rule is user-level too. Outside the app the server
/// offers no tools, so the rule allows nothing there.
pub fn sync_agy(core: &Core) -> Result<(), String> {
    // agy runs hook commands through cmd with Go-style \" escapes, which cmd takes literally: the command must need
    // no quotes, like Codex's. Its stderr ("not recognized", CP950) then broke agy's protobuf (live test 2026-11-13).
    let plain = hook_exe().and_then(|exe| plain_path(&exe));
    let want_hook = enabled(core, "antigravity") && !core.is_cli_bypass();
    let want_mcp = mcp_enabled(core, "antigravity");

    let file = agy_hooks_file();
    if let Some(mut v) = read_obj(&file, want_hook)? {
        let obj = v.as_object_mut().unwrap();
        let entry = plain.as_ref().map(|cmd| json!({ "PreToolUse": [{ "matcher": "write_file|edit_file", "hooks": [{ "type": "command", "command": format!("{cmd} agy"), "timeout": HOOK_TIMEOUT_S }] }] }));
        let changed = match (want_hook, entry) {
            (true, Some(e)) if obj.get(AGY_ENTRY) == Some(&e) => false,
            (true, Some(e)) => {
                obj.insert(AGY_ENTRY.into(), e);
                true
            }
            _ => obj.shift_remove(AGY_ENTRY).is_some(),
        };
        if changed {
            write_obj(&file, &v)?;
        }
    }

    let file = agy_mcp_file();
    if let Some(mut v) = read_obj(&file, want_mcp)? {
        let servers = v.as_object_mut().unwrap().entry("mcpServers").or_insert_with(|| json!({}));
        let Some(servers) = servers.as_object_mut() else { return Err("agy mcpServers is not an object; left untouched".into()) };
        let entry = plain.as_ref().map(|cmd| json!({ "command": cmd, "args": ["mcp"] }));
        let changed = match (want_mcp, entry) {
            (true, Some(e)) if servers.get(MCP_NAME) == Some(&e) => false,
            (true, Some(e)) => {
                servers.insert(MCP_NAME.into(), e);
                true
            }
            _ => servers.shift_remove(MCP_NAME).is_some(),
        };
        if changed {
            write_obj(&file, &v)?;
        }
    }

    let file = agy_settings_file();
    let rule = format!("mcp({MCP_NAME}/*)");
    if let Some(mut v) = read_obj(&file, want_mcp)? {
        let perms = v.as_object_mut().unwrap().entry("permissions").or_insert_with(|| json!({}));
        let Some(perms) = perms.as_object_mut() else { return Ok(()) };
        let allow = perms.entry("allow").or_insert_with(|| json!([]));
        let Some(allow) = allow.as_array_mut() else { return Ok(()) };
        let has = allow.iter().any(|x| x.as_str() == Some(rule.as_str()));
        let changed = if want_mcp && plain.is_some() && !has {
            allow.push(json!(rule));
            true
        } else if (!want_mcp || plain.is_none()) && has {
            allow.retain(|x| x.as_str() != Some(rule.as_str()));
            true
        } else {
            false
        };
        if changed {
            write_obj(&file, &v)?;
        }
    }
    Ok(())
}

// ---------- endpoint ----------

fn start_server(core: &Arc<Core>) -> Option<String> {
    let name = format!("{PIPE_PREFIX}{}", uuid());
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let weak = Arc::downgrade(core);
    let n2 = name.clone();
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
        rt.block_on(serve(n2, weak, ready_tx));
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).ok().filter(|ok| *ok).map(|_| name)
}

#[cfg(windows)]
async fn serve(name: String, core: std::sync::Weak<Core>, ready: std::sync::mpsc::Sender<bool>) {
    use tokio::net::windows::named_pipe::ServerOptions;
    // Remote clients are refused: the pipe is for this machine's CLIs only
    let new = |first: bool| ServerOptions::new().first_pipe_instance(first).reject_remote_clients(true).create(&name);
    let mut server = match new(true) {
        Ok(s) => s,
        Err(_) => {
            let _ = ready.send(false);
            return;
        }
    };
    let _ = ready.send(true);
    loop {
        if server.connect().await.is_err() {
            continue;
        }
        let conn = server;
        server = match new(false) {
            Ok(s) => s,
            Err(_) => return,
        };
        let core = core.clone();
        tokio::spawn(async move {
            let _ = handle(conn, core).await;
        });
    }
}

#[cfg(not(windows))]
async fn serve(_name: String, _core: std::sync::Weak<Core>, ready: std::sync::mpsc::Sender<bool>) {
    let _ = ready.send(false);
}

#[cfg(windows)]
async fn handle(mut conn: tokio::net::windows::named_pipe::NamedPipeServer, core: std::sync::Weak<Core>) -> std::io::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    let (token, cli, input) = {
        let mut r = BufReader::new(&mut conn);
        let mut head = String::new();
        tokio::time::timeout(Duration::from_secs(10), r.read_line(&mut head)).await.map_err(|_| std::io::ErrorKind::TimedOut)??;
        let parts: Vec<&str> = head.trim_end().split(' ').collect();
        // `WBMCP1 <token> <len>`: a team tool request from `wb-hook mcp`; the reply is `<len>\n<json>`
        if let (Some(&"WBMCP1"), Some(token), Some(len)) = (parts.first(), parts.get(1), parts.get(2).and_then(|l| l.parse::<usize>().ok())) {
            if len > MAX_INPUT {
                return Ok(());
            }
            let mut body = vec![0; len];
            tokio::time::timeout(Duration::from_secs(10), r.read_exact(&mut body)).await.map_err(|_| std::io::ErrorKind::TimedOut)??;
            let (token, req) = (token.to_string(), serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null));
            drop(r);
            let Some(core) = core.upgrade() else { return Ok(()) };
            let out = tokio::task::spawn_blocking(move || crate::canvas::mcp_request(&core, &token, &req)).await.unwrap_or(json!({ "ok": false, "text": "internal error" }));
            let bytes = serde_json::to_vec(&out).unwrap_or_default();
            conn.write_all(format!("{}\n", bytes.len()).as_bytes()).await?;
            conn.write_all(&bytes).await?;
            return conn.flush().await;
        }
        let (Some(&"WBHOOK1"), Some(token), Some(cli), Some(len)) = (parts.first(), parts.get(1), parts.get(2), parts.get(3).and_then(|l| l.parse::<usize>().ok())) else {
            return conn.write_all(b"pass\n").await;
        };
        if len > MAX_INPUT {
            return conn.write_all(b"pass\n").await;
        }
        let mut body = vec![0; len];
        tokio::time::timeout(Duration::from_secs(10), r.read_exact(&mut body)).await.map_err(|_| std::io::ErrorKind::TimedOut)??;
        (token.to_string(), cli.to_string(), body)
    };
    let Some(core) = core.upgrade() else { return conn.write_all(b"pass\n").await };
    let card = card(&cli, &String::from_utf8_lossy(&input));
    let (c2, t2) = (core.clone(), token.clone());
    let mut decide = tokio::task::spawn_blocking(move || crate::canvas::hook_request(&c2, &t2, card, WAIT));
    // The CLI kills wb-hook when the user answers in the terminal first: that must expire the card at once
    let mut probe = [0u8; 1];
    let decision = tokio::select! {
        d = &mut decide => d.unwrap_or(HookDecision::Pass),
        _ = conn.read(&mut probe) => {
            let (c3, t3) = (core.clone(), token.clone());
            let _ = tokio::task::spawn_blocking(move || crate::canvas::hook_gone(&c3, &t3)).await;
            let _ = decide.await;
            return Ok(());
        }
    };
    let line = match decision {
        HookDecision::Allow => "allow\n".to_string(),
        HookDecision::Deny(m) => format!("deny {}\n", js_prefix(&m.replace(['\r', '\n'], " "), 300)),
        HookDecision::Pass => "pass\n".to_string(),
    };
    conn.write_all(line.as_bytes()).await?;
    conn.flush().await
}

// ---------- the three CLIs' inputs as one card (Phase 0 field names) ----------

/// Lines added and removed, after trimming the common head and tail: enough for "+12 -3" on a card or a push
fn counts(before: &str, after: &str) -> (usize, usize) {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = a[head..].iter().rev().zip(b[head..].iter().rev()).take_while(|(x, y)| x == y).count();
    (b.len() - head - tail, a.len() - head - tail)
}

fn file_card(path: &str, before: Option<String>, after: String) -> Value {
    let (added, removed) = counts(before.as_deref().unwrap_or(""), &after);
    json!({ "path": path, "added": added, "removed": removed, "before": before.map(|b| mask_secrets(&js_prefix(&b, 256 * 1024))), "after": mask_secrets(&js_prefix(&after, 256 * 1024)) })
}

fn current(path: &str) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > 256 * 1024 {
        return None;
    }
    read_text(Path::new(path))
}

/// Codex apply_patch: one entry per file with its hunk text and counts
fn patch_files(patch: &str) -> Vec<Value> {
    let mut out: Vec<(String, String, usize, usize)> = Vec::new();
    for line in patch.lines() {
        if let Some(path) = ["*** Add File: ", "*** Update File: ", "*** Delete File: "].iter().find_map(|p| line.strip_prefix(p)) {
            out.push((path.trim().to_string(), String::new(), 0, 0));
            continue;
        }
        if line.starts_with("*** ") {
            continue;
        }
        if let Some(f) = out.last_mut() {
            f.1.push_str(line);
            f.1.push('\n');
            if line.starts_with('+') {
                f.2 += 1;
            } else if line.starts_with('-') {
                f.3 += 1;
            }
        }
    }
    out.into_iter().map(|(path, text, added, removed)| json!({ "path": path, "added": added, "removed": removed, "patch": mask_secrets(&js_prefix(&text, 256 * 1024)) })).collect()
}

pub fn card(cli: &str, raw: &str) -> Value {
    let v: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let raw_shown = mask_secrets(&js_prefix(raw, 64 * 1024));
    let str_of = |x: &Value| match x {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "),
        _ => String::new(),
    };
    let (tool, input) = if cli == "agy" { (v["toolCall"]["name"].as_str().unwrap_or(""), &v["toolCall"]["args"]) } else { (v["tool_name"].as_str().unwrap_or(""), &v["tool_input"]) };
    let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut files = Vec::new();
    let mut command = None;
    match (cli, tool) {
        ("claude", "Write") => files.push(file_card(&s("file_path"), current(&s("file_path")), s("content"))),
        ("claude", "Edit") => files.push(file_card(&s("file_path"), Some(s("old_string")), s("new_string"))),
        ("claude", "MultiEdit") => {
            for e in input["edits"].as_array().into_iter().flatten() {
                files.push(file_card(&s("file_path"), e["old_string"].as_str().map(str::to_string), e["new_string"].as_str().unwrap_or("").to_string()));
            }
        }
        ("codex", "apply_patch") => {
            let patch = str_of(&input["command"]);
            files = patch_files(&patch);
            command = Some(patch);
        }
        ("agy", "write_file") => files.push(file_card(&s("Path"), current(&s("Path")), s("Content"))),
        ("agy", "edit_file") => files.push(file_card(&s("Path"), Some(s("Target")), s("Replacement"))),
        _ => {
            let c = str_of(&input["command"]);
            let c = if c.is_empty() { str_of(&input["CommandLine"]) } else { c };
            if !c.is_empty() {
                command = Some(c);
            }
        }
    }
    let target = files.first().and_then(|f| f["path"].as_str()).map(|p| crate::util::basename(p)).or_else(|| command.as_deref().map(|c| js_prefix(c.lines().next().unwrap_or(""), 80)));
    let title = match target {
        Some(t) => format!("{} {}", if tool.is_empty() { "Permission" } else { tool }, t),
        None => if tool.is_empty() { "Permission request".to_string() } else { tool.to_string() },
    };
    json!({ "title": title, "hook": { "tool": tool, "files": files, "command": command.map(|c| mask_secrets(&js_prefix(&c, 64 * 1024))), "raw": raw_shown } })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Inputs recorded in the Phase 0 spike (Claude 2.1.322, Codex 0.167.2, agy 1.5.6), paths shortened
    #[test]
    fn cards_from_recorded_inputs() {
        let claude = r#"{"session_id":"s","prompt_id":"p","permission_mode":"default","hook_event_name":"PermissionRequest","tool_name":"Edit","tool_input":{"file_path":"C:\\r\\README.md","old_string":"spike","new_string":"spiked","replace_all":false}}"#;
        let c = card("claude", claude);
        assert_eq!(c["title"], "Edit README.md");
        assert_eq!(c["hook"]["files"][0]["before"], "spike");
        assert_eq!((c["hook"]["files"][0]["added"].as_u64(), c["hook"]["files"][0]["removed"].as_u64()), (Some(1), Some(1)));

        let codex = r#"{"session_id":"s","turn_id":"t","tool_name":"apply_patch","tool_input":{"command":"*** Begin Patch\n*** Add File: l.txt\n+hi\n*** Update File: a.rs\n@@\n-old\n+new\n+more\n*** End Patch"}}"#;
        let c = card("codex", codex);
        let f = c["hook"]["files"].as_array().unwrap();
        assert_eq!((f.len(), f[0]["path"].as_str(), f[0]["added"].as_u64()), (2, Some("l.txt"), Some(1)));
        assert_eq!((f[1]["added"].as_u64(), f[1]["removed"].as_u64()), (Some(2), Some(1)));

        let agy = r#"{"toolCall":{"name":"edit_file","args":{"Path":"C:\\r\\README.md","Target":"spike","Replacement":"spiked"}},"hookEventName":"PreToolUse"}"#;
        assert_eq!(card("agy", agy)["hook"]["files"][0]["after"], "spiked");

        let garbage = card("codex", "not json");
        assert_eq!(garbage["title"], "Permission request");
        assert_eq!(garbage["hook"]["raw"], "not json");
    }

    #[test]
    fn counts_trim_common_lines() {
        assert_eq!(counts("a\nb\nc", "a\nx\nc"), (1, 1));
        assert_eq!(counts("", "a\nb"), (2, 0));
        assert_eq!(counts("a\nb", "a\nb"), (0, 0));
    }

    #[test]
    fn plain_paths_need_no_quotes() {
        assert_eq!(plain_path(Path::new(r"C:\a\wb-hook.exe")).as_deref(), Some(r"C:\a\wb-hook.exe"));
        assert_eq!(plain_path(Path::new(r"C:\no such dir\x y.exe")), None, "no short name for a missing file");
    }
}
