// Terminals. A pty has any number of subscribers (desktop windows, a phone). Output is fanned out under the mirror
// lock, so a subscriber added with a snapshot sits at an exact stream position: nothing missed, nothing repeated.
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::{json, Value};
use wb_proc::KillOnCloseJob;
use wb_pty::approval::looks_like_approval_prompt;
use wb_pty::{PtyOptions, PtySession};

use crate::conn::resolve_connection_env;
use crate::core::{arg_str, Core};
use crate::custom_agent::{sanitize_custom_agent, CUSTOM_AGENT_ID};
use crate::paths::find_cli;
use crate::util::{exists, home, now_ms, p, read_text, sv, uuid};

const TAIL_LIMIT: usize = 16 * 1024;
const SCREEN_SCROLLBACK: usize = 5000;
const BUSY_IDLE: Duration = Duration::from_millis(4000);
/// Coalesce ANSI fragments first, so a partial redraw is not read as a new question
const APPROVAL_DELAY: Duration = Duration::from_millis(120);

pub trait PtySub: Send + Sync {
    fn data(&self, id: &str, chunk: &str);
    fn exit(&self, id: &str, code: i32);
    fn resized(&self, _id: &str, _cols: u16, _rows: u16) {}
}

struct WindowSub {
    window: u32,
    core: Weak<Core>,
}

impl PtySub for WindowSub {
    fn data(&self, id: &str, chunk: &str) {
        if let Some(c) = self.core.upgrade() {
            c.host.send(self.window, &format!("pty:data:{id}"), json!([chunk]));
        }
    }
    fn exit(&self, id: &str, code: i32) {
        if let Some(c) = self.core.upgrade() {
            c.host.send(self.window, &format!("pty:exit:{id}"), json!([code]));
        }
    }
    fn resized(&self, id: &str, cols: u16, rows: u16) {
        if let Some(c) = self.core.upgrade() {
            c.host.send(self.window, &format!("pty:resized:{id}"), json!([cols, rows]));
        }
    }
}

#[derive(Clone, Debug)]
pub enum PtyEvent {
    Changed,
    Approval(String),
    Exit { id: String, code: i32, title: String, killed: bool },
    Activity(String, bool),
}

/// A byte stream split anywhere: keeps an incomplete character for the next chunk
#[derive(Default)]
struct Utf8Stream(Vec<u8>);

impl Utf8Stream {
    fn push(&mut self, bytes: &[u8]) -> String {
        self.0.extend_from_slice(bytes);
        let keep = match std::str::from_utf8(&self.0) {
            Ok(_) => 0,
            Err(e) if e.error_len().is_none() => self.0.len() - e.valid_up_to(),
            Err(_) => {
                // Invalid in the middle: find a possible incomplete tail after it
                let n = self.0.len();
                (1..=3.min(n)).find(|i| self.0[n - i] >= 0xC0).filter(|i| std::str::from_utf8(&self.0[n - i..]).err().map(|e| e.error_len().is_none()).unwrap_or(false)).unwrap_or(0)
            }
        };
        let split = self.0.len() - keep;
        let text = String::from_utf8_lossy(&self.0[..split]).into_owned();
        self.0.drain(..split);
        text
    }
}

#[derive(Default)]
struct Live {
    needs_approval: bool,
    dirty: Option<Instant>,
    last_output: Option<Instant>,
    last_output_ms: f64,
    busy: bool,
    dev_port: Option<u16>,
    tail: String,
    killed: bool,
    cols: u16,
    rows: u16,
}

struct Shared {
    live: Mutex<Live>,
    subs: Mutex<HashMap<String, Arc<dyn PtySub>>>,
    stream: Mutex<Utf8Stream>,
}

struct Entry {
    session: Arc<PtySession>,
    shared: Arc<Shared>,
    meta: Value,
    title: String,
    owner: u32,
    workspace: String,
    reprints_on_resize: bool,
    /// Identity, for "this exit belongs to the entry still in the table"
    serial: u64,
}

