//! Cowork teams (cowork-team-implement.md): a lead and members inside one channel, talking through an inbox.
//! Agents never type into each other's terminals. The app pastes one fixed notice line into an idle CLI; the content
//! only comes back as the result of the `read_inbox` MCP tool. Who is talking is decided by the hook token, never
//! by what a message says.
use std::collections::BTreeMap;

use super::*;

pub(super) const MAX_TEAMS: usize = 16;
const MAX_GOAL: usize = 8 * 1024;
const MAX_SUMMARY: usize = 2 * 1024;
const MAX_MESSAGES_KEPT: usize = 1000;
const RENUDGE_MS: f64 = 3.0 * 60_000.0;
const MAX_NUDGES: u8 = 3;
const PING_PONG: usize = 10;
/// The notice and the Enter go separately: some TUIs take text plus Enter in one burst as a pasted newline
const ENTER_DELAY: Duration = Duration::from_millis(800);

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
pub(super) struct Bounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Limits {
    max_messages: u32,
    per_member_per10_min: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_messages: 200, per_member_per10_min: 20 }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(super) struct Team {
    pub(super) id: String,
    name: String,
    goal: String,
    lead_id: String,
    pub(super) member_ids: Vec<String>,
    /// approve | lead-auto | auto
    relay: String,
    /// draft | running | paused | done | failed
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    paused_reason: Option<String>,
    bounds: Bounds,
    #[serde(default)]
    limits: Limits,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started_at: Option<f64>,
    #[serde(default)]
    message_count: u32,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(super) struct NodeTask {
    title: String,
    /// idle | working | blocked | done | failed
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    updated_at: f64,
    /// assign | report | user
    source: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(super) struct Msg {
    id: String,
    team_id: String,
    /// a node id, or "app" / "user"
    from: String,
    to: String,
    /// task | message | report | system
    kind: String,
    text: String,
    at: f64,
    /// pending-approval | queued | notified | read | rejected
    state: String,
}

/// A lead's request to bring a member's branch into its own worktree, waiting for the user
pub(super) struct MergeReq {
    channel: String,
    team: String,
    lead: String,
    /// The lead's worktree: the merge target
    lead_dir: String,
    member: String,
    token: wt::MergeToken,
    commits: Vec<String>,
    at: f64,
}

const RELAYS: [&str; 3] = ["approve", "lead-auto", "auto"];
const STATUSES: [&str; 5] = ["draft", "running", "paused", "done", "failed"];

pub(super) fn teams_ok(c: &Channel) -> bool {
    let ids: HashSet<&str> = c.nodes.iter().map(|n| n.id.as_str()).collect();
    let mut seen = HashSet::new();
    c.teams.len() <= MAX_TEAMS
        && c.teams.iter().all(|t| {
            valid_id(&t.id)
                && t.name.chars().count() <= 80
                && t.goal.len() <= MAX_GOAL
                && RELAYS.contains(&t.relay.as_str())
                && STATUSES.contains(&t.status.as_str())
                && !t.member_ids.is_empty()
                && t.member_ids.contains(&t.lead_id)
                && t.member_ids.iter().all(|m| ids.contains(m.as_str()) && seen.insert(m.clone()))
                && [t.bounds.x, t.bounds.y, t.bounds.width, t.bounds.height].iter().all(|v| v.is_finite() && v.abs() <= 1e6)
                && t.limits.max_messages >= 1
                && t.limits.per_member_per10_min >= 1
        })
        && c.tasks.iter().all(|(k, t)| valid_id(k) && t.title.len() <= 1024 && t.summary.as_ref().is_none_or(|s| s.len() <= MAX_SUMMARY))
}

// ---------- pure rules (unit tested) ----------

fn team_of<'a>(c: &'a Channel, node: &str) -> Option<&'a Team> {
    c.teams.iter().find(|t| t.member_ids.iter().any(|m| m == node))
}

/// Lead and members talk both ways; two members only over a line the user drew between them
fn route_ok(t: &Team, edges: &[Edge], from: &str, to: &str) -> bool {
    if from == to || !t.member_ids.iter().any(|m| m == to) || !t.member_ids.iter().any(|m| m == from) {
        return false;
    }
    from == t.lead_id || to == t.lead_id || edges.iter().any(|e| (e.from == from && e.to == to) || (e.from == to && e.to == from))
}

/// Messages written by the app or the user are never held; agents' messages follow the team's relay mode
fn needs_approval(t: &Team, from: &str) -> bool {
    match t.relay.as_str() {
        "auto" => false,
        "lead-auto" => from != t.lead_id && from != "app" && from != "user",
        _ => from != "app" && from != "user",
    }
}

/// Caps only apply where messages flow without the user (lead-auto, auto); approve mode has the user on every one
fn limit_hit(t: &Team, msgs: &[Msg], from: &str, to: &str, now: f64) -> Option<String> {
    if t.relay == "approve" || from == "app" || from == "user" {
        return None;
    }
    if t.message_count >= t.limits.max_messages {
        return Some(format!("message limit reached ({})", t.limits.max_messages));
    }
    let recent = msgs.iter().filter(|m| m.team_id == t.id && m.from == from && now - m.at < 10.0 * 60_000.0).count();
    if recent as u32 >= t.limits.per_member_per10_min {
        return Some(format!("{from} sent {recent} messages in 10 minutes"));
    }
    // Ping-pong: the last messages of the team all between the same two nodes, with no report in between
    let pair = |m: &Msg| (m.from == from && m.to == to) || (m.from == to && m.to == from);
    let tail: Vec<&Msg> = msgs.iter().rev().filter(|m| m.team_id == t.id && m.kind != "system").take(PING_PONG - 1).collect();
    if tail.len() == PING_PONG - 1 && tail.iter().all(|m| pair(m) && m.kind != "report") {
        return Some(format!("{from} and {to} keep messaging each other without a report"));
    }
    None
}

fn display(c: &Channel, id: &str) -> String {
    match c.nodes.iter().find(|n| n.id == id) {
        Some(n) => format!("{} ({})", n.id, n.agent),
        None => id.to_string(),
    }
}

