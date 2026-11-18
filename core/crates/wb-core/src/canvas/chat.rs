//! Chat nodes (cowork-agui-implement.md): the CLI runs headless behind an AG-UI harness. Live events go to every
//! window as `canvas:agui`; whole messages, tool summaries and approvals are kept per node in `<channel>.chat.json`.
use std::collections::BTreeMap;

use crate::agui::session::{Chat, Sink, Spawn};
use crate::agui::transcript::{Entry, Transcript};
use crate::agui::{acp::Acp, claude, codex::Codex, Event, Harness, Out};

use super::*;

pub const AGUI_EVENT: &str = "canvas:agui";
pub const MODE: &str = "chat";
/// Claude's own mode in a chat node: edits in the worktree go ahead, commands and anything outside ask
const CLAUDE_MODE: &str = "acceptEdits";
/// Codex's sandbox in a chat node: same idea
const CODEX_SANDBOX: &str = "workspace-write";

/// Which protocol a node's agent speaks in chat mode; None = terminal only
pub fn protocol(core: &Core, agent: &str) -> Option<&'static str> {
    match agent {
        "claude" => Some("claude-stream-json"),
        "codex" => Some("codex-app-server"),
        "antigravity" => Some("acp"),
        crate::custom_agent::CUSTOM_AGENT_ID => match core.load_settings().pointer("/customAgent/protocol").and_then(Value::as_str) {
            Some("claude-stream-json") => Some("claude-stream-json"),
            Some("codex-app-server") => Some("codex-app-server"),
            Some("acp") => Some("acp"),
            _ => None,
        },
        _ => None,
    }
}

impl Repo {
    fn chat_file(&self, channel: &str) -> PathBuf {
        self.dir().join(format!("{channel}.chat.json"))
    }

    fn transcripts<'a>(&self, st: &'a mut RepoState, channel: &str) -> &'a mut BTreeMap<String, Transcript> {
        st.transcripts.entry(channel.to_string()).or_insert_with(|| read_json_with_fallback(&self.chat_file(channel), &|v| serde_json::from_value::<BTreeMap<String, Transcript>>(v.clone()).is_ok()).and_then(|(v, _)| serde_json::from_value(v).ok()).unwrap_or_default())
    }

    fn save_transcripts(&self, st: &RepoState, channel: &str) {
        if let Some(t) = st.transcripts.get(channel) {
            let _ = std::fs::create_dir_all(self.dir());
            let _ = write_json_atomic(&self.chat_file(channel), &serde_json::to_value(t).unwrap());
        }
    }
}

fn live(core: &Core, repo: &Repo, ch: &str, nd: &str, body: Value) {
    let mut v = json!({ "repoId": repo.id, "channelId": ch, "nodeId": nd });
    if let (Some(o), Value::Object(b)) = (v.as_object_mut(), body) {
        o.extend(b);
    }
    core.host.send_all(AGUI_EVENT, json!([v]));
}

/// Entries change on these; the stream's fragments only update the copy in memory
fn worth_saving(e: &Event) -> bool {
    !matches!(e, Event::TextMessageContent { .. } | Event::ToolCallArgs { .. } | Event::TextMessageStart { .. } | Event::ToolCallStart { .. })
}

fn on_out(core: &Arc<Core>, repo: &Arc<Repo>, ch: &str, nd: &str, generation: &str, out: Out) {
    let now = now_ms();
    let mut refresh = false;
    let mut entry: Option<Entry> = None;
    {
        let mut st = repo.st.lock().unwrap();
        let st = &mut *st;
        // Output from a process this node no longer runs is dropped
        if st.rt.get(&key(ch, nd)).and_then(|r| r.generation.as_deref()) != Some(generation) {
            return;
        }
        match &out {
            Out::Ui(e) => {
                entry = repo.transcripts(st, ch).entry(nd.to_string()).or_default().apply(e, now);
                if let Some(rt) = st.rt.get_mut(&key(ch, nd)) {
                    match e {
                        Event::RunStarted { .. } => {
                            rt.in_run = true;
                            refresh = true;
                        }
                        Event::RunFinished { .. } | Event::RunError { .. } => {
                            rt.in_run = false;
                            refresh = true;
                        }
                        _ => {}
                    }
                }
                if worth_saving(e) {
                    repo.save_transcripts(st, ch);
                }
            }
            Out::Approval(a) => {
                repo.transcripts(st, ch).entry(nd.to_string()).or_default().approval(a, now);
                repo.save_transcripts(st, ch);
                refresh = true;
            }
            Out::Session(s) => {
                if let Some(c) = st.channels.get_mut(ch) {
                    if let Some(n) = c.nodes.iter_mut().find(|n| n.id == nd) {
                        n.session = Some(s.clone());
                        let _ = repo.save(c);
                    }
                }
            }
            Out::Send(_) => {}
        }
    }
    match out {
        Out::Ui(e) => live(core, repo, ch, nd, json!({ "event": e, "entry": entry })),
        Out::Approval(a) => live(core, repo, ch, nd, json!({ "approval": a })),
        _ => {}
    }
    if refresh {
        emit(core, repo, ch);
        // A node that just finished a turn may have team mail waiting
        team::deliver(core, repo, ch);
    }
}