/// `cmd /c "a b.cmd" "c d"` is misread: a command line starting with a quote loses its first and last quote, so a script under a
/// path with spaces plus any quoted argument fails to start. Starting with `call` keeps the quotes as written.
fn batch_args(script: String) -> Vec<String> {
    vec!["/c".into(), "call".into(), script]
}

#[derive(Default)]
pub struct PtyManager {
    entries: Mutex<HashMap<String, Entry>>,
    listeners: Mutex<Vec<Arc<dyn Fn(&PtyEvent) + Send + Sync>>>,
    job: OnceLock<Option<KillOnCloseJob>>,
    serial: Mutex<u64>,
}

impl PtyManager {
    pub fn on_event(&self, f: Arc<dyn Fn(&PtyEvent) + Send + Sync>) -> usize {
        let mut l = self.listeners.lock().unwrap();
        l.push(f);
        l.len() - 1
    }

    fn emit(&self, ev: PtyEvent) {
        let list: Vec<_> = self.listeners.lock().unwrap().clone();
        for f in list {
            f(&ev);
        }
    }

    fn job(&self) -> Option<&KillOnCloseJob> {
        self.job.get_or_init(|| KillOnCloseJob::new().ok()).as_ref()
    }

    pub fn assign_job(&self, pid: u32) {
        if let Some(j) = self.job() {
            let _ = j.assign(pid);
        }
    }

    /// Agents in a job die with the app if it crashes; a clean quit releases it, so independent programs survive
    pub fn release_job(&self) {
        if let Some(j) = self.job() {
            let _ = j.release();
        }
    }
}

fn dev_url_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?i)http://(?:localhost|127(?:\.\d{1,3}){3}|0\.0\.0\.0|\[::1\]):(\d{1,5})(?:[/?#\s)'"\]}>,.]|$)"#).unwrap())
}

fn ansi_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1b\\))").unwrap())
}

/// The last local dev server URL in terminal output (needs a port, at least 1024)
pub fn detect_dev_port(output: &str) -> Option<u16> {
    let plain = ansi_re().replace_all(output, "");
    let mut found = None;
    for m in dev_url_re().captures_iter(&plain) {
        if let Ok(port) = m[1].parse::<u32>() {
            if (1024..=65535).contains(&port) {
                found = Some(port as u16);
            }
        }
    }
    found
}

fn bypass_flag(agent: &str) -> Option<(&'static str, Vec<&'static str>)> {
    let a = agent.to_lowercase();
    if a.contains("claude") {
        Some(("--permission-mode", vec!["--permission-mode", "bypassPermissions"]))
    } else if a.contains("codex") {
        Some(("--dangerously-bypass-approvals-and-sandbox", vec!["--dangerously-bypass-approvals-and-sandbox"]))
    } else if a == "antigravity" || a.contains("agy") || a.contains("antigravity") {
        Some(("--dangerously-skip-permissions", vec!["--dangerously-skip-permissions"]))
    } else {
        None
    }
}

fn apply_bypass(agent: &str, args: &mut Vec<String>) {
    if let Some((marker, flags)) = bypass_flag(agent) {
        if !args.iter().any(|a| a == marker) {
            args.extend(flags.iter().map(|s| s.to_string()));
        }
    }
}

fn is_agent(name: &str) -> bool {
    matches!(name, "claude" | "antigravity" | "codex")
}

fn builtin_title(cmd: &str) -> Option<&'static str> {
    Some(match cmd {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "antigravity" => "Antigravity",
        "powershell" | "pwsh" => "PowerShell",
        "cmd" => "Command Prompt",
        "bash" => "Bash",
        _ => return None,
    })
}

const KNOWN_CLI: [&str; 7] = ["claude", "codex", "antigravity", "powershell", "pwsh", "cmd", "bash"];

fn blocked_env(k: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    // These can inject code into someone else's process, even a genuine official CLI, so a yaml file may not set them
    R.get_or_init(|| Regex::new(r"(?i)^(NODE_OPTIONS|NODE_REPL_EXTERNAL_MODULE|PATH|LD_PRELOAD|LD_AUDIT|LD_LIBRARY_PATH|DYLD_.*|PYTHONPATH|PYTHONSTARTUP|PERL5OPT|RUBYOPT|ELECTRON_RUN_AS_NODE|BASH_ENV|ENV)$").unwrap()).is_match(k)
}