fn kickoff(c: &Channel, t: &Team) -> String {
    let members: Vec<String> = t.member_ids.iter().filter(|m| **m != t.lead_id).map(|m| format!("- {}", display(c, m))).collect();
    format!(
        "You are the lead of the Agent Workbench team \"{}\".\nGoal:\n{}\n\nMembers:\n{}\n\nHow to work:\n\
- Split the goal into tasks and give each member one with assign_task(member, title, details). Members work in their own git worktrees and commit there.\n\
- Members answer with report(); their reports reach you as new messages. Use send_message(to, text) for anything else.\n\
- get_member_changes(member) shows what a member committed. request_merge(member) brings a member's branch into your own worktree; commit your own work first.\n\
- You may work in your own worktree too.\n\
- When the goal is done, call report(status=\"done\", summary=...).",
        t.name,
        t.goal,
        if members.is_empty() { "- (none)".into() } else { members.join("\n") }
    )
}

fn task_text(title: &str, details: &str) -> String {
    format!(
        "Task from your team lead: {title}\n{details}\n\nWork in your current directory (your own git worktree) and commit your changes there. \
When you finish, call report(status=\"done\", summary=...). If you are stuck, call report(status=\"blocked\", summary=...)."
    )
}

// ---------- state helpers ----------

pub(super) fn messages_file(repo: &Repo, channel: &str) -> PathBuf {
    repo.dir().join(format!("{channel}.messages.json"))
}

pub(super) fn load_messages(repo: &Repo, channel: &str) -> Vec<Msg> {
    let file = messages_file(repo, channel);
    let ok = |v: &Value| serde_json::from_value::<Vec<Msg>>(v.clone()).is_ok();
    read_json_with_fallback(&file, &ok).and_then(|(v, _)| serde_json::from_value(v).ok()).unwrap_or_default()
}

fn save_messages(repo: &Repo, channel: &str, msgs: &mut Vec<Msg>) {
    if msgs.len() > MAX_MESSAGES_KEPT {
        // Drop the oldest finished ones first; unread mail is never thrown away
        let mut drop = msgs.len() - MAX_MESSAGES_KEPT;
        msgs.retain(|m| {
            if drop > 0 && matches!(m.state.as_str(), "read" | "rejected") {
                drop -= 1;
                false
            } else {
                true
            }
        });
    }
    let _ = write_json_atomic(&messages_file(repo, channel), &serde_json::to_value(&*msgs).unwrap());
}

fn pause(c: &mut Channel, team: &str, reason: &str) {
    if let Some(t) = c.teams.iter_mut().find(|t| t.id == team && t.status == "running") {
        t.status = "paused".into();
        t.paused_reason = Some(reason.into());
        push_event(c, json!({ "t": "team-paused", "at": now_ms(), "team": team, "reason": reason }));
    }
}

/// Stage 1 of the stop pauses every running team of the repo
pub(super) fn pause_all(c: &mut Channel, reason: &str) {
    let ids: Vec<String> = c.teams.iter().filter(|t| t.status == "running").map(|t| t.id.clone()).collect();
    for id in ids {
        pause(c, &id, reason);
    }
}

/// A node leaving the canvas leaves its team; a team without its lead stops
pub(super) fn forget_node(c: &mut Channel, node: &str) {
    c.tasks.remove(node);
    for t in c.teams.iter_mut() {
        t.member_ids.retain(|m| m != node);
        if t.lead_id == node {
            t.lead_id = t.member_ids.first().cloned().unwrap_or_default();
            if t.status == "running" {
                t.status = "paused".into();
                t.paused_reason = Some("the lead left the canvas".into());
            }
        }
    }
    c.teams.retain(|t| !t.member_ids.is_empty());
}

/// Queues one message, or holds it for the user. Returns what the sender is told.
fn post(st: &mut RepoState, repo: &Repo, ch: &str, team: &str, from: &str, to: &str, kind: &str, text: &str) -> R<String> {
    let now = now_ms();
    let text = js_prefix(&sanitize_text(text, true), MAX_TEXT);
    let c = st.channels.get_mut(ch).ok_or_else(|| err("not-found", "channel"))?;
    let t = c.teams.iter().find(|t| t.id == team).ok_or_else(|| err("not-found", "team"))?.clone();
    if t.status != "running" && from != "app" && from != "user" {
        return Err(err("paused", "The team is not running; nothing was sent"));
    }
    let msgs = st.msgs.entry(ch.to_string()).or_default();
    if let Some(reason) = limit_hit(&t, msgs, from, to, now) {
        pause(c, team, &reason);
        let _ = repo.save(c);
        return Err(err("paused", format!("The team was paused: {reason}. Nothing was sent; the user decides how to go on.")));
    }
    let hold = needs_approval(&t, from);
    let id = short_id();
    msgs.push(Msg { id: id.clone(), team_id: team.into(), from: from.into(), to: to.into(), kind: kind.into(), text, at: now, state: if hold { "pending-approval" } else { "queued" }.into() });
    if let Some(t) = c.teams.iter_mut().find(|t| t.id == team) {
        if from != "app" && from != "user" {
            t.message_count += 1;
        }
    }
    // The event keeps who, to whom and how long; the text lives in the messages file only
    let len = msgs.last().map(|m| m.text.len()).unwrap_or(0);
    push_event(c, json!({ "t": "message", "at": now, "team": team, "id": id, "from": from, "to": to, "kind": kind, "len": len, "held": hold }));
    let _ = repo.save(c);
    save_messages(repo, ch, msgs);
    Ok(if hold { format!("Message {id} is waiting for the user's approval; you will hear back if it is rejected.") } else { format!("Message {id} queued for {to}.") })
}

