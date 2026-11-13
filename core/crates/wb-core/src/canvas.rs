//! Cowork 2.0 canvas: channels, node lifecycle, approvals, handoffs and stop (cowork2.0-implement.md §3).
//! One manager per Git common dir serializes every change; windows and the phone send operations, never whole
//! channels. The backend owns processes: nothing a view does (open, hide, close a window) ends a node's PTY.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use wb_git as g;

use crate::core::Core;
use crate::pty::PtyEvent;
use crate::util::{js_prefix, now_ms, p, read_json_with_fallback, real_path, uuid, write_json_atomic};
use crate::worktree::{self as wt, NodeWorktree};

mod team;
pub use team::{mcp_call, mcp_request, tool_names};

pub const UPDATE_EVENT: &str = "canvas:update";
const MAX_EVENTS: usize = 500;
const MAX_TEXT: usize = 16 * 1024;
const MAX_NODES: usize = 32;
const MAX_CHANNELS: usize = 64;
const APPROVAL_TTL_MS: f64 = 30.0 * 60_000.0;
const EXIT_WAIT: Duration = Duration::from_secs(5);
const BUILTIN: [&str; 3] = ["claude", "codex", "antigravity"];

// ---------- errors ----------

#[derive(Debug)]
pub struct CanvasError {
    pub code: &'static str,
    pub message: String,
}
type R<T> = Result<T, CanvasError>;

fn err(code: &'static str, message: impl Into<String>) -> CanvasError {
    CanvasError { code, message: message.into() }
}

/// worktree.rs reports plain strings; the UI branches on a few of them
fn git_err(e: String) -> CanvasError {
    let code = if e == "dirty" || e.contains("ignored") || e.contains("index flags") {
        "dirty"
    } else if e.starts_with("conflict") {
        "conflict"
    } else if e.starts_with("stale") || e.contains("moved") {
        "stale-preview"
    } else if e.contains("ownership") || e.contains("not registered") || e.contains("marker") {
        "not-owned"
    } else {
        "invalid"
    };
    err(code, e)
}

pub fn wrap(r: R<Value>) -> Value {
    match r {
        Ok(data) => json!({ "ok": true, "data": data }),
        Err(e) => json!({ "ok": false, "code": e.code, "message": e.message }),
    }
}