struct Launcher {
    id: String,
    name: String,
    cli: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
}

/// agents/*.yaml travels with the repo: it picks from known CLIs and cannot name its own executable
fn read_launchers(ws: &str) -> Vec<Launcher> {
    // No folder is not the current directory: that is wherever the app happened to start
    if ws.is_empty() {
        return Vec::new();
    }
    let dir = Path::new(ws).join("agents");
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else { return out };
    let mut names: Vec<_> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    for file in names {
        if !(file.ends_with(".yaml") || file.ends_with(".yml")) {
            continue;
        }
        let Some(parsed) = read_text(&dir.join(&file)).and_then(|t| serde_norway::from_str::<Value>(&t).ok()) else { continue };
        let Some(l) = parsed.get("launcher") else { continue };
        let id = sv(l, "id");
        if id.is_empty() || id == CUSTOM_AGENT_ID || !KNOWN_CLI.contains(&sv(l, "cli")) {
            continue;
        }
        let mut env = Vec::new();
        for (k, v) in l.get("env").and_then(Value::as_object).into_iter().flatten() {
            if blocked_env(k) {
                continue;
            }
            match v {
                Value::String(s) => env.push((k.clone(), s.clone())),
                Value::Number(n) => env.push((k.clone(), n.to_string())),
                Value::Bool(b) => env.push((k.clone(), b.to_string())),
                _ => {}
            }
        }
        out.push(Launcher {
            id: id.to_string(),
            name: Some(sv(l, "name")).filter(|n| !n.is_empty()).unwrap_or(id).to_string(),
            cli: sv(l, "cli").to_string(),
            args: l.get("args").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect(),
            env,
        });
    }
    out
}

impl Core {
    /// Logical name to executable plus leading args; scripts get the right interpreter on Windows
    fn resolve_pty_command(&self, name: &str) -> Result<(String, Vec<String>), String> {
        let win = cfg!(windows);
        if name == CUSTOM_AGENT_ID {
            let c = sanitize_custom_agent(self.load_settings().get("customAgent"));
            let cmd = sv(&c, "command");
            if c["enabled"] != json!(true) || cmd.is_empty() {
                return Err("Custom Agent is disabled or has no executable".into());
            }
            let extra: Vec<String> = c["args"].as_array().unwrap().iter().filter_map(Value::as_str).map(str::to_string).collect();
            let target = if exists(cmd) { cmd.to_string() } else { find_cli(cmd, false).unwrap_or_else(|| cmd.to_string()) };
            let lower = target.to_lowercase();
            if win && lower.ends_with(".ps1") {
                let mut a = vec!["-ExecutionPolicy".to_string(), "Bypass".into(), "-File".into(), target];
                a.extend(extra);
                return Ok(("powershell.exe".into(), a));
            }
            if win && (lower.ends_with(".cmd") || lower.ends_with(".bat")) {
                let mut a = batch_args(target);
                a.extend(extra);
                return Ok(("cmd.exe".into(), a));
            }
            return Ok((target, extra));
        }
        let custom = self.custom_cli_path(name);
        let mut target = custom.clone().unwrap_or_else(|| name.to_string());
        if custom.is_none() {
            if is_agent(name) {
                if let Some(found) = self.find_agent_cli(name, false) {
                    target = found;
                } else if name == "antigravity" {
                    target = "agy".into();
                }
            } else {
                match name {
                    "powershell" => target = if win { "powershell.exe".into() } else { "pwsh".into() },
                    "cmd" => target = if win { "cmd.exe".into() } else { "sh".into() },
                    _ => {}
                }
            }
        }
        if win {
            let lower = target.to_lowercase();
            if ![".exe", ".cmd", ".bat", ".ps1"].iter().any(|e| lower.ends_with(e)) {
                for e in [".cmd", ".exe", ".bat", ".ps1"] {
                    if exists(&format!("{target}{e}")) {
                        target = format!("{target}{e}");
                        break;
                    }
                }
            }
            let lower = target.to_lowercase();
            if lower.ends_with(".ps1") {
                return Ok(("powershell.exe".into(), vec!["-ExecutionPolicy".into(), "Bypass".into(), "-File".into(), target]));
            }
            if lower.ends_with(".cmd") || lower.ends_with(".bat") {
                return Ok(("cmd.exe".into(), batch_args(target)));
            }
        }
        Ok((target, Vec::new()))
    }