/// Pastes the notice into every idle recipient with mail. Call it after any queueing and whenever a CLI goes idle.
pub(super) fn deliver(core: &Arc<Core>, repo: &Arc<Repo>, ch: &str) {
    let now = now_ms();
    let mut sends: Vec<(String, String, String, usize, String)> = Vec::new();
    {
        let mut st = repo.st.lock().unwrap();
        let st = &mut *st;
        if st.stop.is_some() {
            return;
        }
        let Some(c) = st.channels.get(ch) else { return };
        let running: HashSet<&str> = c.teams.iter().filter(|t| t.status == "running").map(|t| t.id.as_str()).collect();
        let msgs = st.msgs.entry(ch.to_string()).or_default();
        let mut by_node: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, m) in msgs.iter().enumerate() {
            if running.contains(m.team_id.as_str()) && matches!(m.state.as_str(), "queued" | "notified") {
                by_node.entry(m.to.clone()).or_default().push(i);
            }
        }
        for (node, idx) in by_node {
            let k = key(ch, &node);
            let Some(rt) = st.rt.get(&k) else { continue };
            if rt_view(core, rt, false)["inputReady"] != json!(true) {
                continue;
            }
            let fresh = idx.iter().any(|i| msgs[*i].state == "queued");
            let (last, count) = st.nudged.get(&k).copied().unwrap_or((0.0, 0));
            if !fresh && (now - last < RENUDGE_MS || count >= MAX_NUDGES) {
                continue;
            }
            let (Some(pty), Some(generation)) = (rt.pty_id.clone(), rt.generation.clone()) else { continue };
            let senders: Vec<String> = idx.iter().map(|i| msgs[*i].from.clone()).collect::<HashSet<_>>().into_iter().collect();
            let from = senders.iter().map(|s| if s == "app" { "Agent Workbench".to_string() } else { display(c, s) }).collect::<Vec<_>>().join(", ");
            for i in &idx {
                msgs[*i].state = "notified".into();
            }
            st.nudged.insert(k, (now, if fresh { 1 } else { count + 1 }));
            sends.push((node, pty, generation, idx.len(), from));
        }
        if !sends.is_empty() {
            let msgs = st.msgs.get_mut(ch).unwrap();
            save_messages(repo, ch, msgs);
        }
    }
    for (node, pty, generation, n, from) in sends {
        // App-written text only: the content of any message is fetched by the agent through read_inbox
        let notice = format!("[Agent Workbench] You have {n} new message{} from {from}. Call the read_inbox tool of the workbench MCP server and follow it.", if n == 1 { "" } else { "s" });
        // Node ids are validated, but an agent name is free text: one line, no control characters
        let notice = js_prefix(&sanitize_text(&notice, false), 400);
        if !core.write_pty(&pty, &notice) {
            continue;
        }
        let (core, repo, ch) = (core.clone(), repo.clone(), ch.to_string());
        std::thread::spawn(move || {
            std::thread::sleep(ENTER_DELAY);
            let same = repo.st.lock().unwrap().rt.get(&key(&ch, &node)).is_some_and(|r| r.generation.as_deref() == Some(generation.as_str()));
            if same {
                core.write_pty(&pty, "\r");
            }
        });
    }
}

// ---------- desktop and phone operations ----------

fn team_mut<'a>(c: &'a mut Channel, id: &str) -> R<&'a mut Team> {
    c.teams.iter_mut().find(|t| t.id == id).ok_or_else(|| err("not-found", "team"))
}