// ---------- persisted shapes (src/shared/canvas.ts) ----------

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
struct Pos {
    x: f64,
    y: f64,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
struct Size {
    width: f64,
    height: f64,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Node {
    id: String,
    agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    worktree: Option<NodeWorktree>,
    position: Pos,
    size: Size,
}

#[derive(Serialize, Deserialize, Clone)]
struct Edge {
    id: String,
    from: String,
    to: String,
    at: f64,
    text: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Channel {
    schema_version: u32,
    id: String,
    name: String,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    events: Vec<Value>,
    /// Teams of nodes working together (cowork-team-implement.md); missing in files from before teams
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    teams: Vec<team::Team>,
    /// What each node is working on, for the Feed cards; keyed by node id
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    tasks: std::collections::BTreeMap<String, team::NodeTask>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn short_id() -> String {
    uuid().replace('-', "")[..10].to_string()
}

fn pos_ok(p: Pos) -> bool {
    p.x.is_finite() && p.y.is_finite() && p.x.abs() <= 1e6 && p.y.abs() <= 1e6
}

fn size_ok(s: Size) -> bool {
    s.width.is_finite() && s.height.is_finite() && (200.0..=4000.0).contains(&s.width) && (120.0..=3000.0).contains(&s.height)
}

/// Structural check for a loaded file; anything else is treated as damaged and never overwritten
fn channel_ok(c: &Channel) -> bool {
    let ids: HashSet<&str> = c.nodes.iter().map(|n| n.id.as_str()).collect();
    c.schema_version == 1
        && valid_id(&c.id)
        && c.name.chars().count() <= 80
        && c.nodes.len() <= MAX_NODES
        && ids.len() == c.nodes.len()
        && c.nodes.iter().all(|n| valid_id(&n.id) && !n.agent.is_empty() && n.agent.len() <= 128 && pos_ok(n.position) && size_ok(n.size))
        && c.edges.iter().all(|e| valid_id(&e.id) && ids.contains(e.from.as_str()) && ids.contains(e.to.as_str()) && e.text.len() <= MAX_TEXT)
        && c.events.len() <= MAX_EVENTS
        && team::teams_ok(c)
}

// ---------- runtime ----------

pub enum HookDecision {
    Allow,
    Deny(String),
    Pass,
}

struct Approval {
    request_id: String,
    generation: String,
    source: &'static str,
    created_at: f64,
    expires_at: f64,
    title: String,
    tail: String,
    can_approve: bool,
    can_reject: bool,
    fingerprint: String,
    hook: Option<Value>,
    reply: Option<mpsc::Sender<HookDecision>>,
}

#[derive(Default)]
struct NodeRt {
    status: &'static str,
    error: Option<String>,
    pty_id: Option<String>,
    generation: Option<String>,
    exit_code: Option<i32>,
    approval: Option<Approval>,
    proxy_mode: Option<Value>,
    /// Phase 6: the token wb-hook presents for this session
    hook_token: Option<String>,
}

#[derive(Default)]
struct RepoState {
    channels: HashMap<String, Channel>,
    loaded: bool,
    rt: HashMap<String, NodeRt>,
    stop: Option<f64>,
    merges: HashMap<String, (String, String, wt::MergeToken)>,
    discards: HashMap<String, (String, String, wt::DiscardToken)>,
    /// Team messages per channel, saved next to the channel file
    msgs: HashMap<String, Vec<team::Msg>>,
    /// channel/node -> (last notice, notices for the same unread mail)
    nudged: HashMap<String, (f64, u8)>,
    /// A lead's merge requests waiting for the user
    merge_reqs: HashMap<String, team::MergeReq>,
}

struct Repo {
    root: PathBuf,
    common: PathBuf,
    id: String,
    st: Mutex<RepoState>,
}

#[derive(Default)]
pub struct CanvasApp {
    repos: Mutex<HashMap<String, Arc<Repo>>>,
    /// pty id -> (repo, channel, node)
    ptys: Mutex<HashMap<String, (Arc<Repo>, String, String)>>,
    /// hook token -> pty id
    hooks: Mutex<HashMap<String, String>>,
}

fn key(channel: &str, node: &str) -> String {
    format!("{channel}/{node}")
}

fn app(core: &Core) -> R<&CanvasApp> {
    core.features.get().map(|f| &f.canvas).ok_or_else(|| err("invalid", "app is starting"))
}

impl Repo {
    fn dir(&self) -> PathBuf {
        self.common.join("cowork").join("channels")
    }

    fn file(&self, id: &str) -> PathBuf {
        self.dir().join(format!("{id}.json"))
    }

    fn load(&self, st: &mut RepoState) {
        if st.loaded {
            return;
        }
        st.loaded = true;
        let Ok(rd) = std::fs::read_dir(self.dir()) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            // "<id>.messages.json" and "<id>.prev.json" are not channels: the id check rejects the dot
            let Some(id) = name.strip_suffix(".json").filter(|i| valid_id(i)) else { continue };
            let ok = |v: &Value| serde_json::from_value::<Channel>(v.clone()).map(|c| c.id == id && channel_ok(&c)).unwrap_or(false);
            // Unknown schema or damage on both copies: left on disk untouched, not shown
            if let Some((v, _)) = read_json_with_fallback(&e.path(), &ok) {
                if let Ok(c) = serde_json::from_value::<Channel>(v) {
                    st.channels.insert(c.id.clone(), c);
                }
            }
        }
        let ids: Vec<String> = st.channels.keys().cloned().collect();
        for id in ids {
            let m = team::load_messages(self, &id);
            st.msgs.insert(id, m);
        }
        // No process survives an app restart: every node starts as ended, and a lost worktree shows as an error
        for c in st.channels.values() {
            for n in &c.nodes {
                let rt = match &n.worktree {
                    Some(w) if !Path::new(&w.path).exists() => NodeRt { status: "error", error: Some("worktree folder is missing".into()), ..Default::default() },
                    Some(_) => NodeRt { status: "pending-cleanup", ..Default::default() },
                    None => NodeRt { status: "exited", ..Default::default() },
                };
                st.rt.insert(key(&c.id, &n.id), rt);
            }
        }
    }

    fn save(&self, c: &Channel) -> R<()> {
        std::fs::create_dir_all(self.dir()).map_err(|e| err("invalid", e.to_string()))?;
        write_json_atomic(&self.file(&c.id), &serde_json::to_value(c).unwrap()).map_err(|e| err("invalid", e))
    }
}

fn push_event(c: &mut Channel, ev: Value) {
    c.events.push(ev);
    if c.events.len() > MAX_EVENTS {
        let cut = c.events.len() - MAX_EVENTS;
        c.events.drain(..cut);
    }
}

fn repo_for(core: &Core, ws: &str) -> R<Arc<Repo>> {
    if ws.is_empty() {
        return Err(err("invalid", "Open a folder first"));
    }
    let dir = Path::new(ws);
    let root = g::git(dir, &["rev-parse", "--show-toplevel"]).map_err(|_| err("invalid", "Cowork needs a Git repository"))?;
    let root = PathBuf::from(root.trim());
    let common = g::git(dir, &["rev-parse", "--git-common-dir"]).map_err(|e| err("invalid", e))?;
    let common = dir.join(common.trim());
    let k = real_path(&p(&common)).replace('\\', "/").to_lowercase();
    let app = app(core)?;
    let mut repos = app.repos.lock().unwrap();
    Ok(repos.entry(k).or_insert_with(|| Arc::new(Repo { id: crate::util::repo_id_of(&p(&common)), root, common, st: Mutex::default() })).clone())
}

// ---------- views ----------

fn approval_view(a: &Approval) -> Value {
    json!({
        "requestId": a.request_id, "sessionGeneration": a.generation, "source": a.source, "createdAt": a.created_at, "expiresAt": a.expires_at,
        "title": a.title, "tail": a.tail, "canApprove": a.can_approve, "canReject": a.can_reject, "hook": a.hook,
    })
}

fn rt_view(core: &Core, rt: &NodeRt, stopped: bool) -> Value {
    let info = rt.pty_id.as_deref().and_then(|id| core.get_pty_session(id));
    let busy = info.as_ref().is_some_and(|i| i["busy"] == json!(true));
    let waiting = info.as_ref().is_some_and(|i| i["needsApproval"] == json!(true));
    let ready = rt.status == "running" && info.is_some() && !busy && !waiting && rt.approval.is_none() && !stopped;
    json!({
        "status": rt.status, "error": rt.error, "ptyId": rt.pty_id, "sessionGeneration": rt.generation, "busy": busy, "exitCode": rt.exit_code,
        "inputReady": ready, "approval": rt.approval.as_ref().map(approval_view), "proxyMode": rt.proxy_mode.as_ref().and_then(|m| m.get("mode")).cloned(),
    })
}

fn view(core: &Core, st: &RepoState, channel: &str) -> Option<Value> {
    let c = st.channels.get(channel)?;
    let mut runtime = serde_json::Map::new();
    for n in &c.nodes {
        if let Some(rt) = st.rt.get(&key(channel, &n.id)) {
            runtime.insert(n.id.clone(), rt_view(core, rt, st.stop.is_some()));
        }
    }
    let mut v = json!({ "channel": c, "runtime": runtime, "stop": stop_view(st) });
    team::view_extra(st, c, &mut v);
    Some(v)
}

fn stop_view(st: &RepoState) -> Value {
    json!({ "requested": st.stop.is_some(), "at": st.stop })
}

/// Tell every window and the phone; called after the repo lock is released
fn emit(core: &Core, repo: &Repo, channel: &str) {
    let v = {
        let st = repo.st.lock().unwrap();
        view(core, &st, channel)
    };
    let update = json!({ "repoId": repo.id, "channelId": channel, "view": v });
    core.host.send_all(UPDATE_EVENT, json!([update]));
    crate::remote::on_canvas_update(core, &update);
}

fn emit_all(core: &Core, repo: &Repo) {
    let ids: Vec<String> = repo.st.lock().unwrap().channels.keys().cloned().collect();
    for id in ids {
        emit(core, repo, &id);
    }
}

// ---------- approvals ----------

/// Ends a pending request: a hook gets "hand back", a screen card just goes away. Never an approval.
fn expire(c: Option<&mut Channel>, node: &str, rt: &mut NodeRt) {
    if let Some(a) = rt.approval.take() {
        if let Some(tx) = a.reply {
            let _ = tx.send(HookDecision::Pass);
        }
        if let Some(c) = c {
            push_event(c, json!({ "t": "approval", "at": now_ms(), "node": node, "requestId": a.request_id, "status": "expired" }));
        }
    }
}

fn screen_tail(screen: &str) -> String {
    let lines: Vec<&str> = screen.lines().collect();
    let start = lines.len().saturating_sub(24);
    js_prefix(&lines[start..].join("\n"), 3000)
}

fn on_screen_approval(core: &Core, pty: &str) {
    let Ok(app) = app(core) else { return };
    let Some((repo, channel, node)) = app.ptys.lock().unwrap().get(pty).cloned() else { return };
    {
        let mut st = repo.st.lock().unwrap();
        let st = &mut *st;
        let Some(rt) = st.rt.get_mut(&key(&channel, &node)) else { return };
        if rt.approval.is_some() || rt.pty_id.as_deref() != Some(pty) {
            return;
        }
        let Some(generation) = rt.generation.clone() else { return };
        let screen = core.pty_screen_text(pty).unwrap_or_default();
        let parsed = wb_pty::approval::parse_approval_prompt(&screen);
        let now = now_ms();
        let a = Approval {
            request_id: uuid(),
            generation,
            source: "screen",
            created_at: now,
            expires_at: now + APPROVAL_TTL_MS,
            title: parsed.as_ref().map(|p| p.title.clone()).unwrap_or_else(|| "Waiting for an answer".into()),
            tail: screen_tail(&screen),
            can_approve: parsed.as_ref().is_some_and(|p| p.can_approve),
            can_reject: parsed.as_ref().is_some_and(|p| p.can_reject),
            fingerprint: parsed.map(|p| p.fingerprint).unwrap_or_default(),
            hook: None,
            reply: None,
        };
        if let Some(c) = st.channels.get_mut(&channel) {
            push_event(c, json!({ "t": "approval", "at": now, "node": node, "requestId": a.request_id, "status": "pending" }));
            let _ = repo.save(c);
        }
        rt.approval = Some(a);
    }
    emit(core, &repo, &channel);
}

/// The prompt left the screen (answered in the terminal, or the CLI moved on): the card is no longer valid
fn recheck_screen_approvals(core: &Core) {
    let Ok(app) = app(core) else { return };
    let list: Vec<(String, Arc<Repo>, String, String)> = app.ptys.lock().unwrap().iter().map(|(p, (r, c, n))| (p.clone(), r.clone(), c.clone(), n.clone())).collect();
    for (pty, repo, channel, node) in list {
        let gone = {
            let mut st = repo.st.lock().unwrap();
            let st = &mut *st;
            let Some(rt) = st.rt.get_mut(&key(&channel, &node)) else { continue };
            let pending = rt.approval.as_ref().is_some_and(|a| a.source == "screen");
            if !pending || core.get_pty_session(&pty).is_some_and(|i| i["needsApproval"] == json!(true)) {
                false
            } else {
                expire(st.channels.get_mut(&channel), &node, rt);
                true
            }
        };
        if gone {
            emit(core, &repo, &channel);
        }
    }
}

fn on_exit(core: &Core, pty: &str, code: i32) {
    let Ok(app) = app(core) else { return };
    let Some((repo, channel, node)) = app.ptys.lock().unwrap().remove(pty) else { return };
    app.hooks.lock().unwrap().retain(|_, p| p != pty);
    {
        let mut st = repo.st.lock().unwrap();
        let st = &mut *st;
        let has_wt = st.channels.get(&channel).and_then(|c| c.nodes.iter().find(|n| n.id == node)).is_some_and(|n| n.worktree.is_some());
        if let Some(rt) = st.rt.get_mut(&key(&channel, &node)) {
            if rt.pty_id.as_deref() == Some(pty) {
                expire(st.channels.get_mut(&channel), &node, rt);
                rt.pty_id = None;
                rt.generation = None;
                rt.hook_token = None;
                rt.exit_code = Some(code);
                rt.status = if has_wt { "pending-cleanup" } else { "exited" };
            }
        }
        if let Some(c) = st.channels.get_mut(&channel) {
            push_event(c, json!({ "t": "node-exited", "at": now_ms(), "node": node }));
            let _ = repo.save(c);
        }
    }
    emit(core, &repo, &channel);
}

/// Wire the PTY events in; called once from start_features
pub fn start(core: &Arc<Core>) {
    let weak = Arc::downgrade(core);
    core.pty.on_event(Arc::new(move |ev| {
        let Some(core) = weak.upgrade() else { return };
        // PTY events arrive under no lock of ours; handle them off the PTY thread so a slow save never stalls output
        let ev = ev.clone();
        std::thread::spawn(move || match ev {
            PtyEvent::Approval(id) => on_screen_approval(&core, &id),
            PtyEvent::Exit { id, code, .. } => on_exit(&core, &id, code),
            PtyEvent::Activity(id, busy) => {
                let owned = app(&core).ok().and_then(|a| a.ptys.lock().unwrap().get(&id).cloned());
                if let Some((repo, channel, _)) = owned {
                    recheck_screen_approvals(&core);
                    emit(&core, &repo, &channel);
                    // A CLI that just went idle may have team mail waiting
                    if !busy {
                        team::deliver(&core, &repo, &channel);
                    }
                }
            }
            PtyEvent::Changed => recheck_screen_approvals(&core),
        });
    }));
}

// ---------- process ----------

fn agy_trusts(root: &Path) -> bool {
    let file = crate::util::home().join(".gemini").join("antigravity-cli").join("settings.json");
    let Some(v) = crate::util::read_text(&file).and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { return false };
    let norm = |s: &str| real_path(s).replace('\\', "/").trim_end_matches('/').to_lowercase();
    let want = norm(&p(root));
    v["trustedWorkspaces"].as_array().into_iter().flatten().filter_map(Value::as_str).any(|t| norm(t) == want)
}

/// Spawns the node's CLI and records the new session. Caller holds no repo lock.
fn launch(core: &Arc<Core>, repo: &Arc<Repo>, channel: &str, node: &Node) -> R<()> {
    let cwd = node.worktree.as_ref().map(|w| w.path.clone()).unwrap_or_else(|| p(&repo.root));
    let builtin = BUILTIN.contains(&node.agent.as_str()) || node.agent == crate::custom_agent::CUSTOM_AGENT_ID;
    let mut opts = json!({ "cwd": cwd, "cols": 120, "rows": 32, "title": format!("{} · {}", node.agent, node.id) });
    opts[if builtin { "command" } else { "launcherId" }] = json!(node.agent);
    let mut args: Vec<String> = Vec::new();
    // agy asks again in every worktree; only carry over a trust the user already gave the repo (Phase 0, Q3)
    if node.agent == "antigravity" && node.worktree.is_some() && agy_trusts(&repo.root) {
        args.extend(["--settings".to_string(), json!({ "trustedWorkspaces": [cwd] }).to_string()]);
    }
    let generation = uuid();
    let mount = crate::approval_hook::mount(core, &node.agent);
    args.extend(mount.args);
    opts["args"] = json!(args);
    let pty = core.spawn_pty_env(&opts, &p(&repo.root), 0, false, mount.env).map_err(|e| err("invalid", e))?;
    let proxy = core.get_pty_session(&pty).and_then(|i| i.get("proxy").cloned());
    let app = app(core)?;
    app.ptys.lock().unwrap().insert(pty.clone(), (repo.clone(), channel.to_string(), node.id.clone()));
    if let Some(t) = &mount.token {
        app.hooks.lock().unwrap().insert(t.clone(), pty.clone());
    }
    let mut st = repo.st.lock().unwrap();
    let st = &mut *st;
    let rt = st.rt.entry(key(channel, &node.id)).or_default();
    *rt = NodeRt { status: "running", pty_id: Some(pty.clone()), generation: Some(generation), proxy_mode: proxy, hook_token: mount.token, ..Default::default() };
    if let Some(c) = st.channels.get_mut(channel) {
        push_event(c, json!({ "t": "node-started", "at": now_ms(), "node": node.id }));
        let _ = repo.save(c);
    }
    Ok(())
}

fn wait_gone(core: &Core, pty: &str) -> bool {
    let until = Instant::now() + EXIT_WAIT;
    while Instant::now() < until {
        if core.get_pty_session(pty).is_none() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    core.get_pty_session(pty).is_none()
}

// ---------- operations ----------

#[derive(Clone, Copy, PartialEq)]
enum Origin {
    Desktop,
    Phone,
}

fn s<'a>(req: &'a Value, k: &str) -> &'a str {
    req.get(k).and_then(Value::as_str).unwrap_or("")
}

fn find_node(st: &RepoState, channel: &str, node: &str) -> R<Node> {
    let c = st.channels.get(channel).ok_or_else(|| err("not-found", "channel"))?;
    c.nodes.iter().find(|n| n.id == node).cloned().ok_or_else(|| err("not-found", "node"))
}

fn status_of(st: &RepoState, channel: &str, node: &str) -> &'static str {
    st.rt.get(&key(channel, node)).map(|r| r.status).unwrap_or("exited")
}

fn require_ended(st: &RepoState, channel: &str, node: &str) -> R<()> {
    match status_of(st, channel, node) {
        "running" | "creating" | "closing" => Err(err("busy", "End the node's process first")),
        _ => Ok(()),
    }
}

fn current_branch(repo: &Repo) -> R<String> {
    g::git(&repo.root, &["symbolic-ref", "--short", "HEAD"]).map(|s| s.trim().to_string()).map_err(|_| err("invalid", "The repository is not on a branch"))
}

fn sanitize_text(text: &str, keep_newlines: bool) -> String {
    let t = text.replace("\r\n", "\n").replace('\r', "\n");
    t.chars().filter(|c| (keep_newlines && (*c == '\n' || *c == '\t')) || !(c.is_control() || ('\u{80}'..='\u{9f}').contains(c))).collect()
}

fn handoff_text(repo: &Repo, from: &Node, note: &str) -> String {
    let mut out = vec![format!("Handoff from {} node {}", from.agent, from.id)];
    let dir = from.worktree.as_ref().map(|w| PathBuf::from(&w.path)).unwrap_or_else(|| repo.root.clone());
    match &from.worktree {
        Some(w) => {
            out.push(format!("Worktree: {}", w.path));
            out.push(format!("Branch: {} (base {})", w.branch, &w.base_commit[..w.base_commit.len().min(12)]));
            let range = format!("{}..HEAD", w.base_commit);
            let commits = g::git(&dir, &["log", "--format=%h %s", "-n", "50", &range]).unwrap_or_default();
            out.push(if commits.trim().is_empty() { "Commits since base: none".into() } else { format!("Commits since base:\n{}", commits.trim_end()) });
            let files = g::git(&dir, &["diff", "--name-status", "--no-renames", &w.base_commit, "HEAD"]).unwrap_or_default();
            if !files.trim().is_empty() {
                out.push(format!("Changed files (committed):\n{}", files.trim_end()));
            }
        }
        None => out.push(format!("Works in the repository root: {}", p(&repo.root))),
    }
    let dirty = g::git(&dir, &["status", "--porcelain", "-uall"]).map(|s| s.lines().count()).unwrap_or(0);
    out.push(if dirty == 0 {
        "Uncommitted changes: none".into()
    } else {
        format!("Uncommitted changes: {dirty} file(s), not part of this handoff; commit and merge them to share the code")
    });
    let note = sanitize_text(note, true);
    if !note.trim().is_empty() {
        out.push(format!("Note: {}", note.trim()));
    }
    // Commit subjects and file names come from another agent: no control characters reach a draft
    js_prefix(&sanitize_text(&out.join("\n"), true), MAX_TEXT)
}

fn dispatch(core: &Arc<Core>, ws: &str, op: &str, req: &Value, origin: Origin) -> R<Value> {
    let repo = repo_for(core, ws)?;
    repo.load(&mut repo.st.lock().unwrap());
    let (ch, nd) = (s(req, "channelId").to_string(), s(req, "nodeId").to_string());
    match op {
        "channels.list" => {
            let st = repo.st.lock().unwrap();
            let mut list: Vec<Value> = st
                .channels
                .values()
                .map(|c| {
                    let pending = c.nodes.iter().filter(|n| st.rt.get(&key(&c.id, &n.id)).is_some_and(|r| r.approval.is_some())).count();
                    let updated = c.events.last().and_then(|e| e["at"].as_f64()).unwrap_or(0.0);
                    json!({ "id": c.id, "name": c.name, "nodeCount": c.nodes.len(), "pendingApprovals": pending, "updatedAt": updated })
                })
                .collect();
            list.sort_by(|a, b| b["updatedAt"].as_f64().partial_cmp(&a["updatedAt"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
            Ok(json!(list))
        }
        "channel.get" => view(core, &repo.st.lock().unwrap(), &ch).ok_or_else(|| err("not-found", "channel")),
        "channel.create" => {
            let name = sanitize_text(s(req, "name"), false);
            let name = js_prefix(name.trim(), 80);
            let id = short_id();
            {
                let mut st = repo.st.lock().unwrap();
                if st.channels.len() >= MAX_CHANNELS {
                    return Err(err("invalid", "Too many channels"));
                }
                let c = Channel { schema_version: 1, id: id.clone(), name: if name.is_empty() { "general".into() } else { name }, nodes: vec![], edges: vec![], events: vec![], teams: vec![], tasks: Default::default() };
                repo.save(&c)?;
                st.channels.insert(id.clone(), c);
            }
            emit(core, &repo, &id);
            view(core, &repo.st.lock().unwrap(), &id).ok_or_else(|| err("not-found", "channel"))
        }
        "channel.rename" => {
            let name = js_prefix(sanitize_text(s(req, "name"), false).trim(), 80);
            if name.is_empty() {
                return Err(err("invalid", "Empty name"));
            }
            {
                let mut st = repo.st.lock().unwrap();
                let c = st.channels.get_mut(&ch).ok_or_else(|| err("not-found", "channel"))?;
                c.name = name;
                repo.save(c)?;
            }
            emit(core, &repo, &ch);
            Ok(Value::Null)
        }
        "channel.delete" => {
            {
                let mut st = repo.st.lock().unwrap();
                let c = st.channels.get(&ch).ok_or_else(|| err("not-found", "channel"))?;
                if !c.nodes.is_empty() {
                    return Err(err("busy", "Close and clean up every node first"));
                }
                std::fs::remove_file(repo.file(&ch)).map_err(|e| err("invalid", e.to_string()))?;
                let _ = std::fs::remove_file(crate::util::prev_path(&repo.file(&ch)));
                st.channels.remove(&ch);
            }
            let update = json!({ "repoId": repo.id, "channelId": ch });
            core.host.send_all(UPDATE_EVENT, json!([update]));
            crate::remote::on_canvas_update(core, &update);
            Ok(Value::Null)
        }
        "node.create" => {
            let agent = s(req, "agent").to_string();
            let position: Pos = serde_json::from_value(req["position"].clone()).map_err(|_| err("invalid", "position"))?;
            let size: Size = serde_json::from_value(req["size"].clone()).map_err(|_| err("invalid", "size"))?;
            if agent.is_empty() || agent.len() > 128 || !pos_ok(position) || !size_ok(size) {
                return Err(err("invalid", "node"));
            }
            let in_root = s(req, "workspace") == "root";
            let id = short_id();
            {
                let mut st = repo.st.lock().unwrap();
                if st.stop.is_some() {
                    return Err(err("stopped", "Stopped: clear the stop first"));
                }
                // One root-directory node per repo, across every channel and window: nothing else isolates them
                if in_root && st.channels.values().flat_map(|c| c.nodes.iter()).any(|n| n.worktree.is_none()) {
                    return Err(err("root-node-exists", "This repository already has a node working in its root"));
                }
                let c = st.channels.get_mut(&ch).ok_or_else(|| err("not-found", "channel"))?;
                if c.nodes.len() >= MAX_NODES {
                    return Err(err("invalid", "Too many nodes"));
                }
                // The record comes first, so a failure below still leaves something to inspect and retry
                c.nodes.push(Node { id: id.clone(), agent: agent.clone(), worktree: None, position, size });
                repo.save(c)?;
                st.rt.insert(key(&ch, &id), NodeRt { status: "creating", ..Default::default() });
            }
            emit(core, &repo, &ch);
            let fail = |e: CanvasError| {
                let mut st = repo.st.lock().unwrap();
                if let Some(rt) = st.rt.get_mut(&key(&ch, &id)) {
                    rt.status = "error";
                    rt.error = Some(e.message.clone());
                }
                e
            };
            let result = (|| {
                if !in_root {
                    let custom = core.load_settings().pointer("/cowork/worktreeRoot").and_then(Value::as_str).map(str::to_string).filter(|s| !s.is_empty());
                    let w = wt::create_node(&repo.root, &ch, &id, custom.as_deref()).map_err(git_err)?;
                    let mut st = repo.st.lock().unwrap();
                    let c = st.channels.get_mut(&ch).ok_or_else(|| err("not-found", "channel"))?;
                    if let Some(n) = c.nodes.iter_mut().find(|n| n.id == id) {
                        n.worktree = Some(w);
                    }
                    repo.save(c)?;
                }
                let node = find_node(&repo.st.lock().unwrap(), &ch, &id)?;
                launch(core, &repo, &ch, &node)?;
                Ok(serde_json::to_value(&node).unwrap())
            })();
            let out = result.map_err(fail);
            emit(core, &repo, &ch);
            out
        }
        "node.layout" => {
            let position: Pos = serde_json::from_value(req["position"].clone()).map_err(|_| err("invalid", "position"))?;
            let size: Size = serde_json::from_value(req["size"].clone()).map_err(|_| err("invalid", "size"))?;
            if !pos_ok(position) || !size_ok(size) {
                return Err(err("invalid", "layout"));
            }
            {
                let mut st = repo.st.lock().unwrap();
                let c = st.channels.get_mut(&ch).ok_or_else(|| err("not-found", "channel"))?;
                let n = c.nodes.iter_mut().find(|n| n.id == nd).ok_or_else(|| err("not-found", "node"))?;
                if n.position == position && n.size == size {
                    return Ok(Value::Null);
                }
                n.position = position;
                n.size = size;
                repo.save(c)?;
            }
            emit(core, &repo, &ch);
            Ok(Value::Null)
        }
        "node.restart" => {
            let node = {
                let st = repo.st.lock().unwrap();
                if st.stop.is_some() {
                    return Err(err("stopped", "Stopped: clear the stop first"));
                }
                require_ended(&st, &ch, &nd)?;
                find_node(&st, &ch, &nd)?
            };
            // Never recreate a lost worktree at the same path: that would hide the loss
            if let Some(w) = &node.worktree {
                if !Path::new(&w.path).exists() {
                    return Err(err("not-found", "The node's worktree folder is missing"));
                }
            }
            repo.st.lock().unwrap().rt.entry(key(&ch, &nd)).or_default().status = "creating";
            let r = launch(core, &repo, &ch, &node);
            if let Err(e) = &r {
                let mut st = repo.st.lock().unwrap();
                let rt = st.rt.entry(key(&ch, &nd)).or_default();
                rt.status = "error";
                rt.error = Some(e.message.clone());
            }
            emit(core, &repo, &ch);
            r.map(|_| Value::Null)
        }
        "node.close" => {
            let pty = {
                let mut st = repo.st.lock().unwrap();
                let st = &mut *st;
                find_node(st, &ch, &nd)?;
                let rt = st.rt.entry(key(&ch, &nd)).or_default();
                if rt.status != "running" {
                    return Ok(Value::Null);
                }
                // Approvals first, so nothing can be answered on a process that is going away
                expire(st.channels.get_mut(&ch), &nd, rt);
                rt.status = "closing";
                rt.pty_id.clone()
            };
            emit(core, &repo, &ch);
            if let Some(pty) = pty {
                core.kill_pty(&pty);
                if !wait_gone(core, &pty) {
                    let mut st = repo.st.lock().unwrap();
                    let rt = st.rt.entry(key(&ch, &nd)).or_default();
                    rt.status = "error";
                    rt.error = Some("The process did not confirm its exit; its worktree is kept".into());
                    drop(st);
                    emit(core, &repo, &ch);
                    return Err(err("busy", "The process did not confirm its exit"));
                }
            }
            Ok(Value::Null)
        }
        "node.changes" => {
            let node = find_node(&repo.st.lock().unwrap(), &ch, &nd)?;
            let w = node.worktree.ok_or_else(|| err("invalid", "This node works in the repository root"))?;
            let sum = wt::changes_summary(&repo.root, &w, 256 * 1024).map_err(git_err)?;
            let clean = sum.staged.is_empty() && sum.unstaged.is_empty() && sum.untracked.is_empty() && !sum.commits.is_empty();
            let mut v = serde_json::to_value(sum).unwrap();
            v["baseCommit"] = json!(w.base_commit);
            v["clean"] = json!(clean);
            Ok(v)
        }
        "node.commitAll" => {
            let node = {
                let st = repo.st.lock().unwrap();
                require_ended(&st, &ch, &nd)?;
                find_node(&st, &ch, &nd)?
            };
            let w = node.worktree.ok_or_else(|| err("invalid", "This node works in the repository root"))?;
            let files: Vec<String> = req["files"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            let message = s(req, "message").trim().to_string();
            let commit = wt::commit_all(&repo.root, &w, &files, &message).map_err(git_err)?;
            emit(core, &repo, &ch);
            Ok(json!({ "commit": commit }))
        }
        "node.mergePreview" => {
            let node = {
                let st = repo.st.lock().unwrap();
                require_ended(&st, &ch, &nd)?;
                find_node(&st, &ch, &nd)?
            };
            let w = node.worktree.ok_or_else(|| err("invalid", "This node works in the repository root"))?;
            let target = s(req, "target");
            let t = wt::merge_preview(&repo.root, &w, target).map_err(git_err)?;
            let sum = wt::changes_summary(&repo.root, &w, 0).map_err(git_err)?;
            let token = uuid();
            let out = json!({
                "token": token, "source": { "branch": w.branch, "commit": t.source }, "target": { "branch": t.target_branch, "head": t.target_head },
                "commits": sum.commits, "files": sum.committed.iter().map(|f| json!({ "path": f.path, "status": f.status })).collect::<Vec<_>>(),
            });
            repo.st.lock().unwrap().merges.insert(token, (ch.clone(), nd.clone(), t));
            Ok(out)
        }
        "node.merge" => {
            let token = s(req, "token");
            let (c, n, t) = repo.st.lock().unwrap().merges.remove(token).ok_or_else(|| err("stale-preview", "Preview again"))?;
            if c != ch || n != nd {
                return Err(err("stale-preview", "Preview again"));
            }
            require_ended(&repo.st.lock().unwrap(), &ch, &nd)?;
            let commit = wt::merge(&repo.root, &t).map_err(git_err)?;
            {
                let mut st = repo.st.lock().unwrap();
                if let Some(c) = st.channels.get_mut(&ch) {
                    push_event(c, json!({ "t": "merged", "at": now_ms(), "node": nd }));
                    repo.save(c)?;
                }
            }
            emit(core, &repo, &ch);
            Ok(json!({ "commit": commit }))
        }
        "node.cleanup" => {
            let node = {
                let st = repo.st.lock().unwrap();
                require_ended(&st, &ch, &nd)?;
                find_node(&st, &ch, &nd)?
            };
            // A root node has nothing to delete: cleanup only takes it off the canvas
            if let Some(w) = &node.worktree {
                wt::cleanup(&repo.root, w, &current_branch(&repo)?).map_err(git_err)?;
            }
            remove_node(core, &repo, &ch, &nd, None)
        }
        "node.discardPreview" => {
            let node = {
                let st = repo.st.lock().unwrap();
                require_ended(&st, &ch, &nd)?;
                find_node(&st, &ch, &nd)?
            };
            let w = node.worktree.ok_or_else(|| err("invalid", "A root node is never discarded; use cleanup"))?;
            let wt::DiscardPreview { unmerged_commits, changes, ignored, token: t } = wt::discard_preview(&repo.root, &w, &current_branch(&repo)?).map_err(git_err)?;
            let token = uuid();
            let out = json!({ "token": token, "unmergedCommits": unmerged_commits, "changes": changes, "ignored": ignored });
            repo.st.lock().unwrap().discards.insert(token, (ch.clone(), nd.clone(), t));
            Ok(out)
        }
        "node.discard" => {
            let token = s(req, "token");
            let (c, n, t) = repo.st.lock().unwrap().discards.remove(token).ok_or_else(|| err("stale-preview", "Preview again"))?;
            if c != ch || n != nd {
                return Err(err("stale-preview", "Preview again"));
            }
            require_ended(&repo.st.lock().unwrap(), &ch, &nd)?;
            wt::discard(&repo.root, &t).map_err(git_err)?;
            remove_node(core, &repo, &ch, &nd, Some("discarded"))
        }
        "node.approve" | "node.reject" => answer(core, &repo, &ch, &nd, req, op == "node.approve", origin),
        "node.message" => {
            let text = s(req, "text");
            if text.trim().is_empty() || text.len() > MAX_TEXT {
                return Err(err("invalid", "Empty or too long"));
            }
            let pty = {
                let st = repo.st.lock().unwrap();
                if st.stop.is_some() {
                    return Err(err("stopped", "Stopped"));
                }
                let rt = st.rt.get(&key(&ch, &nd)).ok_or_else(|| err("not-found", "node"))?;
                if rt.generation.as_deref() != Some(s(req, "sessionGeneration")) {
                    return Err(err("stale-request", "The session changed"));
                }
                if rt_view(core, rt, false)["inputReady"] != json!(true) {
                    return Err(err("not-ready", "The CLI is busy, waiting for an answer or not ready; copy the text instead"));
                }
                rt.pty_id.clone().ok_or_else(|| err("not-ready", "not running"))?
            };
            let clean = sanitize_text(text, true);
            // Without bracketed paste a newline is Enter: only a single line may go in, and never with Enter
            let data = if core.pty_bracketed_paste(&pty) == Some(true) {
                format!("\x1b[200~{clean}\x1b[201~")
            } else if !clean.contains('\n') {
                clean
            } else {
                return Err(err("not-ready", "This CLI did not turn on bracketed paste; copy the text instead"));
            };
            if !core.write_pty(&pty, &data) {
                return Err(err("not-ready", "not running"));
            }
            Ok(Value::Null)
        }
        "handoff.create" => {
            let (from, to) = (s(req, "from"), s(req, "to"));
            let node = {
                let st = repo.st.lock().unwrap();
                find_node(&st, &ch, to)?;
                find_node(&st, &ch, from)?
            };
            if from == to {
                return Err(err("invalid", "Same node"));
            }
            let text = handoff_text(&repo, &node, s(req, "note"));
            let edge = Edge { id: short_id(), from: from.into(), to: to.into(), at: now_ms(), text };
            {
                let mut st = repo.st.lock().unwrap();
                let c = st.channels.get_mut(&ch).ok_or_else(|| err("not-found", "channel"))?;
                if c.edges.len() >= 200 {
                    return Err(err("invalid", "Too many handoffs"));
                }
                c.edges.push(edge.clone());
                push_event(c, json!({ "t": "handoff", "at": edge.at, "edge": edge.id, "from": edge.from, "to": edge.to, "text": edge.text }));
                repo.save(c)?;
            }
            emit(core, &repo, &ch);
            Ok(serde_json::to_value(&edge).unwrap())
        }
        "edge.remove" => {
            {
                let mut st = repo.st.lock().unwrap();
                let c = st.channels.get_mut(&ch).ok_or_else(|| err("not-found", "channel"))?;
                c.edges.retain(|e| e.id != s(req, "edgeId"));
                repo.save(c)?;
            }
            emit(core, &repo, &ch);
            Ok(Value::Null)
        }
        "stop" => stop(core, &repo),
        "stop.killAll" if origin == Origin::Desktop => kill_all(core, &repo),
        "stop.clear" if origin == Origin::Desktop => {
            {
                let mut st = repo.st.lock().unwrap();
                let st = &mut *st;
                if st.stop.take().is_some() {
                    for c in st.channels.values_mut() {
                        push_event(c, json!({ "t": "stop-cleared", "at": now_ms() }));
                        let _ = repo.save(c);
                    }
                }
            }
            emit_all(core, &repo);
            Ok(stop_view(&repo.st.lock().unwrap()))
        }
        "legacy.inventory" if origin == Origin::Desktop => {
            let raw = crate::util::read_text(&core.global_settings_path()).and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(Value::Null);
            let list = wt::legacy_inventory(&repo.root, &core.data_dir.join("cowork"), &raw).map_err(git_err)?;
            Ok(serde_json::to_value(list).unwrap())
        }
        op if op.starts_with("team.") || op.starts_with("relay.") || op.starts_with("teammerge.") => team::dispatch(core, &repo, op, req, origin),
        _ => Err(err("invalid", format!("Unknown canvas operation: {}", js_prefix(op, 40)))),
    }
}

fn remove_node(core: &Core, repo: &Arc<Repo>, ch: &str, nd: &str, event: Option<&str>) -> R<Value> {
    {
        let mut st = repo.st.lock().unwrap();
        let st = &mut *st;
        let c = st.channels.get_mut(ch).ok_or_else(|| err("not-found", "channel"))?;
        c.nodes.retain(|n| n.id != nd);
        // The handoff text stays in the event history after its edge goes
        c.edges.retain(|e| e.from != nd && e.to != nd);
        team::forget_node(c, nd);
        if let Some(t) = event {
            push_event(c, json!({ "t": t, "at": now_ms(), "node": nd }));
        }
        repo.save(c)?;
        st.rt.remove(&key(ch, nd));
    }
    emit(core, repo, ch);
    Ok(Value::Null)
}

/// One-shot answer to the current request. Approve is refused unless the backend can prove what it would answer.
fn answer(core: &Arc<Core>, repo: &Arc<Repo>, ch: &str, nd: &str, req: &Value, approve: bool, origin: Origin) -> R<Value> {
    let (pty, a) = {
        let mut st = repo.st.lock().unwrap();
        let st = &mut *st;
        let stopped = st.stop.is_some();
        let rt = st.rt.get_mut(&key(ch, nd)).ok_or_else(|| err("not-found", "node"))?;
        let ok = rt.approval.as_ref().is_some_and(|a| a.request_id == s(req, "requestId") && a.generation == s(req, "sessionGeneration") && rt.generation.as_deref() == Some(a.generation.as_str()));
        if !ok {
            return Err(err("stale-request", "This request is no longer current"));
        }
        if rt.approval.as_ref().is_some_and(|a| now_ms() > a.expires_at) {
            expire(st.channels.get_mut(ch), nd, rt);
            return Err(err("stale-request", "This request expired"));
        }
        if approve && stopped {
            return Err(err("stopped", "Stopped: approvals are refused"));
        }
        let a = rt.approval.as_ref().unwrap();
        if (approve && !a.can_approve) || (!approve && !a.can_reject) {
            return Err(err("invalid", "Answer this one in the terminal"));
        }
        // Consumed here, under the lock: a second answer from another window or the phone finds nothing
        (rt.pty_id.clone(), rt.approval.take().unwrap())
    };
    let decided = |decision: &str| {
        let mut st = repo.st.lock().unwrap();
        if let Some(c) = st.channels.get_mut(ch) {
            let mut ev = json!({ "t": "approval", "at": now_ms(), "node": nd, "requestId": a.request_id, "status": "resolved", "decision": decision });
            if origin == Origin::Phone {
                ev["via"] = json!("phone");
            }
            push_event(c, ev);
            let _ = repo.save(c);
        }
    };
    let result = match (&a.reply, a.source) {
        (Some(tx), _) => {
            let reason = js_prefix(&sanitize_text(s(req, "reason"), false), 300);
            let sent = tx.send(if approve { HookDecision::Allow } else { HookDecision::Deny(reason) }).is_ok();
            if sent {
                Ok(())
            } else {
                Err(err("stale-request", "The CLI stopped waiting"))
            }
        }
        (None, _) => {
            let pty = pty.as_deref().ok_or_else(|| err("stale-request", "not running"))?;
            // The screen must still show the same question, or the key would answer something else
            let now = wb_pty::approval::parse_approval_prompt(&core.pty_screen_text(pty).unwrap_or_default());
            match now {
                Some(p) if p.fingerprint == a.fingerprint && (if approve { p.can_approve } else { p.can_reject }) => {
                    core.write_pty(pty, if approve { wb_pty::approval::APPROVE_KEYS } else { wb_pty::approval::REJECT_KEYS });
                    Ok(())
                }
                _ => Err(err("stale-request", "The question on screen changed")),
            }
        }
    };
    match &result {
        Ok(()) => decided(if approve { "approved" } else { "rejected" }),
        Err(_) => {
            let mut st = repo.st.lock().unwrap();
            if let Some(c) = st.channels.get_mut(ch) {
                push_event(c, json!({ "t": "approval", "at": now_ms(), "node": nd, "requestId": a.request_id, "status": "expired" }));
                let _ = repo.save(c);
            }
        }
    }
    emit(core, repo, ch);
    // A changed screen gets a fresh card on its own; a hook that gave up falls back to the CLI's prompt
    if let Some(pty) = &pty {
        if result.is_err() && core.get_pty_session(pty).is_some_and(|i| i["needsApproval"] == json!(true)) {
            on_screen_approval(core, pty);
        }
    }
    result.map(|_| Value::Null)
}

/// Stage 1 for this repo: one Esc per running node, every approval handed back, new work refused
fn stop(core: &Arc<Core>, repo: &Arc<Repo>) -> R<Value> {
    let ptys: Vec<String> = {
        let mut st = repo.st.lock().unwrap();
        let st = &mut *st;
        let first = st.stop.is_none();
        if first {
            st.stop = Some(now_ms());
        }
        let mut ptys = Vec::new();
        for c in st.channels.values_mut() {
            let ids: Vec<String> = c.nodes.iter().map(|n| n.id.clone()).collect();
            for id in ids {
                if let Some(rt) = st.rt.get_mut(&key(&c.id, &id)) {
                    if let Some(a) = rt.approval.take() {
                        if let Some(tx) = a.reply {
                            let _ = tx.send(HookDecision::Pass);
                        }
                        push_event(c, json!({ "t": "approval", "at": now_ms(), "node": id, "requestId": a.request_id, "status": "expired" }));
                    }
                    // A repeated stop never sends a second Esc: two in a row mean rewind or edit in some CLIs
                    if first && rt.status == "running" {
                        ptys.extend(rt.pty_id.clone());
                    }
                }
            }
            if first {
                push_event(c, json!({ "t": "stop-requested", "at": now_ms() }));
                // Teams stay paused after the stop is cleared; the user resumes each one
                team::pause_all(c, "stopped");
            }
            let _ = repo.save(c);
        }
        ptys
    };
    for pty in ptys {
        core.write_pty(&pty, "\x1b");
    }
    emit_all(core, repo);
    Ok(stop_view(&repo.st.lock().unwrap()))
}

fn kill_all(core: &Arc<Core>, repo: &Arc<Repo>) -> R<Value> {
    let targets: Vec<(String, String, String)> = {
        let st = repo.st.lock().unwrap();
        if st.stop.is_none() {
            return Err(err("invalid", "Stop first"));
        }
        st.channels.values().flat_map(|c| c.nodes.iter().filter_map(|n| st.rt.get(&key(&c.id, &n.id)).and_then(|r| r.pty_id.clone()).map(|p| (c.id.clone(), n.id.clone(), p)))).collect()
    };
    for (_, _, pty) in &targets {
        core.kill_pty(pty);
    }
    let failed: Vec<Value> = targets.iter().filter(|(_, _, pty)| !wait_gone(core, pty)).map(|(c, n, _)| json!({ "channelId": c, "nodeId": n, "error": "exit not confirmed" })).collect();
    emit_all(core, repo);
    Ok(json!({ "failed": failed }))
}

// ---------- entry points ----------

/// Desktop windows: `canvas:<op>` with one request object
pub fn call(core: &Arc<Core>, window: u32, channel: &str, args: &[Value]) -> Result<Value, String> {
    let op = channel.strip_prefix("canvas:").unwrap_or("");
    let ws = core.workspace_for(Some(window));
    let req = args.first().cloned().unwrap_or(Value::Null);
    let core2 = core.clone();
    // A panic in a handler comes back as an error instead of taking the app along
    Ok(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| wrap(dispatch(&core2, &ws, op, &req, Origin::Desktop)))).unwrap_or_else(|_| json!({ "ok": false, "code": "internal" })))
}

/// The phone's whitelist is enforced by the remote server; this only runs those ops
pub fn phone_call(core: &Arc<Core>, ws: &str, op: &str, req: &Value) -> Value {
    if !crate::remote::server::PHONE_CANVAS_OPS.contains(&op) {
        return json!({ "ok": false, "code": "not-allowed" });
    }
    wrap(dispatch(core, ws, op, req, Origin::Phone))
}

/// File contents never go to a phone (§3.9): hook cards keep names and line counts only. Team messages keep their
/// first 200 characters, enough to decide on a relay card (cowork-team-implement.md §5.4).
pub fn for_phone(mut v: Value) -> Value {
    fn walk(v: &mut Value) {
        match v {
            Value::Object(o) => {
                if o.contains_key("teamId") && o.contains_key("kind") {
                    if let Some(Value::String(t)) = o.get_mut("text") {
                        *t = js_prefix(t, 200);
                    }
                }
                if let Some(Value::Object(h)) = o.get_mut("hook") {
                    h.remove("raw");
                    h.remove("command");
                    if let Some(Value::Array(files)) = h.get_mut("files") {
                        for f in files.iter_mut().filter_map(Value::as_object_mut) {
                            f.remove("before");
                            f.remove("after");
                            f.remove("patch");
                        }
                    }
                }
                for x in o.values_mut() {
                    walk(x);
                }
            }
            Value::Array(a) => a.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(&mut v);
    v
}

// ---------- Phase 6: hook requests ----------

/// A hook request from wb-hook, already parsed by approval_hook. Blocks until a decision, expiry or hand-back.
pub fn hook_request(core: &Arc<Core>, token: &str, card: Value, wait: Duration) -> HookDecision {
    let Ok(app) = app(core) else { return HookDecision::Pass };
    let Some(pty) = app.hooks.lock().unwrap().get(token).cloned() else { return HookDecision::Pass };
    let Some((repo, ch, nd)) = app.ptys.lock().unwrap().get(&pty).cloned() else { return HookDecision::Pass };
    let (tx, rx) = mpsc::channel();
    let request_id = {
        let mut st = repo.st.lock().unwrap();
        let st = &mut *st;
        if st.stop.is_some() {
            return HookDecision::Pass;
        }
        let Some(rt) = st.rt.get_mut(&key(&ch, &nd)) else { return HookDecision::Pass };
        let Some(generation) = rt.generation.clone() else { return HookDecision::Pass };
        // One card per node: an overlapping request goes back to the CLI rather than replacing the current one
        if rt.approval.as_ref().is_some_and(|a| a.source == "hook") {
            return HookDecision::Pass;
        }
        // A hook beats a screen card for the same moment: the screen one is about the same question, with less detail
        expire(st.channels.get_mut(&ch), &nd, rt);
        let now = now_ms();
        let id = uuid();
        rt.approval = Some(Approval {
            request_id: id.clone(),
            generation,
            source: "hook",
            created_at: now,
            expires_at: now + wait.as_millis() as f64,
            title: card["title"].as_str().unwrap_or("Permission request").to_string(),
            tail: String::new(),
            can_approve: true,
            can_reject: true,
            fingerprint: String::new(),
            hook: Some(card["hook"].clone()),
            reply: Some(tx),
        });
        if let Some(c) = st.channels.get_mut(&ch) {
            push_event(c, json!({ "t": "approval", "at": now, "node": nd, "requestId": id, "status": "pending" }));
            let _ = repo.save(c);
        }
        id
    };
    emit(core, &repo, &ch);
    let decision = rx.recv_timeout(wait).unwrap_or(HookDecision::Pass);
    // Timed out or handed back: the card must not stay answerable
    let changed = {
        let mut st = repo.st.lock().unwrap();
        let st = &mut *st;
        match st.rt.get_mut(&key(&ch, &nd)) {
            Some(rt) if rt.approval.as_ref().is_some_and(|a| a.request_id == request_id) => {
                expire(st.channels.get_mut(&ch), &nd, rt);
                true
            }
            _ => false,
        }
    };
    if changed {
        emit(core, &repo, &ch);
    }
    decision
}

/// wb-hook's connection dropped (the CLI killed it after the user answered in the terminal): expire that card
pub fn hook_gone(core: &Arc<Core>, token: &str) {
    let Ok(app) = app(core) else { return };
    let Some(pty) = app.hooks.lock().unwrap().get(token).cloned() else { return };
    let Some((repo, ch, nd)) = app.ptys.lock().unwrap().get(&pty).cloned() else { return };
    let changed = {
        let mut st = repo.st.lock().unwrap();
        let st = &mut *st;
        match st.rt.get_mut(&key(&ch, &nd)) {
            Some(rt) if rt.approval.as_ref().is_some_and(|a| a.source == "hook") => {
                expire(st.channels.get_mut(&ch), &nd, rt);
                true
            }
            _ => false,
        }
    };
    if changed {
        emit(core, &repo, &ch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_validation_rejects_bad_ids_and_dangling_edges() {
        let n = |id: &str| Node { id: id.into(), agent: "claude".into(), worktree: None, position: Pos { x: 0.0, y: 0.0 }, size: Size { width: 640.0, height: 400.0 } };
        let mut c = Channel { schema_version: 1, id: "abc".into(), name: "x".into(), nodes: vec![n("a"), n("b")], edges: vec![], events: vec![], teams: vec![], tasks: Default::default() };
        assert!(channel_ok(&c));
        c.edges.push(Edge { id: "e".into(), from: "a".into(), to: "zz".into(), at: 0.0, text: String::new() });
        assert!(!channel_ok(&c), "edge to a missing node");
        c.edges.clear();
        c.nodes.push(n("../x"));
        assert!(!channel_ok(&c), "path in an id");
        c.nodes.pop();
        c.schema_version = 2;
        assert!(!channel_ok(&c), "unknown schema is never loaded, so never overwritten");
        c.schema_version = 1;
        c.nodes[0].size.width = f64::NAN;
        assert!(!channel_ok(&c));
    }

    #[test]
    fn drafts_and_messages_carry_no_control_sequences() {
        let s = sanitize_text("a\x1b[201~\r\nrm -rf /\x07\u{9b}31m\tb", true);
        assert_eq!(s, "a[201~\nrm -rf /31m\tb");
        assert!(!sanitize_text("x\ny", false).contains('\n'));
    }

    #[test]
    fn phone_view_has_no_file_contents() {
        let v = json!({ "ok": true, "data": { "runtime": { "n": { "approval": { "hook": { "tool": "Edit", "raw": "secret", "command": "*** Begin Patch", "files": [{ "path": "a.rs", "added": 1, "removed": 0, "before": "x", "after": "y", "patch": "p" }] } } } } } });
        let out = for_phone(v).to_string();
        for bad in ["secret", "Begin Patch", "\"before\"", "\"after\"", "\"patch\""] {
            assert!(!out.contains(bad), "{bad}");
        }
        assert!(out.contains("a.rs") && out.contains("\"added\":1"));
        let long = "x".repeat(5000);
        let v = json!({ "data": { "relays": [{ "id": "m", "teamId": "t", "kind": "message", "text": long }] } });
        assert_eq!(for_phone(v)["data"]["relays"][0]["text"].as_str().unwrap().len(), 200, "team messages are cut for the phone");
    }
}