    pub fn list_launchers(&self, ws: &str) -> Result<Value, String> {
        let bypass = self.is_cli_bypass();
        let mut out = Vec::new();
        for l in read_launchers(ws) {
            let (cmd, extra) = self.resolve_pty_command(&l.cli)?;
            let mut a = extra;
            a.extend(l.args.iter().cloned());
            if bypass {
                apply_bypass(&l.cli, &mut a);
            }
            let env: serde_json::Map<String, Value> = l.env.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
            // Only yaml args or env make it really custom; otherwise it is an alias a caller need not list twice
            out.push(json!({ "id": l.id, "name": l.name, "cli": l.cli, "command": cmd, "args": a, "env": env, "hasExtras": !l.args.is_empty() || !l.env.is_empty() }));
        }
        let c = sanitize_custom_agent(self.load_settings().get("customAgent"));
        if c["enabled"] == json!(true) && !sv(&c, "command").is_empty() {
            let (cmd, extra) = self.resolve_pty_command(CUSTOM_AGENT_ID)?;
            out.push(json!({ "id": CUSTOM_AGENT_ID, "name": sv(&c, "name"), "cli": CUSTOM_AGENT_ID, "command": cmd, "args": extra, "env": {}, "hasExtras": true }));
        }
        Ok(json!(out))
    }

    /// The one path to a new terminal, from a desktop window or from the phone. It closes with its owner window.
    pub fn spawn_pty(self: &Arc<Self>, opts: &Value, workspace: &str, owner: u32, subscribe_owner: bool) -> Result<String, String> {
        let o_cmd = opts.get("command").and_then(Value::as_str).filter(|s| !s.is_empty());
        let o_launcher = opts.get("launcherId").and_then(Value::as_str).filter(|s| !s.is_empty());
        let o_title = opts.get("title").and_then(Value::as_str).filter(|s| !s.is_empty());
        let o_args: Vec<String> = opts.get("args").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
        let is_custom = o_cmd == Some(CUSTOM_AGENT_ID) || o_launcher == Some(CUSTOM_AGENT_ID);
        let requested = if is_custom { Some(CUSTOM_AGENT_ID) } else { o_cmd };
        let (mut command, extra) = match requested {
            Some(r) => self.resolve_pty_command(r)?,
            None => (if cfg!(windows) { "cmd.exe".to_string() } else { "bash".to_string() }, Vec::new()),
        };
        let mut args = extra;
        args.extend(o_args.iter().cloned());
        let mut env: Vec<(String, String)> = std::env::vars().collect();
        let mut target_agent: Option<String> = o_cmd.filter(|c| is_agent(c)).map(str::to_string);
        let mut title = o_title.map(str::to_string).unwrap_or_else(|| match o_cmd {
            Some(c) => builtin_title(c).map(str::to_string).unwrap_or_else(|| c.to_string()),
            None => "Shell".to_string(),
        });
        let settings_custom = || sanitize_custom_agent(self.load_settings().get("customAgent"));
        if is_custom {
            if o_title.is_none() {
                title = sv(&settings_custom(), "name").to_string();
            }
        } else if let Some(lid) = o_launcher {
            if let Some(l) = read_launchers(workspace).into_iter().find(|x| x.id == lid) {
                let (cmd, extra) = self.resolve_pty_command(&l.cli)?;
                command = cmd;
                // opts args stay: --resume and --conversation arrive here, and dropping them makes every resume a new chat
                args = extra;
                args.extend(l.args.iter().cloned());
                args.extend(o_args.iter().cloned());
                env.extend(l.env.iter().cloned());
                if is_agent(&l.cli) {
                    target_agent = Some(l.cli.clone());
                }
                if o_title.is_none() {
                    title = l.name.clone();
                }
            }
        }
        let cmd_for_match = o_cmd.unwrap_or(&command).to_string();
        if !is_custom && self.is_cli_bypass() {
            apply_bypass(target_agent.as_deref().unwrap_or(&cmd_for_match), &mut args);
        }
        // Codex keeps its scrollback only without the alternate screen
        if !is_custom && (target_agent.as_deref() == Some("codex") || cmd_for_match.to_lowercase().contains("codex")) && !args.iter().any(|a| a == "--no-alt-screen") {
            args.push("--no-alt-screen".into());
        }
        // Claude Code's fullscreen TUI has no scrollback and leaves the cursor mid-screen; a launcher's own setting wins
        let is_claude = !is_custom && (target_agent.as_deref() == Some("claude") || cmd_for_match.to_lowercase().contains("claude"));
        if is_claude && !env.iter().any(|(k, _)| k == "CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN") {
            env.push(("CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN".into(), "1".into()));
        }
        // Credentials enter here and nowhere else: MCP servers inherit them, so no agent config file ever holds one
        env.extend(resolve_connection_env(self));
        let is_agy = !is_custom && (target_agent.as_deref() == Some("antigravity") || command.to_lowercase().contains("agy") || o_cmd == Some("antigravity") || o_launcher == Some("antigravity"));
        if is_agy {
            // An IDE-only id is unknown to the CLI, which would print a warning
            if let Some(i) = args.iter().position(|a| a == "--conversation") {
                if let Some(id) = args.get(i + 1) {
                    if !home().join(".gemini").join("antigravity-cli").join("conversations").join(format!("{id}.db")).exists() {
                        args.drain(i..i + 2);
                    }
                }
            }
        }
        let id = uuid();
        let cols = opts.get("cols").and_then(Value::as_u64).filter(|n| *n > 0).unwrap_or(80) as u16;
        let rows = opts.get("rows").and_then(Value::as_u64).filter(|n| *n > 0).unwrap_or(24) as u16;
        let cwd = opts
            .get("cwd")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty() && exists(c))
            .map(str::to_string)
            .or_else(|| (!workspace.is_empty() && exists(workspace)).then(|| workspace.to_string()))
            .unwrap_or_else(|| p(&home()));