fn read_members(c: &Channel, req: &Value, except: Option<&str>) -> R<(Vec<String>, String)> {
    let members: Vec<String> = req["memberIds"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    let lead = s(req, "leadId").to_string();
    let unique: HashSet<&String> = members.iter().collect();
    if members.is_empty() || unique.len() != members.len() || !members.contains(&lead) {
        return Err(err("invalid", "A team needs its members, and the lead must be one of them"));
    }
    for m in &members {
        if !c.nodes.iter().any(|n| &n.id == m) {
            return Err(err("not-found", "node"));
        }
        if c.teams.iter().any(|t| Some(t.id.as_str()) != except && t.member_ids.contains(m)) {
            return Err(err("invalid", format!("{m} is already in another team")));
        }
    }
    Ok((members, lead))
}

pub(super) fn dispatch(core: &Arc<Core>, repo: &Arc<Repo>, op: &str, req: &Value, origin: Origin) -> R<Value> {
    let ch = s(req, "channelId").to_string();
    let tid = s(req, "teamId").to_string();
    let desktop = origin == Origin::Desktop;
    let out = match op {
        "team.create" if desktop => {
            let mut st = repo.st.lock().unwrap();
            let c = st.channels.get_mut(&ch).ok_or_else(|| err("not-found", "channel"))?;
            if c.teams.len() >= MAX_TEAMS {
                return Err(err("invalid", "Too many teams"));
            }
            let (members, lead) = read_members(c, req, None)?;
            let relay = if RELAYS.contains(&s(req, "relay")) { s(req, "relay") } else { "approve" };
            let bounds: Bounds = serde_json::from_value(req["bounds"].clone()).map_err(|_| err("invalid", "bounds"))?;
            let name = js_prefix(sanitize_text(s(req, "name"), false).trim(), 80);
            let t = Team {
                id: short_id(),
                name: if name.is_empty() { "team".into() } else { name },
                goal: js_prefix(&sanitize_text(s(req, "goal"), true), MAX_GOAL),
                lead_id: lead,
                member_ids: members,
                relay: relay.into(),
                status: "draft".into(),
                paused_reason: None,
                bounds,
                limits: Limits::default(),
                started_at: None,
                message_count: 0,
            };
            c.teams.push(t.clone());
            if !teams_ok(c) {
                c.teams.pop();
                return Err(err("invalid", "team"));
            }
            repo.save(c)?;
            serde_json::to_value(&t).unwrap()
        }
        "team.update" if desktop => {
            let mut st = repo.st.lock().unwrap();
            let c = st.channels.get_mut(&ch).ok_or_else(|| err("not-found", "channel"))?;
            let running = team_mut(c, &tid)?.status == "running";
            let before = c.teams.clone();
            if req.get("memberIds").is_some() || req.get("leadId").is_some() || req.get("goal").is_some() {
                if running {
                    return Err(err("busy", "Pause the team before changing its members, lead or goal"));
                }
                if req.get("memberIds").is_some() {
                    let (members, lead) = read_members(c, req, Some(&tid))?;
                    let t = team_mut(c, &tid)?;
                    t.member_ids = members;
                    t.lead_id = lead;
                }
                if req.get("goal").is_some() {
                    team_mut(c, &tid)?.goal = js_prefix(&sanitize_text(s(req, "goal"), true), MAX_GOAL);
                }
            }
            let t = team_mut(c, &tid)?;
            if req.get("name").is_some() {
                let name = js_prefix(sanitize_text(s(req, "name"), false).trim(), 80);
                if !name.is_empty() {
                    t.name = name;
                }
            }
            if RELAYS.contains(&s(req, "relay")) {
                t.relay = s(req, "relay").into();
            }
            if let Ok(l) = serde_json::from_value::<Limits>(req["limits"].clone()) {
                t.limits = Limits { max_messages: l.max_messages.clamp(1, 100_000), per_member_per10_min: l.per_member_per10_min.clamp(1, 10_000) };
            }
            if let Ok(b) = serde_json::from_value::<Bounds>(req["bounds"].clone()) {
                t.bounds = b;
            }
            if !teams_ok(c) {
                c.teams = before;
                return Err(err("invalid", "team"));
            }
            repo.save(c)?;
            Value::Null
        }
        "team.delete" if desktop => {
            let mut st = repo.st.lock().unwrap();
            let c = st.channels.get_mut(&ch).ok_or_else(|| err("not-found", "channel"))?;
            if team_mut(c, &tid)?.status == "running" {
                return Err(err("busy", "Pause the team first"));
            }
            c.teams.retain(|t| t.id != tid);
            repo.save(c)?;
            Value::Null
        }
        "team.start" | "team.resume" if desktop => {
            {
                let mut st = repo.st.lock().unwrap();
                let st = &mut *st;
                if st.stop.is_some() {
                    return Err(err("stopped", "Stopped: clear the stop first"));
                }
                let c = st.channels.get_mut(&ch).ok_or_else(|| err("not-found", "channel"))?;
                let t = team_mut(c, &tid)?;
                let first = t.started_at.is_none() || t.status == "done" || t.status == "failed";
                if t.status == "running" {
                    return Ok(Value::Null);
                }
                if op == "team.start" && t.goal.trim().is_empty() {
                    return Err(err("invalid", "Give the team a goal first"));
                }
                t.status = "running".into();
                t.paused_reason = None;
                if first {
                    t.started_at = Some(now_ms());
                    t.message_count = 0;
                }
                let t = t.clone();
                push_event(c, json!({ "t": if first { "team-started" } else { "team-resumed" }, "at": now_ms(), "team": tid }));
                repo.save(c)?;
                if first {
                    let text = kickoff(st.channels.get(&ch).unwrap(), &t);
                    let lead = t.lead_id.clone();
                    st.channels.get_mut(&ch).unwrap().tasks.insert(lead.clone(), NodeTask { title: "Lead: plan and assign the work".into(), status: "working".into(), summary: None, updated_at: now_ms(), source: "assign".into() });
                    post(st, repo, &ch, &tid, "app", &lead, "system", &text)?;
                }
            }
            emit(core, repo, &ch);
            deliver(core, repo, &ch);
            return Ok(Value::Null);
        }
        "team.pause" => {
            let mut st = repo.st.lock().unwrap();
            let c = st.channels.get_mut(&ch).ok_or_else(|| err("not-found", "channel"))?;
            team_mut(c, &tid)?;
            pause(c, &tid, if desktop { "paused by the user" } else { "paused from the phone" });
            repo.save(c)?;
            Value::Null
        }
        "team.finish" if desktop => {
            let mut st = repo.st.lock().unwrap();
            let c = st.channels.get_mut(&ch).ok_or_else(|| err("not-found", "channel"))?;
            let t = team_mut(c, &tid)?;
            t.status = "done".into();
            t.paused_reason = None;
            push_event(c, json!({ "t": "team-done", "at": now_ms(), "team": tid, "by": "user" }));
            repo.save(c)?;
            Value::Null
        }
        "relay.approve" | "relay.reject" => {
            let approve = op == "relay.approve";
            let id = s(req, "messageId");
            {
                let mut st = repo.st.lock().unwrap();
                let st = &mut *st;
                let msgs = st.msgs.entry(ch.clone()).or_default();
                let m = msgs.iter_mut().find(|m| m.id == id && m.state == "pending-approval").ok_or_else(|| err("stale-request", "This message is no longer waiting"))?;
                if approve && st.stop.is_some() {
                    return Err(err("stopped", "Stopped"));
                }
                m.state = if approve { "queued" } else { "rejected" }.into();
                let (team, from, to) = (m.team_id.clone(), m.from.clone(), m.to.clone());
                save_messages(repo, &ch, msgs);
                if let Some(c) = st.channels.get_mut(&ch) {
                    let mut ev = json!({ "t": "relay", "at": now_ms(), "id": id, "decision": if approve { "approved" } else { "rejected" } });
                    if !desktop {
                        ev["via"] = json!("phone");
                    }
                    push_event(c, ev);
                    let _ = repo.save(c);
                }
                if !approve && from != "app" && from != "user" {
                    let to_name = st.channels.get(&ch).map(|c| display(c, &to)).unwrap_or(to.clone());
                    let _ = post(st, repo, &ch, &team, "app", &from, "system", &format!("The user rejected your message {id} to {to_name}. It was not delivered."));
                }
            }
            emit(core, repo, &ch);
            deliver(core, repo, &ch);
            return Ok(Value::Null);
        }
        "teammerge.approve" | "teammerge.reject" if desktop => {
            let id = s(req, "requestId");
            let r = repo.st.lock().unwrap().merge_reqs.remove(id).ok_or_else(|| err("stale-request", "This request is no longer waiting"))?;
            let text = if op == "teammerge.approve" {
                match merge_into_lead(&r) {
                    Ok(t) => t,
                    Err(e) => format!("The merge of {} into your worktree failed: {e}", r.member),
                }
            } else {
                format!("The user declined to merge {} into your worktree.", r.member)
            };
            {
                let mut st = repo.st.lock().unwrap();
                let st = &mut *st;
                if let Some(c) = st.channels.get_mut(&r.channel) {
                    push_event(c, json!({ "t": "team-merge", "at": now_ms(), "team": r.team, "member": r.member, "decision": if op == "teammerge.approve" { "approved" } else { "rejected" } }));
                    let _ = repo.save(c);
                }
                let _ = post(st, repo, &r.channel, &r.team, "app", &r.lead, "system", &text);
            }
            emit(core, repo, &r.channel);
            deliver(core, repo, &r.channel);
            return Ok(Value::Null);
        }
        "team.messages" => {
            let st = repo.st.lock().unwrap();
            let list: Vec<&Msg> = st.msgs.get(&ch).map(|m| m.iter().filter(|m| tid.is_empty() || m.team_id == tid).rev().take(200).collect()).unwrap_or_default();
            return Ok(json!(list));
        }
        _ => return Err(err("not-allowed", "Not available here")),
    };
    emit(core, repo, &ch);
    Ok(out)
}

/// What the views add for teams: the teams, pending relay and merge cards, and per-node role, task and unread
pub(super) fn view_extra(st: &RepoState, c: &Channel, out: &mut Value) {
    let msgs: &[Msg] = st.msgs.get(&c.id).map(|v| v.as_slice()).unwrap_or(&[]);
    let relays: Vec<&Msg> = msgs.iter().filter(|m| m.state == "pending-approval").collect();
    let merges: Vec<Value> = st
        .merge_reqs
        .iter()
        .filter(|(_, r)| r.channel == c.id)
        .map(|(id, r)| json!({ "id": id, "team": r.team, "lead": r.lead, "member": r.member, "commits": r.commits, "at": r.at }))
        .collect();
    out["teams"] = json!(c.teams);
    out["relays"] = json!(relays);
    out["mergeRequests"] = json!(merges);
    out["messages"] = json!(msgs.iter().rev().take(100).collect::<Vec<_>>());
    if let Some(rt) = out["runtime"].as_object_mut() {
        for (node, v) in rt.iter_mut() {
            if let Some(t) = team_of(c, node) {
                v["teamId"] = json!(t.id);
                v["role"] = json!(if t.lead_id == *node { "lead" } else { "member" });
            }
            v["task"] = json!(c.tasks.get(node));
            v["unread"] = json!(msgs.iter().filter(|m| &m.to == node && matches!(m.state.as_str(), "queued" | "notified")).count());
        }
    }
}

// ---------- MCP tools (wb-hook mcp, Phase 2) ----------

/// Tools offered to a node. Outside a team only report and read_inbox, so Feed cards still learn the task.
pub fn tool_names(core: &Arc<Core>, token: &str) -> Vec<&'static str> {
    match whois(core, token) {
        Some((repo, ch, nd)) => {
            let st = repo.st.lock().unwrap();
            match st.channels.get(&ch).and_then(|c| team_of(c, &nd).map(|t| t.lead_id == nd)) {
                Some(true) => ALL_TOOLS.to_vec(),
                Some(false) => vec!["team_info", "read_inbox", "send_message", "report"],
                None => vec!["read_inbox", "report"],
            }
        }
        None => vec![],
    }
}