fn on_ended(core: &Arc<Core>, repo: &Arc<Repo>, ch: &str, nd: &str, generation: &str, code: Option<i32>, stderr: String) {
    {
        let mut st = repo.st.lock().unwrap();
        let st = &mut *st;
        let has_wt = st.channels.get(ch).and_then(|c| c.nodes.iter().find(|n| n.id == nd)).is_some_and(|n| n.worktree.is_some());
        let Some(rt) = st.rt.get_mut(&key(ch, nd)) else { return };
        if rt.generation.as_deref() != Some(generation) {
            return;
        }
        let failed = rt.status == "running" && code != Some(0) && !stderr.trim().is_empty();
        rt.chat = None;
        rt.generation = None;
        rt.in_run = false;
        rt.exit_code = code;
        rt.status = if has_wt { "pending-cleanup" } else { "exited" };
        if failed {
            rt.error = Some(js_prefix(stderr.trim(), 2000));
        }
        let t = repo.transcripts(st, ch).entry(nd.to_string()).or_default();
        t.expire_pending();
        repo.save_transcripts(st, ch);
        if let Some(c) = st.channels.get_mut(ch) {
            push_event(c, json!({ "t": "node-exited", "at": now_ms(), "node": nd }));
            let _ = repo.save(c);
        }
    }
    emit(core, repo, ch);
}

/// Starts the node's CLI headless; resumes the node's last session when it has one. Caller holds no repo lock.
pub(super) fn launch(core: &Arc<Core>, repo: &Arc<Repo>, ch: &str, node: &Node) -> R<()> {
    let proto = protocol(core, &node.agent).ok_or_else(|| err("invalid", format!("{} has no chat mode", node.agent)))?;
    let cwd = node.worktree.as_ref().map(|w| w.path.clone()).unwrap_or_else(|| p(&repo.root));
    let (command, mut args, env) = core.agent_process(&node.agent).map_err(|e| err("invalid", e))?;
    let thread = key(ch, &node.id);
    let resume = node.session.as_deref();
    let harness: Box<dyn Harness> = match proto {
        "claude-stream-json" => {
            args.extend(claude::args(resume, CLAUDE_MODE, None));
            Box::new(claude::Claude::new(&thread))
        }
        "codex-app-server" => {
            codex_protocol_ok(core, &command, &args, &env)?;
            args.extend(crate::agui::codex::args());
            Box::new(Codex::new(&thread, &cwd, resume, CODEX_SANDBOX))
        }
        _ => {
            args.push("acp".into());
            Box::new(Acp::new(&thread, &cwd, resume))
        }
    };
    let generation = uuid();
    {
        let mut st = repo.st.lock().unwrap();
        let rt = st.rt.entry(key(ch, &node.id)).or_default();
        *rt = NodeRt { status: "creating", generation: Some(generation.clone()), ..Default::default() };
    }
    let (c1, r1, ch1, nd1, g1) = (core.clone(), repo.clone(), ch.to_string(), node.id.clone(), generation.clone());
    let sink: Sink = Arc::new(move |o| on_out(&c1, &r1, &ch1, &nd1, &g1, o));
    let (c2, r2, ch2, nd2, g2) = (core.clone(), repo.clone(), ch.to_string(), node.id.clone(), generation.clone());
    let chat = Chat::spawn(Spawn { command: &command, args: &args, cwd: &cwd, env: &env }, harness, |pid| core.pty.assign_job(pid), sink, move |code, stderr| on_ended(&c2, &r2, &ch2, &nd2, &g2, code, stderr)).map_err(|e| err("invalid", e))?;
    let mut st = repo.st.lock().unwrap();
    if let Some(rt) = st.rt.get_mut(&key(ch, &node.id)) {
        if rt.generation.as_deref() == Some(generation.as_str()) && rt.status == "creating" {
            rt.chat = Some(chat);
            rt.status = "running";
        }
    }
    if let Some(c) = st.channels.get_mut(ch) {
        push_event(c, json!({ "t": "node-started", "at": now_ms(), "node": node.id }));
        let _ = repo.save(c);
    }
    Ok(())
}