        let shared = Arc::new(Shared { live: Mutex::new(Live { cols, rows, ..Default::default() }), subs: Mutex::default(), stream: Mutex::default() });
        if subscribe_owner {
            shared.subs.lock().unwrap().insert(format!("wc:{owner}"), Arc::new(WindowSub { window: owner, core: Arc::downgrade(self) }));
        }
        let serial = {
            let mut s = self.pty.serial.lock().unwrap();
            *s += 1;
            *s
        };
        let weak = Arc::downgrade(self);
        let (id2, shared2, weak2) = (id.clone(), shared.clone(), weak.clone());
        let on_data = move |bytes: &[u8]| {
            let text = shared2.stream.lock().unwrap().push(bytes);
            if text.is_empty() {
                return;
            }
            let (became_busy, port_changed) = {
                let mut l = shared2.live.lock().unwrap();
                l.last_output = Some(Instant::now());
                l.last_output_ms = now_ms();
                l.dirty.get_or_insert_with(Instant::now);
                l.tail.push_str(&text);
                if l.tail.len() > TAIL_LIMIT {
                    let mut cut = l.tail.len() - TAIL_LIMIT;
                    while !l.tail.is_char_boundary(cut) {
                        cut += 1;
                    }
                    l.tail.drain(..cut);
                }
                let became = !l.busy;
                l.busy = true;
                // The tail and not this chunk: a URL can be cut in two
                let mut start = l.tail.len().saturating_sub(4000);
                while !l.tail.is_char_boundary(start) {
                    start += 1;
                }
                let port = detect_dev_port(&l.tail[start..]);
                let changed = port.is_some() && port != l.dev_port;
                if changed {
                    l.dev_port = port;
                }
                (became, changed)
            };
            for sub in shared2.subs.lock().unwrap().values() {
                sub.data(&id2, &text);
            }
            if let Some(core) = weak2.upgrade() {
                if became_busy {
                    core.pty.emit(PtyEvent::Activity(id2.clone(), true));
                }
                if port_changed {
                    core.pty.emit(PtyEvent::Changed);
                }
            }
        };
        let (id3, shared3, weak3) = (id.clone(), shared.clone(), weak.clone());
        let on_exit = move |code: i32| {
            for sub in shared3.subs.lock().unwrap().values() {
                sub.exit(&id3, code);
            }
            if let Some(core) = weak3.upgrade() {
                core.finish_pty(&id3, serial, code);
            }
        };
        // The table lock covers spawn and insert: a process that exits at once still finds its entry
        let mut entries = self.pty.entries.lock().unwrap();
        let session = PtySession::spawn(PtyOptions { program: command, args: args.clone(), cwd: Some(cwd.clone()), env: Some(env), cols, rows, scrollback: SCREEN_SCROLLBACK, credit: None }, on_data, on_exit)?;
        let pid = session.pid().unwrap_or(0);
        if let Some(j) = self.pty.job() {
            if pid != 0 {
                let _ = j.assign(pid);
            }
        }
        let mut meta = json!({ "id": id, "command": o_cmd.or(o_launcher).unwrap_or("shell"), "startTime": now_ms() as u64, "pid": pid, "cwd": cwd, "args": args });
        if let Some(l) = o_launcher {
            meta["launcherId"] = json!(l);
        }
        if let Some(s) = opts.get("sessionId").and_then(Value::as_str) {
            meta["sessionId"] = json!(s);
        }
        entries.insert(id.clone(), Entry { session: Arc::new(session), shared, meta, title, owner, workspace: workspace.to_string(), reprints_on_resize: is_claude || is_agy, serial });
        drop(entries);
        self.pty.emit(PtyEvent::Changed);
        Ok(id)
    }

    fn finish_pty(&self, id: &str, serial: u64, code: i32) {
        let e = {
            let mut entries = self.pty.entries.lock().unwrap();
            if entries.get(id).map(|e| e.serial) != Some(serial) {
                return;
            }
            entries.remove(id).unwrap()
        };
        let killed = {
            let mut l = e.shared.live.lock().unwrap();
            l.busy = false;
            l.killed
        };
        self.pty.emit(PtyEvent::Exit { id: id.to_string(), code, title: e.title.clone(), killed });
        self.pty.emit(PtyEvent::Changed);
    }

    pub fn write_pty(&self, id: &str, data: &str) -> bool {
        let session = self.pty.entries.lock().unwrap().get(id).map(|e| e.session.clone());
        match session {
            Some(s) => {
                let _ = s.write(data.as_bytes());
                true
            }
            None => false,
        }
    }

    /// ConPTY always repaints the visible area after a resize, so clear it first. Claude Code and Antigravity also
    /// reprint the whole conversation, and ConPTY does not forward "clear scrollback", so clear that too.
    // ponytail: decided by CLI kind; if a CLI stops reprinting this clears its history, then detect the reprint instead
    pub fn resize_pty(&self, id: &str, cols: u16, rows: u16) {
        let (session, shared, reprints) = {
            let entries = self.pty.entries.lock().unwrap();
            let Some(e) = entries.get(id) else { return };
            (e.session.clone(), e.shared.clone(), e.reprints_on_resize)
        };
        {
            let mut l = shared.live.lock().unwrap();
            if cols < 2 || rows < 2 || (cols == l.cols && rows == l.rows) {
                return;
            }
            l.cols = cols;
            l.rows = rows;
            l.dirty.get_or_insert_with(Instant::now);
        }
        let prelude: &[u8] = if cfg!(windows) {
            if reprints {
                b"\x1b[H\x1b[2J\x1b[3J"
            } else {
                b"\x1b[H\x1b[2J"
            }
        } else {
            b""
        };
        let _ = session.resize_ordered(cols, rows, prelude, |bytes| {
            let subs = shared.subs.lock().unwrap();
            for sub in subs.values() {
                if !bytes.is_empty() {
                    sub.data(id, &String::from_utf8_lossy(bytes));
                }
                sub.resized(id, cols, rows);
            }
        });
    }

    fn stop(&self, e: &Entry) {
        e.shared.live.lock().unwrap().killed = true;
        // Tree first: a shell's children outlive a plain kill
        if let Some(pid) = e.session.pid() {
            wb_proc::runner::kill_tree(pid);
        }
        let _ = e.session.kill();
    }

    pub fn kill_pty(&self, id: &str) {
        let stopped = {
            let entries = self.pty.entries.lock().unwrap();
            entries.get(id).map(|e| {
                self.stop(e);
                e.serial
            })
        };
        if let Some(serial) = stopped {
            self.finish_pty(id, serial, -1);
        }
    }

    pub fn pty_cleanup_for_window(&self, window: u32) {
        let ids: Vec<(String, u64)> = {
            let entries = self.pty.entries.lock().unwrap();
            entries.iter().filter(|(_, e)| e.owner == window).map(|(id, e)| {
                self.stop(e);
                (id.clone(), e.serial)
            }).collect()
        };
        for (id, serial) in ids {
            self.finish_pty(&id, serial, -1);
        }
    }

    /// Quit: release the job so programs started from a terminal keep running, then end the terminals themselves
    pub fn pty_shutdown(&self) {
        self.pty.release_job();
        let entries = self.pty.entries.lock().unwrap();
        for e in entries.values() {
            self.stop(e);
        }
    }

    /// Current screen for a new subscriber, who then gets everything after it. None if the terminal is gone.
    pub fn subscribe_pty(&self, id: &str, key: &str, sub: Arc<dyn PtySub>) -> Option<(String, u16, u16)> {
        let (session, shared) = {
            let entries = self.pty.entries.lock().unwrap();
            let e = entries.get(id)?;
            (e.session.clone(), e.shared.clone())
        };
        Some(session.with_mirror(|m| {
            shared.subs.lock().unwrap().insert(key.to_string(), sub);
            let (cols, rows) = m.size();
            (m.serialize(SCREEN_SCROLLBACK), cols, rows)
        }))
    }

    pub fn unsubscribe_pty(&self, id: &str, key: &str) {
        if let Some(e) = self.pty.entries.lock().unwrap().get(id) {
            e.shared.subs.lock().unwrap().remove(key);
        }
    }

    fn pty_info(id: &str, e: &Entry) -> Value {
        let l = e.shared.live.lock().unwrap();
        json!({
            "id": id, "title": e.title, "command": e.meta["command"], "launcherId": e.meta.get("launcherId"),
            "workspace": e.workspace, "ownerId": e.owner, "cwd": e.meta["cwd"], "startTime": e.meta["startTime"],
            "cols": l.cols, "rows": l.rows, "needsApproval": l.needs_approval, "lastOutputAt": l.last_output_ms as u64,
            "busy": l.busy, "devPort": l.dev_port
        })
    }

    pub fn list_pty_sessions(&self) -> Vec<Value> {
        self.pty.entries.lock().unwrap().iter().map(|(id, e)| Self::pty_info(id, e)).collect()
    }

    pub fn get_pty_session(&self, id: &str) -> Option<Value> {
        self.pty.entries.lock().unwrap().get(id).map(|e| Self::pty_info(id, e))
    }

    pub fn active_session_metas(&self) -> Vec<Value> {
        self.pty.entries.lock().unwrap().values().map(|e| e.meta.clone()).collect()
    }

    /// Approval text comes from the parsed screen as it is now, not from a raw log of something already erased
    pub fn pty_screen_text(&self, id: &str) -> Option<String> {
        let session = self.pty.entries.lock().unwrap().get(id).map(|e| e.session.clone())?;
        Some(session.screen_text())
    }

    pub fn pty_call(self: &Arc<Self>, window: u32, channel: &str, args: &[Value]) -> Result<Value, String> {
        let a0 = arg_str(args, 0);
        match channel {
            "pty:launchers" => self.list_launchers(&self.workspace_for(Some(window))),
            "pty:spawn" => self.spawn_pty(args.first().unwrap_or(&Value::Null), &self.workspace_for(Some(window)), window, true).map(Value::String).map_err(|e| format!("Failed to spawn terminal: {e}")),
            "pty:attach" => {
                let sub = Arc::new(WindowSub { window, core: Arc::downgrade(self) });
                Ok(match self.subscribe_pty(a0, &format!("wc:{window}"), sub) {
                    Some((snapshot, cols, rows)) => json!({ "snapshot": snapshot, "cols": cols, "rows": rows }),
                    None => Value::Null,
                })
            }
            "pty:write" => {
                self.write_pty(a0, arg_str(args, 1));
                Ok(Value::Null)
            }
            "pty:resize" => {
                let n = |i: usize| args.get(i).and_then(Value::as_u64).unwrap_or(0) as u16;
                self.resize_pty(a0, n(1), n(2));
                Ok(Value::Null)
            }
            "pty:pipe" => {
                let text = arg_str(args, 2);
                let msg = if text.ends_with('\r') || text.ends_with('\n') { text.to_string() } else { format!("{text}\r\n") };
                Ok(json!(self.write_pty(arg_str(args, 1), &msg)))
            }
            "pty:kill" => {
                self.kill_pty(a0);
                Ok(Value::Null)
            }
            _ => Err(format!("No handler registered for '{channel}'")),
        }
    }
}