/// MCP tool definitions; the descriptions are what the models read to decide when to call them
fn tool_spec(name: &str) -> Value {
    let obj = |props: Value, req: &[&str]| json!({ "type": "object", "properties": props, "required": req, "additionalProperties": false });
    let (desc, schema) = match name {
        "team_info" => ("Your Agent Workbench team: the goal, your role, and each member's status and task.", obj(json!({}), &[])),
        "read_inbox" => ("Read your new Agent Workbench messages. Call it whenever a line in your terminal says you have new messages, then follow them.", obj(json!({}), &[])),
        "send_message" => (
            "Send a message to a teammate (the lead, or a member the user connected you with). The user may have to approve it first.",
            obj(json!({ "to": { "type": "string", "description": "Node id of the teammate" }, "text": { "type": "string" } }), &["to", "text"]),
        ),
        "assign_task" => (
            "Lead only: give a member a task. The member is told to work in its own git worktree, commit, and report back.",
            obj(json!({ "member": { "type": "string", "description": "Node id" }, "title": { "type": "string" }, "details": { "type": "string" } }), &["member", "title"]),
        ),
        "report" => (
            "Report your progress. done, blocked and failed also reach your lead. The lead's done marks the whole team done.",
            obj(json!({ "status": { "type": "string", "enum": ["working", "blocked", "done", "failed"] }, "summary": { "type": "string" } }), &["status"]),
        ),
        "get_member_changes" => ("Lead only: the commits and changed files of a member's worktree, from git.", obj(json!({ "member": { "type": "string" } }), &["member"])),
        "request_merge" => (
            "Lead only: bring a member's committed branch into your own worktree. Both worktrees must have no uncommitted changes. Depending on the team's settings the user approves it first.",
            obj(json!({ "member": { "type": "string" } }), &["member"]),
        ),
        _ => ("", obj(json!({}), &[])),
    };
    json!({ "name": name, "description": desc, "inputSchema": schema })
}

const ALL_TOOLS: [&str; 7] = ["team_info", "read_inbox", "send_message", "assign_task", "report", "get_member_changes", "request_merge"];