fn running_chat(st: &RepoState, ch: &str, nd: &str) -> R<Arc<Chat>> {
    let rt = st.rt.get(&key(ch, nd)).ok_or_else(|| err("not-found", "node"))?;
    rt.chat.clone().filter(|_| rt.status == "running").ok_or_else(|| err("not-ready", "The agent is not running"))
}

/// Sends `text` to a chat node as a new turn and records it; `from` is "user" or the sending node's id
pub(super) fn prompt(core: &Arc<Core>, repo: &Arc<Repo>, ch: &str, nd: &str, text: &str, from: &str) -> R<()> {
    let chat = {
        let mut st = repo.st.lock().unwrap();
        if st.stop.is_some() {
            return Err(err("stopped", "Stopped"));
        }
        let chat = running_chat(&st, ch, nd)?;
        let id = short_id();
        repo.transcripts(&mut st, ch).entry(nd.to_string()).or_default().user(&id, text, from, now_ms());
        repo.save_transcripts(&st, ch);
        live(core, repo, ch, nd, json!({ "entry": Entry::User { id, text: text.into(), from: from.into(), at: now_ms() } }));
        chat
    };
    chat.prompt(text);
    Ok(())
}

pub(super) fn dispatch(core: &Arc<Core>, repo: &Arc<Repo>, op: &str, req: &Value) -> R<Value> {
    let (ch, nd) = (s(req, "channelId"), s(req, "nodeId"));
    match op {
        "chat.prompt" => {
            let text = s(req, "text");
            if text.trim().is_empty() || text.len() > MAX_TEXT {
                return Err(err("invalid", "Empty or too long"));
            }
            prompt(core, repo, ch, nd, &sanitize_text(text, true), "user")?;
            Ok(Value::Null)
        }
        "chat.interrupt" => {
            let chat = running_chat(&repo.st.lock().unwrap(), ch, nd)?;
            chat.interrupt();
            Ok(Value::Null)
        }
        "chat.answer" => {
            let id = s(req, "approvalId");
            let allow = req["allow"] == true;
            let for_session = allow && req["forSession"] == true;
            let chat = {
                let mut st = repo.st.lock().unwrap();
                let chat = running_chat(&st, ch, nd)?;
                // One answer per approval, from whichever window or phone gets there first
                if !repo.transcripts(&mut st, ch).entry(nd.to_string()).or_default().settle(id, if allow { "allowed" } else { "denied" }) {
                    return Err(err("stale-request", "Already answered"));
                }
                repo.save_transcripts(&st, ch);
                chat
            };
            chat.answer(id, allow, for_session);
            live(core, repo, ch, nd, json!({ "settled": { "id": id, "status": if allow { "allowed" } else { "denied" } } }));
            emit(core, repo, ch);
            Ok(Value::Null)
        }
        "chat.transcript" => {
            let mut st = repo.st.lock().unwrap();
            let all = repo.transcripts(&mut st, ch).clone();
            Ok(if nd.is_empty() { serde_json::to_value(all).unwrap() } else { serde_json::to_value(all.get(nd).cloned().unwrap_or_default()).unwrap() })
        }
        _ => Err(err("invalid", format!("Unknown chat operation: {}", js_prefix(op, 40)))),
    }
}

/// The executable's own schema must have everything the Codex harness uses (§4.2). Generated once per executable
/// version (path, size, modified time) into the data dir; a later launch of the same file only checks the marker.
fn codex_protocol_ok(core: &Core, command: &str, pre: &[String], env: &[(String, String)]) -> R<()> {
    use sha2::{Digest, Sha256};
    let meta = std::fs::metadata(command).ok();
    let stamp = format!("{command}|{:?}|{:?}", meta.as_ref().map(|m| m.len()), meta.as_ref().and_then(|m| m.modified().ok()));
    let hash: String = Sha256::digest(stamp.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect();
    let dir = core.data_dir.join("agui-schema").join(hash);
    if dir.join("ok").exists() {
        return Ok(());
    }
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| err("invalid", e.to_string()))?;
    let mut args = pre.to_vec();
    args.extend(["app-server", "generate-json-schema", "--out"].map(String::from));
    args.push(p(&dir));
    let r = wb_proc::run_process(&wb_proc::RunSpec { command: command.into(), args, cwd: p(&dir), stdin: String::new(), timeout_ms: 60_000, max_bytes: 1 << 20, env: Some(env.to_vec()) }, &wb_proc::CancelToken::default());
    if r.code != Some(0) {
        let why = r.spawn_error.unwrap_or_else(|| js_prefix(r.stderr.trim(), 400));
        return Err(err("invalid", format!("{command} could not describe its app-server protocol: {why}")));
    }
    let miss = crate::agui::codex::missing(&dir);
    if !miss.is_empty() {
        return Err(err("invalid", format!("This Codex cannot run in chat mode; its app-server lacks: {}", miss.join(", "))));
    }
    let _ = std::fs::write(dir.join("ok"), "");
    Ok(())
}