/// One thread for the delayed checks, which only ever does work for terminals with new output
pub fn start_timer(core: &Arc<Core>) {
    let weak = Arc::downgrade(core);
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(40));
        let Some(core) = weak.upgrade() else { return };
        let list: Vec<(String, Arc<PtySession>, Arc<Shared>)> = core.pty.entries.lock().unwrap().iter().map(|(id, e)| (id.clone(), e.session.clone(), e.shared.clone())).collect();
        for (id, session, shared) in list {
            let due = {
                let mut l = shared.live.lock().unwrap();
                let idle = l.busy && l.last_output.map(|t| t.elapsed() >= BUSY_IDLE).unwrap_or(false);
                if idle {
                    l.busy = false;
                }
                let check = l.dirty.map(|t| t.elapsed() >= APPROVAL_DELAY).unwrap_or(false);
                if check {
                    l.dirty = None;
                }
                (idle, check)
            };
            // Busy or idle is decided here and broadcast only on a flip, so a phone never compares its own clock
            if due.0 {
                core.pty.emit(PtyEvent::Activity(id.clone(), false));
            }
            if due.1 {
                let waiting = looks_like_approval_prompt(&session.screen_text());
                let changed = {
                    let mut l = shared.live.lock().unwrap();
                    let c = l.needs_approval != waiting;
                    l.needs_approval = waiting;
                    c
                };
                if changed {
                    if waiting {
                        core.pty.emit(PtyEvent::Approval(id.clone()));
                    }
                    core.pty.emit(PtyEvent::Changed);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_split_across_chunks() {
        let mut s = Utf8Stream::default();
        let bytes = "中文".as_bytes();
        assert_eq!(s.push(&bytes[..2]), "");
        assert_eq!(s.push(&bytes[2..4]), "中");
        assert_eq!(s.push(&bytes[4..]), "文");
        assert_eq!(s.push(b"a\xffb"), "a\u{fffd}b");
    }

    #[test]
    fn dev_port() {
        assert_eq!(detect_dev_port("Local: \x1b[36mhttp://localhost:5173/\x1b[0m"), Some(5173));
        assert_eq!(detect_dev_port("http://127.0.0.1:80/ http://localhost:3000"), Some(3000));
        assert_eq!(detect_dev_port("https://example.com:8080"), None);
    }

    #[test]
    fn repo_yaml_cannot_set_injection_env() {
        assert!(blocked_env("node_options"));
        assert!(blocked_env("DYLD_INSERT_LIBRARIES"));
        assert!(!blocked_env("MY_FLAG"));
    }

    #[test]
    fn bypass_is_added_once() {
        let mut a = vec!["--permission-mode".to_string(), "plan".to_string()];
        apply_bypass("claude", &mut a);
        assert_eq!(a.len(), 2);
        let mut b = Vec::new();
        apply_bypass("codex", &mut b);
        assert_eq!(b, vec!["--dangerously-bypass-approvals-and-sandbox"]);
    }
}