/// One request from `wb-hook mcp`: {"op":"list"} or {"op":"call","name":...,"arguments":{...}}
pub fn mcp_request(core: &Arc<Core>, token: &str, req: &Value) -> Value {
    match req["op"].as_str() {
        // CLIs ask for the list once, at start, usually before the node joins a team, and this relay cannot
        // announce a change: every canvas node sees all tools and each call is checked against its role
        Some("list") if whois(core, token).is_some() => json!({ "tools": ALL_TOOLS.into_iter().map(tool_spec).collect::<Vec<_>>() }),
        Some("list") => json!({ "tools": [] }),
        Some("call") => {
            let name = req["name"].as_str().unwrap_or("");
            // Only what this node may use now: a member asking for a lead tool gets an error, not a silent no-op
            if !tool_names(core, token).contains(&name) {
                let why = if ALL_TOOLS.contains(&name) { "only the team lead can use it, or you are not in a team" } else { "unknown tool" };
                return json!({ "ok": false, "text": format!("The tool {} is not available to you: {why}", js_prefix(name, 40)) });
            }
            match mcp_call(core, token, name, &req["arguments"]) {
                Ok(text) => json!({ "ok": true, "text": text }),
                Err(text) => json!({ "ok": false, "text": text }),
            }
        }
        _ => json!({ "ok": false, "text": "bad request" }),
    }
}

fn whois(core: &Arc<Core>, token: &str) -> Option<(Arc<Repo>, String, String)> {
    let app = app(core).ok()?;
    let pty = app.hooks.lock().unwrap().get(token).cloned()?;
    app.ptys.lock().unwrap().get(&pty).cloned()
}

/// One tool call from the node that owns `token`. Ok is the text the agent sees; Err is a tool error, also shown.
pub fn mcp_call(core: &Arc<Core>, token: &str, tool: &str, args: &Value) -> Result<String, String> {
    let (repo, ch, me) = whois(core, token).ok_or("This session is not an Agent Workbench canvas node")?;
    let a = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let result = (|| -> R<String> {
        // Kept as a guard, so the git-running tools can really let go of the lock before they run git
        let mut guard = repo.st.lock().unwrap();
        let st = &mut *guard;
        let c = st.channels.get(&ch).ok_or_else(|| err("not-found", "channel"))?;
        let team = team_of(c, &me).cloned();
        let lead_only = |t: &Option<Team>| match t {
            Some(t) if t.lead_id == me => Ok(t.clone()),
            Some(_) => Err(err("not-allowed", "Only the team lead can do that")),
            None => Err(err("not-allowed", "You are not in a team")),
        };
        match tool {
            "team_info" => {
                let t = team.ok_or_else(|| err("not-allowed", "You are not in a team"))?;
                let members: Vec<String> = t
                    .member_ids
                    .iter()
                    .map(|m| {
                        let task = c.tasks.get(m).map(|k| format!("{} [{}]", k.title, k.status)).unwrap_or_else(|| "no task".into());
                        let role = if *m == t.lead_id { "lead" } else { "member" };
                        let status = st.rt.get(&key(&ch, m)).map(|r| r.status).unwrap_or("exited");
                        format!("- {} {role}, {status}, task: {task}", display(c, m))
                    })
                    .collect();
                Ok(format!("Team \"{}\" ({}), relay mode {}.\nYou are {} ({}).\nGoal:\n{}\nMembers:\n{}", t.name, t.status, t.relay, me, if t.lead_id == me { "lead" } else { "member" }, t.goal, members.join("\n")))
            }
            "read_inbox" => {
                let msgs = st.msgs.entry(ch.clone()).or_default();
                let mut out = Vec::new();
                for m in msgs.iter_mut().filter(|m| m.to == me && matches!(m.state.as_str(), "queued" | "notified")) {
                    let from = if m.from == "app" { "Agent Workbench".to_string() } else { display(c, &m.from) };
                    out.push(format!("--- {} from {from} ({}):\n{}", m.kind, m.id, m.text));
                    m.state = "read".into();
                }
                st.nudged.remove(&key(&ch, &me));
                save_messages(&repo, &ch, msgs);
                Ok(if out.is_empty() { "No new messages.".into() } else { out.join("\n\n") })
            }
            "send_message" => {
                let t = team.ok_or_else(|| err("not-allowed", "You are not in a team"))?;
                let to = a("to");
                if !route_ok(&t, &c.edges, &me, &to) {
                    return Err(err("not-allowed", format!("You cannot message {to}: talk to your lead, or ask the user to connect you")));
                }
                let text = a("text");
                if text.trim().is_empty() {
                    return Err(err("invalid", "Empty message"));
                }
                post(st, &repo, &ch, &t.id, &me, &to, "message", &text)
            }
            "assign_task" => {
                let t = lead_only(&team)?;
                let member = a("member");
                if member == me || !t.member_ids.contains(&member) {
                    return Err(err("invalid", format!("{member} is not a member of your team")));
                }
                let title = js_prefix(&sanitize_text(&a("title"), false), 200);
                if title.trim().is_empty() {
                    return Err(err("invalid", "A task needs a title"));
                }
                let r = post(st, &repo, &ch, &t.id, &me, &member, "task", &task_text(&title, &a("details")))?;
                let c = st.channels.get_mut(&ch).unwrap();
                c.tasks.insert(member.clone(), NodeTask { title, status: "working".into(), summary: None, updated_at: now_ms(), source: "assign".into() });
                push_event(c, json!({ "t": "task", "at": now_ms(), "team": t.id, "node": member, "status": "working" }));
                let _ = repo.save(c);
                Ok(r)
            }
            "report" => {
                let status = a("status");
                if !["working", "blocked", "done", "failed"].contains(&status.as_str()) {
                    return Err(err("invalid", "status is working, blocked, done or failed"));
                }
                let summary = js_prefix(&sanitize_text(&a("summary"), true), MAX_SUMMARY);
                let c = st.channels.get_mut(&ch).unwrap();
                let title = c.tasks.get(&me).map(|k| k.title.clone()).unwrap_or_else(|| "Task".into());
                c.tasks.insert(me.clone(), NodeTask { title, status: status.clone(), summary: Some(summary.clone()).filter(|s| !s.is_empty()), updated_at: now_ms(), source: "report".into() });
                push_event(c, json!({ "t": "task", "at": now_ms(), "node": me, "status": status }));
                let mut reply = "Report saved.".to_string();
                if let Some(t) = &team {
                    if t.lead_id == me && status == "done" {
                        if let Some(t) = c.teams.iter_mut().find(|x| x.id == t.id) {
                            t.status = "done".into();
                        }
                        push_event(c, json!({ "t": "team-done", "at": now_ms(), "team": t.id, "by": "lead" }));
                        reply = "Report saved. The team is marked done; the user reviews and merges your worktree.".into();
                    } else if t.lead_id != me && status != "working" {
                        let _ = repo.save(c);
                        post(st, &repo, &ch, &t.id, &me, &t.lead_id, "report", &format!("[{status}] {summary}"))?;
                        reply = "Report saved and sent to your lead.".into();
                    }
                }
                if let Some(c) = st.channels.get(&ch) {
                    let _ = repo.save(c);
                }
                Ok(reply)
            }
            "get_member_changes" => {
                let t = lead_only(&team)?;
                let member = a("member");
                if !t.member_ids.contains(&member) {
                    return Err(err("invalid", format!("{member} is not in your team")));
                }
                let w = c.nodes.iter().find(|n| n.id == member).and_then(|n| n.worktree.clone()).ok_or_else(|| err("invalid", "That member works in the repository root"))?;
                drop(guard);
                let dir = Path::new(&w.path);
                let range = format!("{}..HEAD", w.base_commit);
                let commits = g::git(dir, &["log", "--format=%h %s", "-n", "50", &range]).unwrap_or_default();
                let numstat = g::git(dir, &["diff", "--numstat", "--no-renames", &w.base_commit, "HEAD"]).unwrap_or_default();
                let dirty = g::git(dir, &["status", "--porcelain", "-uall"]).map(|s| s.lines().count()).unwrap_or(0);
                let text = format!(
                    "Member {member}, branch {}.\nCommits since its base:\n{}\nCommitted files (added/removed lines, path):\n{}\nUncommitted files: {dirty}{}",
                    w.branch,
                    if commits.trim().is_empty() { "(none)".into() } else { commits.trim_end().to_string() },
                    if numstat.trim().is_empty() { "(none)".into() } else { numstat.trim_end().to_string() },
                    if dirty > 0 { " (ask the member to commit them; request_merge takes committed work only)" } else { "" }
                );
                return Ok(js_prefix(&sanitize_text(&text, true), MAX_TEXT));
            }
            "request_merge" => {
                let t = lead_only(&team)?;
                let member = a("member");
                if member == me || !t.member_ids.contains(&member) {
                    return Err(err("invalid", format!("{member} is not a member of your team")));
                }
                let node = |id: &str| c.nodes.iter().find(|n| n.id == id).and_then(|n| n.worktree.clone());
                let (Some(lw), Some(mw)) = (node(&me), node(&member)) else { return Err(err("invalid", "Both you and the member need your own worktrees")) };
                let auto = t.relay == "auto";
                drop(guard);
                let token = wt::merge_preview(Path::new(&lw.path), &mw, &lw.branch).map_err(|e| err("invalid", merge_reason(&e)))?;
                let commits = g::git(Path::new(&mw.path), &["log", "--format=%h %s", "-n", "50", &format!("{}..{}", token.target_head, token.source)]).unwrap_or_default().lines().map(str::to_string).collect();
                let r = MergeReq { channel: ch.clone(), team: t.id.clone(), lead: me.clone(), lead_dir: lw.path.clone(), member: member.clone(), token, commits, at: now_ms() };
                if auto {
                    // The user let this team's lead decide (relay mode auto)
                    let text = merge_into_lead(&r).map_err(|e| err("invalid", e))?;
                    let mut st = repo.st.lock().unwrap();
                    if let Some(c) = st.channels.get_mut(&ch) {
                        push_event(c, json!({ "t": "team-merge", "at": now_ms(), "team": t.id, "member": member, "decision": "auto" }));
                        let _ = repo.save(c);
                    }
                    return Ok(text);
                }
                let id = short_id();
                let mut st = repo.st.lock().unwrap();
                if let Some(c) = st.channels.get_mut(&ch) {
                    push_event(c, json!({ "t": "team-merge", "at": now_ms(), "team": t.id, "member": member, "decision": "requested", "id": id }));
                    let _ = repo.save(c);
                }
                st.merge_reqs.insert(id.clone(), r);
                return Ok(format!("Merge request {id} is waiting for the user. You will get a message with the result."));
            }
            _ => Err(err("invalid", format!("Unknown tool {}", js_prefix(tool, 40)))),
        }
    })();
    emit(core, &repo, &ch);
    deliver(core, &repo, &ch);
    result.map_err(|e| e.message)
}