/// The CLI's own TUI resuming a chat node's session
fn resume_args(core: &Core, agent: &str, session: &str) -> Option<Vec<String>> {
    let a = |v: &[&str]| Some(v.iter().map(|s| s.to_string()).chain([session.to_string()]).collect());
    match protocol(core, agent)? {
        "claude-stream-json" => a(&["--resume"]),
        "codex-app-server" => a(&["resume"]),
        _ if agent == "antigravity" => a(&["--conversation"]),
        _ => None,
    }
}

fn note(core: &Core, repo: &Repo, ch: &str, nd: &str, text: &str) {
    let mut st = repo.st.lock().unwrap();
    let id = short_id();
    let at = now_ms();
    repo.transcripts(&mut st, ch).entry(nd.to_string()).or_default().note(&id, text, at);
    repo.save_transcripts(&st, ch);
    drop(st);
    live(core, repo, ch, nd, json!({ "entry": Entry::Note { id, text: text.into(), at } }));
}

/// Take control (§8.2): the headless process ends and the CLI's TUI opens on the same session, in a PTY
pub(super) fn take_control(core: &Arc<Core>, repo: &Arc<Repo>, ch: &str, nd: &str) -> R<()> {
    let (node, chat) = {
        let st = repo.st.lock().unwrap();
        if st.stop.is_some() {
            return Err(err("stopped", "Stopped"));
        }
        let node = find_node(&st, ch, nd)?;
        if node.mode.as_deref() != Some(MODE) {
            return Err(err("invalid", "This conversation already runs in a terminal"));
        }
        (node, running_chat(&st, ch, nd)?)
    };
    let session = node.session.clone().ok_or_else(|| err("not-ready", "Nothing to take over yet: send a message first"))?;
    let args = resume_args(core, &node.agent, &session).ok_or_else(|| err("invalid", format!("{} cannot be taken over", node.agent)))?;
    chat.kill();
    let until = Instant::now() + EXIT_WAIT;
    while repo.st.lock().unwrap().rt.get(&key(ch, nd)).is_some_and(|r| r.chat.is_some()) {
        if Instant::now() > until {
            return Err(err("busy", "The agent did not stop"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    note(core, repo, ch, nd, "took-control");
    launch_pty(core, repo, ch, &node, args)?;
    emit(core, repo, ch);
    Ok(())
}

/// Hand back: the TUI ends and the headless process resumes the same session
pub(super) fn hand_back(core: &Arc<Core>, repo: &Arc<Repo>, ch: &str, nd: &str) -> R<()> {
    let (node, pty) = {
        let st = repo.st.lock().unwrap();
        let node = find_node(&st, ch, nd)?;
        let pty = st.rt.get(&key(ch, nd)).and_then(|r| r.pty_id.clone());
        (node, pty)
    };
    if node.mode.as_deref() != Some(MODE) {
        return Err(err("invalid", "This conversation runs in a terminal"));
    }
    if let Some(pty) = pty {
        core.kill_pty(&pty);
        if !wait_gone(core, &pty) {
            return Err(err("busy", "The terminal did not close"));
        }
    }
    note(core, repo, ch, nd, "handed-back");
    let r = launch(core, repo, ch, &node);
    emit(core, repo, ch);
    r
}

/// Pending approvals of a chat node, for the channel list and Needs you
pub(super) fn pending(st: &RepoState, ch: &str, nd: &str) -> usize {
    st.transcripts.get(ch).and_then(|m| m.get(nd)).map(|t| t.entries.iter().filter(|e| matches!(e, Entry::Approval { status, .. } if status == "pending")).count()).unwrap_or(0)
}