fn merge_reason(e: &str) -> String {
    if e == "dirty" {
        "a worktree has uncommitted changes; commit them first".into()
    } else {
        e.to_string()
    }
}

/// worktree.rs merges into whatever checkout it is given; the lead's worktree is a checkout of the same repo, so
/// every data-safety check of a normal merge (clean target, owned source, guard, ignored files) applies here too
fn merge_into_lead(r: &MergeReq) -> Result<String, String> {
    match wt::merge(Path::new(&r.lead_dir), &r.token) {
        Ok(commit) => Ok(format!("Merged {} ({} commit{}) into your worktree as {}.", r.member, r.commits.len(), if r.commits.len() == 1 { "" } else { "s" }, &commit[..commit.len().min(12)])),
        Err(e) if e.starts_with("conflict") => Err(format!("{}: nothing was changed. Ask {} to rebase or resolve, then try again.", merge_reason(&e), r.member)),
        Err(e) => Err(merge_reason(&e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn team(relay: &str) -> Team {
        Team {
            id: "t".into(),
            name: "n".into(),
            goal: "g".into(),
            lead_id: "L".into(),
            member_ids: vec!["L".into(), "A".into(), "B".into()],
            relay: relay.into(),
            status: "running".into(),
            paused_reason: None,
            bounds: Bounds { x: 0.0, y: 0.0, width: 100.0, height: 100.0 },
            limits: Limits::default(),
            started_at: Some(0.0),
            message_count: 0,
        }
    }

    fn msg(from: &str, to: &str, kind: &str, at: f64) -> Msg {
        Msg { id: short_id(), team_id: "t".into(), from: from.into(), to: to.into(), kind: kind.into(), text: "x".into(), at, state: "read".into() }
    }

    #[test]
    fn routes_follow_the_lead_and_drawn_lines() {
        let t = team("auto");
        assert!(route_ok(&t, &[], "L", "A") && route_ok(&t, &[], "A", "L"));
        assert!(!route_ok(&t, &[], "A", "B"), "members need a line");
        let e = Edge { id: "e".into(), from: "B".into(), to: "A".into(), at: 0.0, text: String::new() };
        assert!(route_ok(&t, &[e], "A", "B"), "either direction of a drawn line");
        assert!(!route_ok(&t, &[], "L", "L") && !route_ok(&t, &[], "L", "X") && !route_ok(&t, &[], "X", "L"));
    }

    #[test]
    fn relay_modes_hold_the_right_messages() {
        assert!(needs_approval(&team("approve"), "L") && needs_approval(&team("approve"), "A"));
        assert!(!needs_approval(&team("lead-auto"), "L") && needs_approval(&team("lead-auto"), "A"));
        assert!(!needs_approval(&team("auto"), "A"));
        for relay in RELAYS {
            assert!(!needs_approval(&team(relay), "app") && !needs_approval(&team(relay), "user"), "{relay}");
        }
    }

    #[test]
    fn caps_apply_only_without_the_user() {
        let now = 1e9;
        let mut t = team("approve");
        t.message_count = 10_000;
        assert_eq!(limit_hit(&t, &[], "A", "L", now), None, "approve mode has the user on every message");
        t.relay = "auto".into();
        assert!(limit_hit(&t, &[], "A", "L", now).unwrap().contains("limit"));
        t.message_count = 0;
        let burst: Vec<Msg> = (0..20).map(|i| msg("A", "L", "report", now - i as f64 * 1000.0)).collect();
        assert!(limit_hit(&t, &burst, "A", "L", now).unwrap().contains("10 minutes"));
        let old: Vec<Msg> = (0..20).map(|i| msg("A", "L", "report", now - 11.0 * 60_000.0 - i as f64)).collect();
        assert_eq!(limit_hit(&t, &old, "A", "L", now), None, "only the last 10 minutes count");
        assert_eq!(limit_hit(&t, &burst, "app", "L", now), None, "the app's own messages never count");
    }

    #[test]
    fn ping_pong_pauses_but_a_report_resets_it() {
        let t = team("auto");
        let mut m: Vec<Msg> = (0..9).map(|i| if i % 2 == 0 { msg("L", "A", "message", i as f64) } else { msg("A", "L", "message", i as f64) }).collect();
        assert!(limit_hit(&t, &m, "L", "A", 100.0).unwrap().contains("keep messaging"));
        m[8].kind = "report".into();
        assert_eq!(limit_hit(&t, &m, "L", "A", 100.0), None);
        let mixed: Vec<Msg> = (0..9).map(|i| if i == 4 { msg("L", "B", "task", i as f64) } else { msg("L", "A", "message", i as f64) }).collect();
        assert_eq!(limit_hit(&t, &mixed, "L", "A", 100.0), None, "talking to someone else is not ping-pong");
    }

    #[test]
    fn team_shapes_are_validated() {
        let n = |id: &str| Node { id: id.into(), agent: "claude".into(), worktree: None, position: Pos { x: 0.0, y: 0.0 }, size: Size { width: 640.0, height: 400.0 } };
        let mut c = Channel { schema_version: 1, id: "c".into(), name: "x".into(), nodes: vec![n("L"), n("A"), n("B")], edges: vec![], events: vec![], teams: vec![team("auto")], tasks: Default::default() };
        assert!(teams_ok(&c));
        c.teams[0].lead_id = "Z".into();
        assert!(!teams_ok(&c), "lead must be a member");
        c.teams[0].lead_id = "L".into();
        c.teams[0].member_ids.push("Q".into());
        assert!(!teams_ok(&c), "members must be nodes");
        c.teams[0].member_ids.pop();
        let mut second = team("auto");
        second.id = "t2".into();
        second.member_ids = vec!["A".into()];
        second.lead_id = "A".into();
        c.teams.push(second);
        assert!(!teams_ok(&c), "a node is in at most one team");
        c.teams.pop();
        c.teams[0].relay = "yolo".into();
        assert!(!teams_ok(&c));
    }

    #[test]
    fn leaving_the_canvas_leaves_the_team() {
        let n = |id: &str| Node { id: id.into(), agent: "claude".into(), worktree: None, position: Pos { x: 0.0, y: 0.0 }, size: Size { width: 640.0, height: 400.0 } };
        let mut c = Channel { schema_version: 1, id: "c".into(), name: "x".into(), nodes: vec![n("L"), n("A")], edges: vec![], events: vec![], teams: vec![team("auto")], tasks: Default::default() };
        c.teams[0].member_ids = vec!["L".into(), "A".into()];
        forget_node(&mut c, "L");
        assert_eq!(c.teams[0].lead_id, "A");
        assert_eq!(c.teams[0].status, "paused");
        forget_node(&mut c, "A");
        assert!(c.teams.is_empty());
    }

    #[test]
    fn kickoff_tells_the_lead_the_tools_and_the_members() {
        let n = |id: &str| Node { id: id.into(), agent: "codex".into(), worktree: None, position: Pos { x: 0.0, y: 0.0 }, size: Size { width: 640.0, height: 400.0 } };
        let c = Channel { schema_version: 1, id: "c".into(), name: "x".into(), nodes: vec![n("L"), n("A"), n("B")], edges: vec![], events: vec![], teams: vec![], tasks: Default::default() };
        let k = kickoff(&c, &team("auto"));
        assert!(k.contains("assign_task") && k.contains("request_merge") && k.contains("report(status=\"done\""));
        assert!(k.contains("- A (codex)") && k.contains("- B (codex)") && !k.contains("- L (codex)"), "the lead is not listed as its own member");
    }
}
