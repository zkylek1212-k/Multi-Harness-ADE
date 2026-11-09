// Cowork's background execution: after approval, tasks go to each agent in a git worktree (.cowork/<runId>/).
// Execution uses the user's full setup and bypass setting. Progress and replies are written back to the run, and a follow-up
// continues the same session. Completion is decided here (git, checks, reported acceptance), never by what the agent claims.
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use regex::{Regex, RegexBuilder};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use wb_git as g;
use wb_proc::{resolve_command, CancelToken, Resolved, RunSpec};

use crate::cowork::{add_num, cerr, cerr_d, cerr_m, diff_guard, exclude_cowork_dir, log_push, run_proc, CoworkError, ProcHooks, Run, Service, CR};
use crate::cowork_shared as sh;
use crate::util::{js_prefix, p, sv};

const TURN_TIMEOUT_MS: f64 = 30.0 * 60_000.0;
const MAX_PROGRESS: usize = 40;
const MAX_TEXT: usize = 40_000;
const MAX_WATCH: usize = 20;
/// ponytail: a guess at what one turn's tokens look like when something is wrong. Logged only; calibrate with real logs.
const TOKEN_SPIKE: f64 = 20_000_000.0;
const CHECK_TIMEOUT_MS: f64 = 10.0 * 60_000.0;
const CHECK_MAX_BYTES: usize = 8 * 1024 * 1024;
const KEEP_OUTPUT: usize = 8000;

pub struct StartOpts {
    pub mode: String,
    pub link_deps: bool,
    pub bypass: bool,
    pub approval: Option<String>,
    pub review: bool,
    pub worktree_root: Option<String>,
    pub runaway_stop: bool,
}

#[derive(Default)]
pub struct ExecState {
    active: Mutex<HashMap<String, u32>>,
    finishing: Mutex<HashSet<String>>,
    /// A second merge request with the same approval waits for the first and shares its answer
    merging: Mutex<HashMap<String, Arc<Mutex<Option<Result<(), CoworkError>>>>>>,
}

fn del(o: &mut Value, k: &str) {
    if let Some(m) = o.as_object_mut() {
        m.shift_remove(k);
    }
}

fn now(svc: &Service) -> i64 {
    svc.now() as i64
}

fn task_of<'a>(board: &'a Value, id: &str) -> Option<&'a Value> {
    board["tasks"].as_array()?.iter().find(|t| sv(t, "id") == id)
}

fn budget_left(svc: &Service, r: &Value) -> f64 {
    let ex = &r["execution"];
    let used = ex["msUsed"].as_f64().unwrap_or(0.0)
        + match ex["activeSince"].as_f64() {
            Some(s) if s != 0.0 => svc.now() - s,
            _ => 0.0,
        };
    r["limits"]["maxExecutionMinutes"].as_f64().unwrap_or(0.0) * 60_000.0 - used
}

fn clock_start(svc: &Service, r: &mut Value) {
    let id = sv(r, "id").to_string();
    let n = {
        let mut a = svc.exec.active.lock().unwrap();
        let n = a.entry(id).or_insert(0);
        *n += 1;
        *n
    };
    if n == 1 {
        r["execution"]["activeSince"] = json!(now(svc));
    }
}

fn clock_stop(svc: &Service, r: &mut Value) {
    let id = sv(r, "id").to_string();
    let n = {
        let mut a = svc.exec.active.lock().unwrap();
        let n = a.entry(id).or_insert(1);
        *n = n.saturating_sub(1);
        *n
    };
    if n == 0 {
        if let Some(s) = r["execution"]["activeSince"].as_f64().filter(|s| *s != 0.0) {
            let d = svc.now() - s;
            add_num(&mut r["execution"]["msUsed"], d);
        }
        r["execution"]["activeSince"] = Value::Null;
    }
}

/// Called every second: the execution budget, only for runs with an agent running
pub fn tick(svc: &Service) {
    let ids: Vec<String> = svc.exec.active.lock().unwrap().iter().filter(|(_, n)| **n > 0).map(|(k, _)| k.clone()).collect();
    for id in ids {
        let Some(run) = svc.get_run(&id) else { continue };
        let mut r = run.lock().unwrap();
        if r["execution"].is_object() && budget_left(svc, &r) <= 0.0 && r["execution"]["paused"] != json!(true) {
            pause_locked(svc, &mut r, "budget");
        }
    }
}

pub fn is_running(svc: &Service, id: &str) -> bool {
    svc.exec.active.lock().unwrap().get(id).copied().unwrap_or(0) > 0
}

fn cloud_dir() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| RegexBuilder::new("onedrive|dropbox|google ?drive|我的雲端硬碟|icloud").case_insensitive(true).build().unwrap())
}

fn worktree_base(r: &Value, custom: Option<&str>) -> String {
    match custom.map(str::trim).filter(|c| !c.is_empty() && Path::new(c).is_absolute()) {
        Some(c) => crate::util::resolve(c),
        None => p(&Path::new(r["repo"]["root"].as_str().unwrap_or("")).join(".cowork")),
    }
}

/// What the user sees before starting. The hash binds it: if any of it changes, an old approval does not apply.
pub fn preview(_svc: &Service, r: &Value, mode: &str, link_deps: bool, bypass: bool, root: Option<&str>) -> CR<Value> {
    if sv(r, "phase") != "approved" {
        return Err(cerr("not-allowed"));
    }
    let Some(board) = sh::current_board(r).filter(|b| b["planRevision"] == r["approvedPlanRevision"]) else { return Err(cerr("not-allowed")) };
    let mut list: Vec<(Value, Vec<Value>)> = Vec::new();
    for t in board["tasks"].as_array().into_iter().flatten() {
        for argv in t["checks"].as_array().into_iter().flatten() {
            list.push((t["id"].clone(), argv.as_array().cloned().unwrap_or_default()));
        }
    }
    for argv in board["checks"].as_array().into_iter().flatten() {
        list.push((Value::Null, argv.as_array().cloned().unwrap_or_default()));
    }
    let repo_root = r["repo"]["root"].as_str().unwrap_or("").to_string();
    let mut checks = Vec::new();
    for (task, argv) in &list {
        let cmd = argv.first().and_then(Value::as_str).unwrap_or("");
        let mut c = json!({ "taskId": task, "argv": argv, "resolved": null });
        match resolve_command(cmd, Path::new(&repo_root)) {
            Resolved::Path(x) => c["resolved"] = json!(x),
            Resolved::Error(e) => c["error"] = json!(e),
        }
        checks.push(c);
    }
    let base = worktree_base(r, root);
    let mut warnings = Vec::new();
    if bypass {
        warnings.push("bypass-on");
    }
    if link_deps {
        warnings.push("shared-deps");
    }
    if !list.is_empty() {
        warnings.push("checks-run-code");
    }
    if cloud_dir().is_match(&base) {
        warnings.push("cloud-worktree");
    }
    let resolved: Vec<&Value> = checks.iter().map(|c| &c["resolved"]).collect();
    let content = format!("{}{}{base}", sh::exec_approval_content(r, board, mode, link_deps, bypass), json!(resolved));
    let hash: String = Sha256::digest(content.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    Ok(json!({ "hash": hash, "mode": mode, "linkDeps": link_deps, "bypass": bypass, "baseCommit": r["repo"]["baseCommit"], "checks": checks, "worktreeRoot": base, "warnings": warnings }))
}

pub fn start(svc: &Service, run: &Run, o: &StartOpts) -> CR<()> {
    let (seen, board, common, root_repo, base_commit, id, data_dir) = {
        let r = run.lock().unwrap();
        if sv(&r, "phase") != "approved" {
            return Err(cerr("not-allowed"));
        }
        let Some(board) = sh::current_board(&r).filter(|b| b["planRevision"] == r["approvedPlanRevision"]).cloned() else { return Err(cerr("not-allowed")) };
        if sh::topo_order(board["tasks"].as_array().unwrap()).is_none() {
            return Err(cerr_m("not-allowed", "dependency cycle"));
        }
        let common = r["repo"]["commonDir"].as_str().unwrap_or("").to_string();
        let id = sv(&r, "id").to_string();
        (preview(svc, &r, &o.mode, o.link_deps, o.bypass, o.worktree_root.as_deref())?, board, common, r["repo"]["root"].as_str().unwrap_or("").to_string(), r["repo"]["baseCommit"].as_str().unwrap_or("").to_string(), id, svc.dir_of(&r))
    };
    if let Some(other) = svc.other_active_run(&id, &common) {
        return Err(cerr_d("active-run-exists", None, json!({ "runId": other })));
    }
    // Re-checked inside the exclusive section: changed bypass, board or checks void the old approval
    if o.approval.as_deref() != seen["hash"].as_str() {
        return Err(cerr_d("stale", Some("The plan or its settings changed. Review them again.".into()), json!({ "preview": seen })));
    }
    let missing: Vec<String> = seen["checks"].as_array().unwrap().iter().filter(|c| c.get("error").is_some()).map(|c| format!("{}: {}", sh::strs(&c["argv"]).join(" "), sv(c, "error"))).collect();
    if !missing.is_empty() {
        return Err(cerr_m("check-unresolved", missing.join("\n")));
    }
    let base = sv(&seen, "worktreeRoot").to_string();
    let root = p(&Path::new(&base).join(&id));
    let tasks = board["tasks"].as_array().unwrap();
    let keys: Vec<String> = if o.mode == "sequential" {
        vec!["main".into()]
    } else {
        let mut k = Vec::new();
        for t in tasks {
            let a = sv(t, "assignee").to_string();
            if !k.contains(&a) {
                k.push(a);
            }
        }
        k
    };
    let mut worktrees = serde_json::Map::new();
    for k in &keys {
        worktrees.insert(k.clone(), json!({ "path": p(&Path::new(&root).join(k)), "branch": format!("cowork/{id}/{k}") }));
    }
    // A task that changes dependencies means nothing shares node_modules: the junction points at the main workspace, so an install
    // would change the user's environment. The cost is that agents install for themselves.
    let dep_tasks: Vec<String> = tasks.iter().filter(|t| sh::touches_dependencies(&sh::strs(&t["scope"]))).map(|t| sv(t, "id").to_string()).collect();
    let link = o.link_deps && dep_tasks.is_empty();
    exclude_cowork_dir(&common);
    let repo = Path::new(&root_repo);
    let hooks = g::export_baseline_hooks(repo, &base_commit, &data_dir.join("exec").join("hooks")).map_err(|e| cerr_m("worktree-failed", e))?;
    let guard = g::git_guard_state(repo).map_err(|e| cerr_m("worktree-failed", e))?;
    let mut created: Vec<String> = Vec::new();
    for k in &keys {
        let w = worktrees.get_mut(k).unwrap();
        let made = g::add_worktree(repo, Path::new(sv(w, "path")), sv(w, "branch"), &base_commit).and_then(|_| {
            created.push(sv(w, "path").to_string());
            if link {
                g::link_node_modules(repo, Path::new(sv(w, "path")))
            } else {
                Ok(false)
            }
        });
        match made {
            Ok(linked) => w["depsLinked"] = json!(linked),
            Err(e) => {
                // Halfway: remove what was made. The run stays approved.
                let _ = g::remove_worktrees(repo, Path::new(&root), &created, Some(&base));
                for k2 in &keys {
                    let _ = g::git(repo, &["branch", "-D", &format!("cowork/{id}/{k2}")]);
                }
                return Err(cerr_m("worktree-failed", e));
            }
        }
    }
    let mut task_map = serde_json::Map::new();
    for t in tasks {
        task_map.insert(sv(t, "id").to_string(), json!({ "status": "pending", "agent": t["assignee"], "worktree": if o.mode == "sequential" { "main" } else { sv(t, "assignee") }, "turns": [], "commits": [] }));
    }
    {
        let mut r = run.lock().unwrap();
        let mut ex = json!({ "mode": o.mode, "startedAt": now(svc), "bypass": o.bypass, "linkDeps": o.link_deps });
        if o.link_deps && !dep_tasks.is_empty() {
            ex["depsLinkSkippedFor"] = json!(dep_tasks);
        }
        for (k, v) in [("gates", json!(true)), ("reviewOn", json!(o.review)), ("runawayStop", json!(o.runaway_stop)), ("hooksDir", json!(hooks)), ("gitGuard", json!(guard)), ("worktreeBase", json!(base)), ("root", json!(root)), ("worktrees", json!(worktrees)), ("tasks", json!(task_map)), ("paused", json!(false)), ("msUsed", json!(0)), ("activeSince", Value::Null)] {
            ex[k] = v;
        }
        r["execution"] = ex;
        r["phase"] = json!("executing");
        log_push(&mut r, json!({ "t": "exec-start", "mode": o.mode, "bypass": o.bypass, "at": now(svc) }));
        svc.save(&mut r);
    }
    schedule(svc, run);
    Ok(())
}

struct TurnOpts {
    message: Option<String>,
}

/// The part of a turn that must happen with the scheduling decision: mark it running so it cannot start twice
fn mark_turn(svc: &Service, r: &mut Value, id: &str, fresh: bool) {
    let te = &mut r["execution"]["tasks"][id];
    te["status"] = json!("running");
    for k in ["error", "scopeReview", "gate"] {
        del(te, k);
    }
    te["startedAt"] = json!(svc.now() as i64);
    if fresh {
        del(te, "sessionId");
    }
}

fn spawn_turn(svc: &Service, run: &Run, id: String, o: TurnOpts) {
    let (svc, run) = (svc.arc(), run.clone());
    std::thread::spawn(move || run_turn(&svc, &run, &id, o));
}

/// Start what can run: dependencies done, its worktree free, its resources free
pub fn schedule(svc: &Service, run: &Run) {
    let mut starts = Vec::new();
    let finish_now;
    {
        let mut r = run.lock().unwrap();
        if svc.is_shutting_down() || !r["execution"].is_object() {
            return;
        }
        let Some(board) = sh::current_board(&r).cloned() else { return };
        let phase = sv(&r, "phase");
        if phase != "executing" && phase != "review" {
            return;
        }
        if r["execution"]["paused"] != json!(true) {
            let tasks = board["tasks"].as_array().unwrap();
            let order = sh::topo_order(tasks).unwrap_or_else(|| tasks.iter().map(|t| sv(t, "id").to_string()).collect());
            for id in order {
                let (status, wt) = {
                    let te = &r["execution"]["tasks"][&id];
                    (sv(te, "status").to_string(), sv(te, "worktree").to_string())
                };
                let Some(task) = task_of(&board, &id) else { continue };
                if status != "pending" {
                    continue;
                }
                let map = r["execution"]["tasks"].as_object().unwrap();
                if !sh::strs(&task["dependsOn"]).iter().all(|d| map.get(d).map(|t| sv(t, "status") == "done").unwrap_or(false)) {
                    continue;
                }
                if map.values().any(|x| sv(x, "status") == "running" && sv(x, "worktree") == wt) {
                    continue;
                }
                let res = sh::strs(&task["resources"]);
                if tasks.iter().any(|o| map.get(sv(o, "id")).map(|t| sv(t, "status") == "running").unwrap_or(false) && res.iter().any(|x| sh::strs(&o["resources"]).contains(x))) {
                    continue;
                }
                if budget_left(svc, &r) <= 0.0 {
                    pause_locked(svc, &mut r, "budget");
                    drop(r);
                    for (id, o) in starts {
                        spawn_turn(svc, run, id, o);
                    }
                    return;
                }
                mark_turn(svc, &mut r, &id, false);
                starts.push((id, TurnOpts { message: None }));
            }
        }
        let map = r["execution"]["tasks"].as_object().unwrap();
        finish_now = !map.is_empty() && map.values().all(|t| sv(t, "status") == "done");
    }
    for (id, o) in starts {
        spawn_turn(svc, run, id, o);
    }
    if finish_now {
        let (s, r2) = (svc.arc(), run.clone());
        std::thread::spawn(move || finish(&s, &r2));
    }
}

fn add_usage(a: &Value, b: &Value) -> Value {
    let n = |v: &Value, k: &str| v[k].as_f64().unwrap_or(0.0);
    let i = |x: f64| json!(x as i64);
    let mut o = json!({ "inputTokens": i(n(a, "inputTokens") + n(b, "inputTokens")), "cachedInputTokens": i(n(a, "cachedInputTokens") + n(b, "cachedInputTokens")), "outputTokens": i(n(a, "outputTokens") + n(b, "outputTokens")) });
    let cost = n(a, "costUsd") + n(b, "costUsd");
    if cost != 0.0 {
        o["costUsd"] = json!(cost);
    }
    o
}

/// One round: first handover, a follow-up, or continuing after an interruption
fn run_turn(svc: &Service, run: &Run, id: &str, o: TurnOpts) {
    let (wt_path, wt_branch, wt_linked, agent, followup, hooks, mode, board, task) = {
        let r = run.lock().unwrap();
        let ex = &r["execution"];
        let te = &ex["tasks"][id];
        let wt = &ex["worktrees"][sv(te, "worktree")];
        let board = sh::current_board(&r).unwrap().clone();
        let task = task_of(&board, id).unwrap().clone();
        let followup = o.message.is_some() && !sv(te, "sessionId").is_empty();
        (sv(wt, "path").to_string(), sv(wt, "branch").to_string(), wt.get("depsLinked").and_then(Value::as_bool), sv(te, "agent").to_string(), followup, ex.get("hooksDir").and_then(Value::as_str).map(str::to_string), sv(ex, "mode").to_string(), board, task)
    };
    // In parallel mode, prerequisites finished in another worktree get applied first
    if mode == "parallel" && !followup {
        let commits = {
            let r = run.lock().unwrap();
            let tasks = board["tasks"].as_array().unwrap();
            let order = sh::topo_order(tasks).unwrap_or_default();
            let mut needed: HashSet<String> = HashSet::new();
            fn walk(board: &Value, id: &str, needed: &mut HashSet<String>) {
                for d in task_of(board, id).map(|t| sh::strs(&t["dependsOn"])).unwrap_or_default() {
                    if needed.insert(d.clone()) {
                        walk(board, &d, needed);
                    }
                }
            }
            walk(&board, id, &mut needed);
            let map = &r["execution"]["tasks"];
            let mine = sv(&map[id], "worktree");
            order.iter().filter(|x| needed.contains(*x) && sv(&map[x.as_str()], "worktree") != mine).flat_map(|x| sh::strs(&map[x.as_str()]["commits"])).collect::<Vec<_>>()
        };
        if !commits.is_empty() {
            let res = g::cherry_pick_all(Path::new(&wt_path), &commits, hooks.as_deref());
            let conflict = match res {
                Ok(g::PickResult::Conflict { commit, message, .. }) => Some((commit, message)),
                Ok(_) => None,
                Err(e) => Some((String::new(), e)),
            };
            if let Some((c, m)) = conflict {
                {
                    let mut r = run.lock().unwrap();
                    let err = format!("Could not apply prerequisite commit {}: {m}", js_prefix(&c, 10));
                    let te = &mut r["execution"]["tasks"][id];
                    te["status"] = json!("blocked");
                    te["error"] = json!(err);
                    te["endedAt"] = json!(now(svc));
                    log_push(&mut r, json!({ "t": "task-failed", "taskId": id, "message": err, "at": now(svc) }));
                    svc.save(&mut r);
                }
                schedule(svc, run);
                return;
            }
        }
    }
    let (prompt, turn_index, gates, bypass) = {
        let mut r = run.lock().unwrap();
        let ex = r["execution"].clone();
        let deps: Vec<Value> = sh::strs(&task["dependsOn"])
            .iter()
            .map(|d| {
                let dt = task_of(&board, d);
                let last = ex["tasks"][d]["turns"].as_array().into_iter().flatten().rev().find(|t| sv(t, "role") == "agent" && !sv(t, "text").is_empty());
                let mut row = json!({ "id": d, "title": dt.map(|t| sv(t, "title")).filter(|t| !t.is_empty()).unwrap_or(d), "summary": last.map(|t| sv(t, "text")).unwrap_or("") });
                if let Some(c) = ex["tasks"][d].get("changedFiles") {
                    row["changedFiles"] = c.clone();
                }
                if let Some(c) = ex["tasks"][d].get("result") {
                    row["result"] = c.clone();
                }
                row
            })
            .collect();
        let gates = ex.get("gates").map(crate::settings::truthy).unwrap_or(false);
        // A resumed session only gets the user's words; with no session, hand it over again with them attached
        let prompt = if followup {
            o.message.clone().unwrap()
        } else {
            let base = sh::build_exec_prompt(&r, &task, &wt_branch, &deps, wt_linked, gates);
            match &o.message {
                Some(m) => format!("{base}\n\nAdditional instructions from the user:\n{m}"),
                None => base,
            }
        };
        let at = now(svc);
        let shown = js_prefix(o.message.as_deref().unwrap_or(&prompt), 8000);
        let turns = r["execution"]["tasks"][id]["turns"].as_array_mut().unwrap();
        turns.push(json!({ "role": if o.message.is_some() { "user" } else { "cowork" }, "text": shown, "at": at }));
        turns.push(json!({ "role": "agent", "text": "", "at": at, "progress": [], "running": true }));
        let idx = turns.len() - 1;
        log_push(&mut r, json!({ "t": "task-start", "taskId": id, "agent": agent, "followUp": o.message.is_some(), "at": at }));
        svc.save(&mut r);
        (prompt, idx, gates, ex["bypass"] == json!(true))
    };
    let _ = gates;
    // Registered before resolving the CLI, which may take a moment: a stop during that wait still takes effect below
    let token = CancelToken::default();
    let run_id = run.lock().unwrap()["id"].as_str().unwrap_or("").to_string();
    svc.register_token(&run_id, &token);
    if svc.is_shutting_down() {
        token.cancel();
    }
    let cli = match svc.deps.resolve_exec_cli(&agent) {
        Ok(c) => c,
        Err(e) => {
            svc.unregister_token(&run_id, &token);
            return end_turn(svc, run, id, turn_index, EndTurn::fail(format!("{agent} is not available: {e}")));
        }
    };
    let (out_file, choice) = {
        let r = run.lock().unwrap();
        let n = r["execution"]["tasks"][id]["turns"].as_array().unwrap().len();
        (svc.dir_of(&r).join("exec").join(format!("{id}-{n}.last.txt")), r["models"].get(&agent).cloned().unwrap_or(Value::Null))
    };
    if let Some(d) = out_file.parent() {
        let _ = fs::create_dir_all(d);
    }
    let resume = if followup { Some(sv(&run.lock().unwrap()["execution"]["tasks"][id], "sessionId").to_string()) } else { None };
    let model = Some(sv(&choice, "model")).filter(|m| !m.is_empty());
    let effort = Some(sv(&choice, "effort")).filter(|m| !m.is_empty());
    let inv = sh::exec_invocation(&agent, &wt_path, resume.as_deref(), model, effort, bypass, &p(&out_file));
    let final_box: Mutex<Option<(bool, String, Option<String>)>> = Mutex::new(None);
    let last_text = Mutex::new(String::new());
    let monitor = Mutex::new(sh::RunawayMonitor::new(svc.now()));
    let timeout = {
        let mut r = run.lock().unwrap();
        clock_start(svc, &mut r);
        budget_left(svc, &r).min(TURN_TIMEOUT_MS).max(60_000.0) as u64
    };
    let spec = RunSpec {
        command: cli.command.clone(),
        args: cli.prefix_args.iter().cloned().chain(inv.args.iter().cloned()).collect(),
        cwd: wt_path.clone(),
        stdin: if inv.stdin_ndjson { sh::agy_stdin(&prompt) } else { prompt.clone() },
        timeout_ms: timeout,
        max_bytes: 32 * 1024 * 1024,
        env: cli.env.as_ref().map(|e| std::env::vars().chain(e.iter().cloned()).collect()),
    };
    let mut on_activity = |_: usize| {};
    let mut on_spawn = || {};
    let mut on_line = |line: &str| -> Option<String> {
        let ev = sh::parse_exec_line(&agent, line, &wt_path)?;
        let mut r = run.lock().unwrap();
        let at = now(svc);
        {
            let te = &mut r["execution"]["tasks"][id];
            if let Some(s) = &ev.session_id {
                te["sessionId"] = json!(s);
            }
            let turn = &mut te["turns"][turn_index];
            if let Some(pr) = &ev.progress {
                let list = turn["progress"].as_array_mut().unwrap();
                list.push(json!(pr));
                let n = list.len();
                if n > MAX_PROGRESS {
                    list.drain(0..n - MAX_PROGRESS);
                }
            }
            if let Some(t) = &ev.text {
                *last_text.lock().unwrap() = t.clone();
                turn["text"] = json!(js_prefix(t, MAX_TEXT));
            }
            if let Some(d) = &ev.text_delta {
                let joined = format!("{}{d}", sv(turn, "text"));
                let chars: Vec<char> = joined.chars().collect();
                let start = chars.len().saturating_sub(MAX_TEXT);
                turn["text"] = json!(chars[start..].iter().collect::<String>());
            }
        }
        if let Some(u) = &ev.usage {
            let cur = r["execution"]["tasks"][id].get("usage").cloned().unwrap_or(json!({}));
            r["execution"]["tasks"][id]["usage"] = add_usage(&cur, u);
            let used = u["inputTokens"].as_f64().unwrap_or(0.0) + u["outputTokens"].as_f64().unwrap_or(0.0);
            if used >= TOKEN_SPIKE {
                log_push(&mut r, json!({ "t": "runaway", "taskId": id, "signal": "token-spike", "detail": format!("{used} tokens in one turn"), "stopped": false, "at": at }));
            }
        }
        if let Some(f) = &ev.final_ {
            *final_box.lock().unwrap() = Some(f.clone());
        }
        svc.emit_soon(&r);
        // Observation by default; only stops once the user opted in
        let stop_on = r["execution"]["runawayStop"] == json!(true);
        let mut stop = None;
        for sig in monitor.lock().unwrap().feed(&ev.tools, &ev.tool_results, svc.now()) {
            let will = sig.can_stop && stop_on;
            let te = &mut r["execution"]["tasks"][id];
            if !te["watch"].is_array() {
                te["watch"] = json!([]);
            }
            let w = te["watch"].as_array_mut().unwrap();
            w.push(json!({ "signal": sig.signal, "detail": sig.detail, "at": at }));
            let n = w.len();
            if n > MAX_WATCH {
                w.drain(0..n - MAX_WATCH);
            }
            log_push(&mut r, json!({ "t": "runaway", "taskId": id, "signal": sig.signal, "detail": sig.detail, "stopped": will, "at": at }));
            if will {
                stop = Some(sig.detail.clone());
            }
        }
        stop
    };
    let result = run_proc(&spec, &token, ProcHooks { on_activity: &mut on_activity, on_line: &mut on_line, on_spawn: &mut on_spawn }, |pid| svc.deps.on_pid(pid));
    svc.unregister_token(&run_id, &token);
    {
        let mut r = run.lock().unwrap();
        clock_stop(svc, &mut r);
    }
    if let Some(g) = &result.guard_reason {
        return end_turn(svc, run, id, turn_index, EndTurn { ok: false, error: Some(format!("Stopped by Cowork: {g}")), blocked: true, ..Default::default() });
    }
    if result.cancelled {
        return end_turn(svc, run, id, turn_index, EndTurn { ok: false, error: Some("stopped".into()), cancelled: true, ..Default::default() });
    }
    if let Some(e) = &result.spawn_error {
        return end_turn(svc, run, id, turn_index, EndTurn::fail(format!("Could not start {agent}: {e}")));
    }
    if result.timed_out {
        return end_turn(svc, run, id, turn_index, EndTurn::fail(format!("{agent} did not finish within the time limit")));
    }
    let f = final_box.lock().unwrap().clone();
    if let Some((true, text, _)) = &f {
        if result.code == Some(0) {
            let out_text = inv.out_file.as_ref().and_then(|x| crate::util::read_text(Path::new(x))).unwrap_or_default();
            let lt = last_text.lock().unwrap().clone();
            let shown = sv(&run.lock().unwrap()["execution"]["tasks"][id]["turns"][turn_index], "text").to_string();
            let full = [text.as_str(), lt.as_str(), out_text.as_str(), shown.as_str()].into_iter().find(|x| !x.is_empty()).unwrap_or("").to_string();
            run.lock().unwrap()["execution"]["tasks"][id]["turns"][turn_index]["text"] = json!(js_prefix(&full, MAX_TEXT));
            // Parsed from the whole text: the display cut must not lose the block at the end
            return end_turn(svc, run, id, turn_index, EndTurn { ok: true, follow_up: o.message.is_some(), result: sh::parse_exec_result(&full), ..Default::default() });
        }
    }
    let why = f.and_then(|x| x.2).unwrap_or_else(|| {
        if result.code != Some(0) {
            let t = result.stderr.trim();
            let c: Vec<char> = t.chars().collect();
            let tail: String = c[c.len().saturating_sub(400)..].iter().collect();
            format!("exit {}: {tail}", result.code.map(|x| x.to_string()).unwrap_or_else(|| "null".into()))
        } else {
            "no result".to_string()
        }
    });
    end_turn(svc, run, id, turn_index, EndTurn::fail(why));
}

#[derive(Default)]
struct EndTurn {
    ok: bool,
    error: Option<String>,
    cancelled: bool,
    follow_up: bool,
    result: Option<Value>,
    blocked: bool,
}

impl EndTurn {
    fn fail(e: String) -> Self {
        Self { error: Some(e), ..Default::default() }
    }
}

fn wt_of(r: &Value, id: &str) -> String {
    let ex = &r["execution"];
    sv(&ex["worktrees"][sv(&ex["tasks"][id], "worktree")], "path").to_string()
}

fn end_turn(svc: &Service, run: &Run, id: &str, turn: usize, e: EndTurn) {
    let (dir, scope) = {
        let mut r = run.lock().unwrap();
        let at = now(svc);
        {
            let te = &mut r["execution"]["tasks"][id];
            te["turns"][turn]["running"] = json!(false);
            te["endedAt"] = json!(at);
        }
        if !e.ok {
            let msg = e.error.clone().unwrap_or_else(|| "failed".into());
            {
                let te = &mut r["execution"]["tasks"][id];
                te["turns"][turn]["error"] = json!(msg);
                te["status"] = json!(if e.cancelled {
                    "cancelled"
                } else if e.blocked {
                    "blocked"
                } else {
                    "failed"
                });
                te["error"] = json!(msg);
            }
            if !e.cancelled {
                log_push(&mut r, json!({ "t": "task-failed", "taskId": id, "message": msg, "at": at }));
            }
            svc.save(&mut r);
            drop(r);
            schedule(svc, run);
            return;
        }
        // A follow-up with no block keeps the previous result
        if let Some(res) = &e.result {
            r["execution"]["tasks"][id]["result"] = res.clone();
        }
        let board = sh::current_board(&r).unwrap();
        (wt_of(&r, id), sh::strs(&task_of(board, id).unwrap()["scope"]))
    };
    // Before git runs anything else: changed settings could name a filter or driver
    let tampered = guard_changes(run, &dir);
    if !tampered.is_empty() {
        return block_gate(svc, run, id, "git-config", tampered, e.follow_up);
    }
    let files = match g::changed_files(Path::new(&dir)) {
        Ok(f) => f,
        Err(err) => return fail_task(svc, run, id, Some(turn), format!("Could not read the changes: {err}")),
    };
    let outside = sh::files_outside_scope(&scope, &files);
    if !outside.is_empty() {
        // Not committed: the user accepts or reverts, or asks the agent to clean up
        let hash = g::scope_content_hash(Path::new(&dir), &outside).ok();
        hold_scope(svc, run, id, outside, e.follow_up, hash, "Changed");
        return;
    }
    complete_task(svc, run, id, Some(turn), e.follow_up);
}

fn hold_scope(svc: &Service, run: &Run, id: &str, outside: Vec<String>, follow_up: bool, hash: Option<String>, verb: &str) {
    {
        let mut r = run.lock().unwrap();
        let te = &mut r["execution"]["tasks"][id];
        te["status"] = json!("blocked");
        let mut review = json!({ "files": outside, "followUp": follow_up });
        if let Some(h) = hash {
            review["hash"] = json!(h);
        }
        te["scopeReview"] = review;
        let head: Vec<&str> = outside.iter().take(10).map(String::as_str).collect();
        let more = if verb == "Changed" && outside.len() > 10 { ", …" } else { "" };
        let label = if verb == "Changed" { "Changed" } else { "Checks changed" };
        te["error"] = json!(format!("{label} {} file(s) outside the task scope: {}{more}", outside.len(), head.join(", ")));
        log_push(&mut r, json!({ "t": "task-scope", "taskId": id, "files": outside, "at": now(svc) }));
        svc.save(&mut r);
    }
    schedule(svc, run);
}

fn guard_changes(run: &Run, dir: &str) -> Vec<String> {
    let base = run.lock().unwrap()["execution"].get("gitGuard").cloned();
    let Some(base) = base.filter(|b| b.is_object()) else { return Vec::new() };
    match g::git_guard_state(Path::new(dir)) {
        Ok(state) => diff_guard(&base, &json!(state)),
        Err(e) => vec![format!("could not read the git configuration: {e}")],
    }
}

fn block_gate(svc: &Service, run: &Run, id: &str, kind: &str, items: Vec<String>, follow_up: bool) {
    {
        let mut r = run.lock().unwrap();
        let kept: Vec<String> = items.iter().take(50).cloned().collect();
        let head: Vec<&str> = items.iter().take(3).map(String::as_str).collect();
        let at = now(svc);
        {
            let te = &mut r["execution"]["tasks"][id];
            te["status"] = json!("blocked");
            te["gate"] = json!({ "kind": kind, "items": kept, "followUp": follow_up });
            te["error"] = json!(js_prefix(&format!("{kind}: {}", head.join("; ")), 1000));
            te["endedAt"] = json!(at);
        }
        log_push(&mut r, json!({ "t": "task-gate", "taskId": id, "kind": kind, "items": kept, "at": at }));
        svc.save(&mut r);
    }
    schedule(svc, run);
}

/// Not passed: ask once automatically. A second miss stops for the user.
fn gate_follow_up(svc: &Service, run: &Run, id: &str, kind: &str, items: Vec<String>, follow_up: bool, message: String) {
    let go = {
        let mut r = run.lock().unwrap();
        let te = &r["execution"]["tasks"][id];
        let count = te["gateFollowUps"].as_i64().unwrap_or(0);
        if count >= 1 || sv(te, "sessionId").is_empty() || r["execution"]["paused"] == json!(true) {
            false
        } else {
            r["execution"]["tasks"][id]["gateFollowUps"] = json!(count + 1);
            let kept: Vec<&String> = items.iter().take(50).collect();
            log_push(&mut r, json!({ "t": "task-gate", "taskId": id, "kind": kind, "items": kept, "at": now(svc) }));
            mark_turn(svc, &mut r, id, false);
            true
        }
    };
    if !go {
        return block_gate(svc, run, id, kind, items, follow_up);
    }
    spawn_turn(svc, run, id.to_string(), TurnOpts { message: Some(message) });
}

/// Run an approved check: argv without a shell, in the worktree, with a timeout and output cap. It still runs project code.
pub fn run_check(svc: &Service, argv: &[String], cwd: &str, token: &CancelToken, timeout: Option<f64>, tree: Option<&str>) -> Value {
    run_check_with(now(svc), &|pid| svc.deps.on_pid(pid), argv, cwd, token, timeout, tree)
}

pub fn run_check_with(at: i64, on_pid: &dyn Fn(u32), argv: &[String], cwd: &str, token: &CancelToken, timeout: Option<f64>, tree: Option<&str>) -> Value {
    let attach = |mut v: Value| {
        if let Some(t) = tree {
            v["tree"] = json!(t);
        }
        v
    };
    let resolved = match resolve_command(argv.first().map(String::as_str).unwrap_or(""), Path::new(cwd)) {
        Resolved::Path(x) => x,
        Resolved::Error(e) => return attach(json!({ "argv": argv, "cwd": cwd, "code": null, "output": "", "ms": 0, "ok": false, "error": e, "at": at })),
    };
    let limit = timeout.unwrap_or(CHECK_TIMEOUT_MS).min(CHECK_TIMEOUT_MS).max(1000.0) as u64;
    let spec = RunSpec { command: resolved.clone(), args: argv[1..].to_vec(), cwd: cwd.to_string(), stdin: String::new(), timeout_ms: limit, max_bytes: CHECK_MAX_BYTES, env: None };
    let mut a = |_: usize| {};
    let mut l = |_: &str| -> Option<String> { None };
    let mut sp = || {};
    let res = run_proc(&spec, token, ProcHooks { on_activity: &mut a, on_line: &mut l, on_spawn: &mut sp }, on_pid);
    let joined = if res.stderr.is_empty() { res.stdout.clone() } else { format!("{}\n{}", res.stdout, res.stderr) };
    let masked = sh::mask_secrets(&joined);
    let c: Vec<char> = masked.chars().collect();
    let output: String = c[c.len().saturating_sub(KEEP_OUTPUT)..].iter().collect();
    let error = res.spawn_error.clone().or_else(|| {
        if res.timed_out {
            Some("timed out".to_string())
        } else if res.cancelled {
            Some("cancelled".to_string())
        } else if res.truncated {
            Some("too much output".to_string())
        } else {
            None
        }
    });
    let mut out = json!({ "argv": argv, "resolved": resolved, "cwd": cwd, "code": res.code, "output": output, "ms": res.ms, "ok": error.is_none() && res.code == Some(0) });
    if let Some(e) = error {
        out["error"] = json!(e);
    }
    out["at"] = json!(at);
    attach(out)
}

fn describe_check(c: &Value) -> String {
    let why = c.get("error").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("exit {}", c["code"]));
    let out = sv(c, "output").trim().to_string();
    let lines: Vec<&str> = out.lines().collect();
    let tail = lines[lines.len().saturating_sub(6)..].join("\n");
    format!("{} -> {why}{}", sh::strs(&c["argv"]).join(" "), if tail.is_empty() { String::new() } else { format!("\n{tail}") })
}

fn complete_task(svc: &Service, run: &Run, id: &str, turn: Option<usize>, follow_up: bool) {
    let (gates, gaps, checks, dir, scope, run_id) = {
        let mut r = run.lock().unwrap();
        let gates = r["execution"].get("gates").map(crate::settings::truthy).unwrap_or(false);
        let board = sh::current_board(&r).unwrap().clone();
        let task = task_of(&board, id).unwrap();
        if gates {
            // Between verifying and committing, nothing else may start in this worktree
            r["execution"]["tasks"][id]["status"] = json!("running");
        }
        let te = &r["execution"]["tasks"][id];
        let gaps = sh::acceptance_gaps(task, te.get("result"));
        (gates, gaps, task["checks"].as_array().cloned().unwrap_or_default(), wt_of(&r, id), sh::strs(&task["scope"]), sv(&r, "id").to_string())
    };
    if !gates {
        return commit_task(svc, run, id, turn, follow_up);
    }
    if !gaps.is_empty() {
        let msg = format!("Cowork completion check: these acceptance items are missing or not done.\n{}\n\nFinish them, then reply again ending with a cowork-result block whose \"acceptance\" array reports every item.", gaps.join("\n"));
        return gate_follow_up(svc, run, id, "acceptance", gaps, follow_up, msg);
    }
    if checks.is_empty() {
        return commit_task(svc, run, id, turn, follow_up);
    }
    let token = CancelToken::default();
    svc.register_token(&run_id, &token);
    {
        let mut r = run.lock().unwrap();
        clock_start(svc, &mut r);
    }
    let path = Path::new(&dir);
    enum Next {
        Commit,
        Stopped,
        Failed(String),
        Scope(Vec<String>, Option<String>),
        Unstable,
        FollowUp(Vec<String>),
    }
    let outcome = (|| -> Next {
        let mut attempt = 0;
        loop {
            let before = match g::tree_of(path) {
                Ok(t) => t,
                Err(e) => return Next::Failed(e),
            };
            let mut runs: Vec<Value> = Vec::new();
            for argv in &checks {
                let args = sh::strs(argv);
                let left = budget_left(svc, &run.lock().unwrap());
                let c = run_check(svc, &args, &dir, &token, Some(left), Some(&before));
                let ok = c["ok"] == json!(true);
                runs.push(c);
                if !ok {
                    break;
                }
            }
            run.lock().unwrap()["execution"]["tasks"][id]["checkRuns"] = json!(runs);
            if token.is_cancelled() {
                return Next::Stopped;
            }
            let failed: Vec<&Value> = runs.iter().filter(|c| c["ok"] != json!(true)).collect();
            let after = match g::tree_of(path) {
                Ok(t) => t,
                Err(e) => return Next::Failed(e),
            };
            if after == before {
                if failed.is_empty() {
                    return Next::Commit;
                }
                return Next::FollowUp(failed.iter().map(|c| describe_check(c)).collect());
            }
            // The checks changed what they verified, so that result does not count. Check the scope, then once more on the new content.
            let files = match g::changed_files(path) {
                Ok(f) => f,
                Err(e) => return Next::Failed(e),
            };
            let outside = sh::files_outside_scope(&scope, &files);
            if !outside.is_empty() {
                return Next::Scope(outside.clone(), g::scope_content_hash(path, &outside).ok());
            }
            if attempt >= 1 {
                return Next::Unstable;
            }
            attempt += 1;
        }
    })();
    svc.unregister_token(&run_id, &token);
    {
        let mut r = run.lock().unwrap();
        clock_stop(svc, &mut r);
    }
    match outcome {
        Next::Commit => commit_task(svc, run, id, turn, follow_up),
        Next::Stopped => {
            let mut r = run.lock().unwrap();
            let te = &mut r["execution"]["tasks"][id];
            te["status"] = json!("cancelled");
            te["error"] = json!("stopped");
            svc.save(&mut r);
        }
        Next::Failed(e) => fail_task(svc, run, id, turn, format!("Could not run the checks: {e}")),
        Next::Scope(outside, hash) => hold_scope(svc, run, id, outside, follow_up, hash, "Checks"),
        Next::Unstable => block_gate(svc, run, id, "checks", vec!["Checks keep changing the files they verify, so the result cannot be trusted.".into()], follow_up),
        Next::FollowUp(items) => {
            let msg = format!("Cowork ran the approved checks and they failed:\n{}\n\nFix the cause, then reply again ending with a cowork-result block.", items.join("\n"));
            gate_follow_up(svc, run, id, "checks", items, follow_up, msg)
        }
    }
}

/// The user accepts what did not pass. The reason stays on record, and it never counts as verified.
pub fn waive(svc: &Service, run: &Run, id: &str, reason: &str, allow_git_config: bool) -> CR<()> {
    let why = reason.trim();
    let (follow_up, dir, kind) = {
        let mut r = run.lock().unwrap();
        if !r["execution"].is_object() || r["execution"]["tasks"].get(id).is_none() {
            return Err(cerr("task-not-found"));
        }
        let te = &r["execution"]["tasks"][id];
        let gate = te.get("gate").filter(|g| g.is_object()).cloned();
        if r["execution"].get("cleaned").map(crate::settings::truthy) == Some(true) || sv(te, "status") != "blocked" || gate.is_none() {
            return Err(cerr("not-allowed"));
        }
        let gate = gate.unwrap();
        if why.is_empty() {
            return Err(cerr("waive-reason-required"));
        }
        // A changed git config is only confirmed at the computer: a phone cannot show it, and should not be able to let it through
        if sv(&gate, "kind") == "git-config" && !allow_git_config {
            return Err(cerr_m("not-allowed", "Review the git configuration change on the computer."));
        }
        let wt = sv(te, "worktree").to_string();
        if r["execution"]["tasks"].as_object().unwrap().values().any(|x| sv(x, "status") == "running" && sv(x, "worktree") == wt) {
            return Err(cerr("busy"));
        }
        let short = js_prefix(why, 2000);
        let at = now(svc);
        {
            let te = &mut r["execution"]["tasks"][id];
            te["waiver"] = json!({ "kind": gate["kind"], "reason": short, "items": gate["items"], "at": at });
            del(te, "gate");
        }
        log_push(&mut r, json!({ "t": "task-waived", "taskId": id, "kind": gate["kind"], "reason": short, "at": at }));
        (gate["followUp"] == json!(true), wt_of(&r, id), sv(&gate, "kind").to_string())
    };
    if kind == "git-config" {
        let state = g::git_guard_state(Path::new(&dir)).map_err(|e| cerr_m("internal", e))?;
        run.lock().unwrap()["execution"]["gitGuard"] = json!(state);
    }
    commit_task(svc, run, id, None, follow_up);
    Ok(())
}

fn commit_task(svc: &Service, run: &Run, id: &str, turn: Option<usize>, follow_up: bool) {
    let (dir, msg, hooks) = {
        let r = run.lock().unwrap();
        let task = task_of(sh::current_board(&r).unwrap(), id).unwrap().clone();
        (wt_of(&r, id), format!("cowork({}): {} {}{}", sv(&r, "id"), sv(&task, "id"), sv(&task, "title"), if follow_up { " (follow-up)" } else { "" }), r["execution"].get("hooksDir").and_then(Value::as_str).map(str::to_string))
    };
    let path = Path::new(&dir);
    let res = g::changed_files(path).and_then(|files| g::commit_all(path, &msg, hooks.as_deref()).map(|c| (files, c)));
    let (files, commit) = match res {
        Ok(x) => x,
        Err(e) => return fail_task(svc, run, id, turn, format!("Could not commit: {e}")),
    };
    {
        let mut r = run.lock().unwrap();
        let at = now(svc);
        {
            let te = &mut r["execution"]["tasks"][id];
            if let Some(c) = &commit {
                let mut all = sh::strs(&te["changedFiles"]);
                for f in files {
                    if !all.contains(&f) {
                        all.push(f);
                    }
                }
                te["changedFiles"] = json!(all);
                te["commits"].as_array_mut().unwrap().push(json!(c));
            }
            te["status"] = json!("done");
            for k in ["error", "scopeReview", "gate"] {
                del(te, k);
            }
        }
        log_push(&mut r, json!({ "t": "task-done", "taskId": id, "commit": commit, "at": at }));
        svc.save(&mut r);
    }
    schedule(svc, run);
}

fn fail_task(svc: &Service, run: &Run, id: &str, turn: Option<usize>, message: String) {
    {
        let mut r = run.lock().unwrap();
        let te = &mut r["execution"]["tasks"][id];
        if let Some(t) = turn {
            te["turns"][t]["error"] = json!(message);
        }
        te["status"] = json!("failed");
        te["error"] = json!(message);
        log_push(&mut r, json!({ "t": "task-failed", "taskId": id, "message": message, "at": now(svc) }));
        svc.save(&mut r);
    }
    schedule(svc, run);
}

fn scope_task(r: &Value, id: &str) -> CR<Value> {
    if !r["execution"].is_object() || r["execution"]["tasks"].get(id).is_none() {
        return Err(cerr("task-not-found"));
    }
    let te = &r["execution"]["tasks"][id];
    let review = te.get("scopeReview").filter(|x| x.is_object());
    if r["execution"].get("cleaned").map(crate::settings::truthy) == Some(true) || sv(te, "status") != "blocked" || review.is_none() {
        return Err(cerr("not-allowed"));
    }
    let wt = sv(te, "worktree");
    if r["execution"]["tasks"].as_object().unwrap().values().any(|x| sv(x, "status") == "running" && sv(x, "worktree") == wt) {
        return Err(cerr("busy"));
    }
    Ok(review.unwrap().clone())
}

/// Accept out-of-scope changes. The hash is what the user saw: same names with new content voids the approval.
pub fn accept_scope(svc: &Service, run: &Run, id: &str, hash: Option<&str>) -> CR<()> {
    let (review, dir, scope) = {
        let r = run.lock().unwrap();
        let review = scope_task(&r, id)?;
        (review, wt_of(&r, id), sh::strs(&task_of(sh::current_board(&r).unwrap(), id).unwrap()["scope"]))
    };
    if let Some(expected) = review.get("hash").and_then(Value::as_str) {
        let path = Path::new(&dir);
        let files = sh::files_outside_scope(&scope, &g::changed_files(path).map_err(|e| cerr_m("internal", e))?);
        let current = g::scope_content_hash(path, &files).map_err(|e| cerr_m("internal", e))?;
        if hash != Some(expected) || current != expected {
            let mut r = run.lock().unwrap();
            let rv = &mut r["execution"]["tasks"][id]["scopeReview"];
            rv["files"] = json!(files);
            rv["hash"] = json!(current);
            svc.save(&mut r);
            return Err(cerr_m("stale", "The files changed after you reviewed them. Review them again."));
        }
    }
    {
        let mut r = run.lock().unwrap();
        log_push(&mut r, json!({ "t": "task-scope-resolved", "taskId": id, "action": "accept", "files": review["files"], "at": now(svc) }));
        del(&mut r["execution"]["tasks"][id], "scopeReview");
    }
    complete_task(svc, run, id, None, review["followUp"] == json!(true));
    Ok(())
}

/// Back to HEAD for the out-of-scope files; changes in scope are committed as usual
pub fn revert_scope(svc: &Service, run: &Run, id: &str) -> CR<()> {
    let (review, dir) = {
        let r = run.lock().unwrap();
        (scope_task(&r, id)?, wt_of(&r, id))
    };
    g::revert_files(Path::new(&dir), &sh::strs(&review["files"])).map_err(|e| cerr_m("revert-failed", e))?;
    {
        let mut r = run.lock().unwrap();
        log_push(&mut r, json!({ "t": "task-scope-resolved", "taskId": id, "action": "revert", "files": review["files"], "at": now(svc) }));
        del(&mut r["execution"]["tasks"][id], "scopeReview");
    }
    complete_task(svc, run, id, None, review["followUp"] == json!(true));
    Ok(())
}

fn finish(svc: &Service, run: &Run) {
    let id = {
        let r = run.lock().unwrap();
        let id = sv(&r, "id").to_string();
        if sv(&r, "phase") == "review" && r["execution"].get("integration").map(|i| i.is_object()).unwrap_or(false) {
            return;
        }
        if !svc.exec.finishing.lock().unwrap().insert(id.clone()) {
            return;
        }
        id
    };
    let (repo, base_commit, root, mode, hooks, base_dir, link) = {
        let r = run.lock().unwrap();
        let ex = &r["execution"];
        let skipped = ex["depsLinkSkippedFor"].as_array().map(|a| !a.is_empty()).unwrap_or(false);
        (r["repo"]["root"].as_str().unwrap_or("").to_string(), r["repo"]["baseCommit"].as_str().unwrap_or("").to_string(), sv(ex, "root").to_string(), sv(ex, "mode").to_string(), ex.get("hooksDir").and_then(Value::as_str).map(str::to_string), ex.get("worktreeBase").and_then(Value::as_str).map(str::to_string), ex.get("gates").map(crate::settings::truthy).unwrap_or(false) && ex["linkDeps"] == json!(true) && !skipped)
    };
    let rp = Path::new(&repo);
    let integ = (|| -> Result<Value, String> {
        if mode == "sequential" {
            let (path, branch) = {
                let r = run.lock().unwrap();
                let wt = &r["execution"]["worktrees"]["main"];
                (sv(wt, "path").to_string(), sv(wt, "branch").to_string())
            };
            let head = g::git(Path::new(&path), &["rev-parse", "HEAD"])?.trim().to_string();
            return Ok(json!({ "status": "ok", "branch": branch, "worktree": "main", "commit": head, "stat": g::diff_stat(Path::new(&path), &base_commit, &head)? }));
        }
        // Parallel: an integration branch from the baseline, cherry-picking each task's commits in dependency order
        let key = "integration";
        let branch = format!("cowork/{id}/{key}");
        let dir = p(&Path::new(&root).join(key));
        let exists = run.lock().unwrap()["execution"]["worktrees"].get(key).is_some();
        if exists {
            let _ = g::remove_worktrees(rp, &Path::new(&root).join("__none__"), std::slice::from_ref(&dir), base_dir.as_deref());
            let _ = g::git(rp, &["branch", "-D", &branch]);
        }
        g::add_worktree(rp, Path::new(&dir), &branch, &base_commit)?;
        // Shared checks need dependencies: same policy as the tasks
        let linked = if link { g::link_node_modules(rp, Path::new(&dir))? } else { false };
        let (order, commits) = {
            let mut r = run.lock().unwrap();
            r["execution"]["worktrees"][key] = json!({ "path": dir, "branch": branch, "depsLinked": linked });
            let board = sh::current_board(&r).unwrap().clone();
            let tasks = board["tasks"].as_array().unwrap();
            let order = sh::topo_order(tasks).unwrap_or_else(|| tasks.iter().map(|t| sv(t, "id").to_string()).collect());
            let commits: Vec<String> = order.iter().flat_map(|x| sh::strs(&r["execution"]["tasks"][x.as_str()]["commits"])).collect();
            (order, commits)
        };
        match g::cherry_pick_all(Path::new(&dir), &commits, hooks.as_deref())? {
            g::PickResult::Conflict { commit, message, .. } => {
                let r = run.lock().unwrap();
                let owner = order.iter().find(|x| sh::strs(&r["execution"]["tasks"][x.as_str()]["commits"]).contains(&commit)).cloned().unwrap_or_else(|| "?".into());
                Ok(json!({ "status": "conflict", "branch": branch, "worktree": key, "message": format!("{owner} ({}): {message}", js_prefix(&commit, 10)) }))
            }
            _ => {
                let head = g::git(Path::new(&dir), &["rev-parse", "HEAD"])?.trim().to_string();
                Ok(json!({ "status": "ok", "branch": branch, "worktree": key, "commit": head, "stat": g::diff_stat(Path::new(&dir), &base_commit, &head)? }))
            }
        }
    })()
    .unwrap_or_else(|e| json!({ "status": "error", "branch": "", "message": js_prefix(&e, 500) }));
    let ok_gated = {
        let mut r = run.lock().unwrap();
        let ok = sv(&integ, "status") == "ok" && r["execution"].get("gates").map(crate::settings::truthy).unwrap_or(false);
        r["execution"]["integration"] = integ;
        ok
    };
    if ok_gated {
        if let Err(e) = after_integration(svc, run) {
            let mut r = run.lock().unwrap();
            let it = &mut r["execution"]["integration"];
            it["checks"] = json!("failed");
            it["message"] = json!(js_prefix(&e, 500));
        }
    }
    svc.exec.finishing.lock().unwrap().remove(&id);
    let mut r = run.lock().unwrap();
    r["phase"] = json!("review");
    let it = r["execution"]["integration"].clone();
    let msg = Some(sv(&it, "message")).filter(|m| !m.is_empty()).unwrap_or(sv(&it, "branch"));
    log_push(&mut r, json!({ "t": "integrated", "ok": sv(&it, "status") == "ok", "message": msg, "at": now(svc) }));
    svc.save(&mut r);
}

/// Shared checks, note the target branch, then review. Unchanged content is not paid for again.
fn after_integration(svc: &Service, run: &Run) -> Result<(), String> {
    let (dir, base_commit, commit, repo, source, checks) = {
        let r = run.lock().unwrap();
        let it = &r["execution"]["integration"];
        let dir = sv(&r["execution"]["worktrees"][sv(it, "worktree")], "path").to_string();
        (dir, r["repo"]["baseCommit"].as_str().unwrap_or("").to_string(), sv(it, "commit").to_string(), r["repo"]["root"].as_str().unwrap_or("").to_string(), r["repo"]["sourceBranch"].as_str().unwrap_or("").to_string(), sh::current_board(&r).unwrap()["checks"].as_array().cloned().unwrap_or_default())
    };
    let path = Path::new(&dir);
    let patch = g::patch_id(path, &base_commit, &commit)?;
    let hook_paths = sh::hook_paths_in(&g::changed_paths(path, &base_commit, &commit)?, g::configured_hooks_path(Path::new(&repo)).as_deref());
    let head = g::rev_parse(Path::new(&repo), &source)?;
    {
        let mut r = run.lock().unwrap();
        let ex = &mut r["execution"];
        ex["integration"]["patchId"] = json!(patch);
        ex["integration"]["hookChanges"] = json!(hook_paths);
        ex["targetHead"] = json!(head);
        del(ex, "targetCheck");
        clock_start(svc, &mut r);
    }
    let result = (|| -> Result<(), String> {
        let (status, runs) = run_shared_checks(svc, run, &dir, &checks)?;
        let review_wanted = {
            let mut r = run.lock().unwrap();
            r["execution"]["integration"]["checks"] = json!(status);
            r["execution"]["integration"]["checkRuns"] = json!(runs);
            log_push(&mut r, json!({ "t": "integration-checks", "status": status, "at": now(svc) }));
            svc.save(&mut r);
            let ex = &r["execution"];
            let rv = ex.get("review").filter(|x| x.is_object());
            let stale = rv.map(|x| sv(x, "status") != "ok" || x["patchId"] != ex["integration"]["patchId"]).unwrap_or(true);
            ex["reviewOn"] == json!(true) && stale && status != "failed"
        };
        if review_wanted {
            svc.review_execution(run);
        }
        Ok(())
    })();
    {
        let mut r = run.lock().unwrap();
        clock_stop(svc, &mut r);
    }
    result
}

/// In the harness's own worktree. Dirtied: reset and run once more. Still dirty: not a pass.
fn run_shared_checks(svc: &Service, run: &Run, dir: &str, checks: &[Value]) -> Result<(&'static str, Vec<Value>), String> {
    if checks.is_empty() {
        return Ok(("none", Vec::new()));
    }
    let run_id = run.lock().unwrap()["id"].as_str().unwrap_or("").to_string();
    let token = CancelToken::default();
    svc.register_token(&run_id, &token);
    let path = Path::new(dir);
    let out = (|| -> Result<(&'static str, Vec<Value>), String> {
        let mut attempt = 0;
        loop {
            let head = g::git(path, &["rev-parse", "HEAD"])?.trim().to_string();
            let mut runs = Vec::new();
            for argv in checks {
                let left = budget_left(svc, &run.lock().unwrap());
                let mut c = run_check(svc, &sh::strs(argv), dir, &token, Some(left), None);
                c["commit"] = json!(head);
                let ok = c["ok"] == json!(true);
                runs.push(c);
                if !ok {
                    break;
                }
            }
            let dirty = g::git(path, &["status", "--porcelain"])?;
            if dirty.trim().is_empty() {
                return Ok((if runs.iter().all(|c| c["ok"] == json!(true)) { "passed" } else { "failed" }, runs));
            }
            g::reset_worktree(path)?;
            if attempt >= 1 {
                if let Some(l) = runs.last_mut() {
                    l["error"] = json!("checks keep changing tracked files");
                }
                return Ok(("failed", runs));
            }
            attempt += 1;
        }
    })();
    svc.unregister_token(&run_id, &token);
    out
}

/// A follow-up on a task: the same session, or a fresh handover with these words if there is none
pub fn message(svc: &Service, run: &Run, id: &str, text: &str) -> CR<()> {
    let msg = text.trim();
    {
        let mut r = run.lock().unwrap();
        if !r["execution"].is_object() || r["execution"]["tasks"].get(id).is_none() {
            return Err(cerr("task-not-found"));
        }
        if msg.is_empty() {
            return Err(cerr("empty-feedback"));
        }
        let phase = sv(&r, "phase").to_string();
        if phase != "executing" && phase != "review" {
            return Err(cerr("not-allowed"));
        }
        if r["execution"].get("cleaned").map(crate::settings::truthy) == Some(true) {
            return Err(cerr_m("not-allowed", "worktrees were cleaned up"));
        }
        let te = &r["execution"]["tasks"][id];
        let (st, wt) = (sv(te, "status").to_string(), sv(te, "worktree").to_string());
        if st == "running" || st == "pending" || r["execution"]["tasks"].as_object().unwrap().values().any(|x| sv(x, "status") == "running" && sv(x, "worktree") == wt) {
            return Err(cerr("busy"));
        }
        if phase == "review" {
            r["phase"] = json!("executing");
        }
        r["execution"]["tasks"][id]["gateFollowUps"] = json!(0);
        mark_turn(svc, &mut r, id, false);
    }
    spawn_turn(svc, run, id.to_string(), TurnOpts { message: Some(js_prefix(msg, 20000)) });
    Ok(())
}

/// Hand over again in a new session; what is already in the worktree stays
pub fn retry(svc: &Service, run: &Run, id: &str) -> CR<()> {
    {
        let mut r = run.lock().unwrap();
        if !r["execution"].is_object() || r["execution"]["tasks"].get(id).is_none() {
            return Err(cerr("task-not-found"));
        }
        if !["failed", "blocked", "cancelled"].contains(&sv(&r["execution"]["tasks"][id], "status")) {
            return Err(cerr("not-allowed"));
        }
        {
            let te = &mut r["execution"]["tasks"][id];
            te["status"] = json!("pending");
            for k in ["sessionId", "error", "scopeReview", "gate"] {
                del(te, k);
            }
            te["gateFollowUps"] = json!(0);
        }
        if sv(&r, "phase") == "review" {
            r["phase"] = json!("executing");
        }
        svc.save(&mut r);
    }
    schedule(svc, run);
    Ok(())
}

fn pause_locked(svc: &Service, r: &mut Value, reason: &str) {
    let id = sv(r, "id").to_string();
    r["execution"]["paused"] = json!(true);
    r["execution"]["pausedReason"] = json!(reason);
    svc.cancel_tokens(&id);
    log_push(r, json!({ "t": "exec-paused", "reason": reason, "at": now(svc) }));
    svc.save(r);
}

pub fn pause(svc: &Service, run: &Run) -> CR<()> {
    let mut r = run.lock().unwrap();
    if !r["execution"].is_object() {
        return Err(cerr("not-allowed"));
    }
    pause_locked(svc, &mut r, "user");
    Ok(())
}

/// A stopped task with a session continues it; without one it gets scheduled again
pub fn resume(svc: &Service, run: &Run) -> CR<()> {
    let mut starts = Vec::new();
    {
        let mut r = run.lock().unwrap();
        if !r["execution"].is_object() || r["execution"]["paused"] != json!(true) {
            return Err(cerr("not-allowed"));
        }
        if budget_left(svc, &r) <= 0.0 {
            return Err(cerr("budget-exhausted"));
        }
        r["execution"]["paused"] = json!(false);
        del(&mut r["execution"], "pausedReason");
        log_push(&mut r, json!({ "t": "exec-resumed", "at": now(svc) }));
        let ids: Vec<String> = r["execution"]["tasks"].as_object().unwrap().iter().filter(|(_, t)| sv(t, "status") == "cancelled").map(|(k, _)| k.clone()).collect();
        for id in ids {
            if sv(&r["execution"]["tasks"][&id], "sessionId").is_empty() {
                r["execution"]["tasks"][&id]["status"] = json!("pending");
            } else {
                mark_turn(svc, &mut r, &id, false);
                starts.push(id);
            }
        }
        svc.save(&mut r);
    }
    for id in starts {
        spawn_turn(svc, run, id, TurnOpts { message: Some("You were interrupted. Continue the task from where you stopped, then reply as instructed before.".into()) });
    }
    schedule(svc, run);
    Ok(())
}

/// The target moved: try the merge and run shared checks on the result. Same patch means no new paid review.
fn refresh_target(svc: &Service, run: &Run, head: &str) -> Result<(), String> {
    let (repo, commit, root, base, link, checks, from) = {
        let r = run.lock().unwrap();
        let ex = &r["execution"];
        let skipped = ex["depsLinkSkippedFor"].as_array().map(|a| !a.is_empty()).unwrap_or(false);
        (r["repo"]["root"].as_str().unwrap_or("").to_string(), sv(&ex["integration"], "commit").to_string(), sv(ex, "root").to_string(), ex.get("worktreeBase").and_then(Value::as_str).map(str::to_string), ex["linkDeps"] == json!(true) && !skipped, sh::current_board(&r).map(|b| b["checks"].as_array().cloned().unwrap_or_default()).unwrap_or_default(), sv(ex, "targetHead").to_string())
    };
    let rp = Path::new(&repo);
    let merged = g::merge_tree(rp, head, &commit);
    let check = match merged {
        g::MergeTree::Failed { message, .. } => json!({ "head": head, "status": "conflict", "message": message, "at": now(svc) }),
        g::MergeTree::Ok { tree, .. } => {
            if checks.is_empty() {
                json!({ "head": head, "status": "none", "at": now(svc) })
            } else {
                let dir = p(&Path::new(&root).join("target-check"));
                let none = Path::new(&root).join("__none__");
                let _ = g::remove_worktrees(rp, &none, std::slice::from_ref(&dir), base.as_deref());
                g::add_tree_worktree(rp, Path::new(&dir), head, &tree)?;
                if link {
                    let _ = g::link_node_modules(rp, Path::new(&dir));
                }
                // This content has no commit: only the check results matter
                let token = CancelToken::default();
                let mut runs = Vec::new();
                for argv in &checks {
                    let c = run_check(svc, &sh::strs(argv), &dir, &token, None, None);
                    let ok = c["ok"] == json!(true);
                    runs.push(c);
                    if !ok {
                        break;
                    }
                }
                let _ = g::remove_worktrees(rp, &none, std::slice::from_ref(&dir), base.as_deref());
                json!({ "head": head, "status": if runs.iter().all(|c| c["ok"] == json!(true)) { "passed" } else { "failed" }, "runs": runs, "at": now(svc) })
            }
        }
    };
    let mut r = run.lock().unwrap();
    r["execution"]["targetHead"] = json!(head);
    let status = check["status"].clone();
    r["execution"]["targetCheck"] = check;
    log_push(&mut r, json!({ "t": "target-moved", "from": from, "to": head, "status": status, "at": now(svc) }));
    svc.save(&mut r);
    Ok(())
}

/// The approval binds the integration commit and target HEAD. A repeated or concurrent send runs once.
pub fn merge(svc: &Service, run: &Run, approval: Option<&Value>) -> CR<()> {
    let id = run.lock().unwrap()["id"].as_str().unwrap_or("").to_string();
    let (slot, leader) = {
        let mut m = svc.exec.merging.lock().unwrap();
        match m.get(&id) {
            Some(s) => (s.clone(), false),
            None => {
                let s: Arc<Mutex<Option<Result<(), CoworkError>>>> = Arc::default();
                m.insert(id.clone(), s.clone());
                (s, true)
            }
        }
    };
    if leader {
        let mut guard = slot.lock().unwrap();
        let out = do_merge(svc, run, approval);
        *guard = Some(out.clone());
        svc.exec.merging.lock().unwrap().remove(&id);
        return out;
    }
    let done = slot.lock().unwrap().clone();
    done.unwrap_or(Ok(()))
}

fn do_merge(svc: &Service, run: &Run, approval: Option<&Value>) -> CR<()> {
    let (repo, source, it, target_head, prompt, hooks) = {
        let r = run.lock().unwrap();
        let ex = &r["execution"];
        let it = ex["integration"].clone();
        // The same approval sent again: already done
        if ex.get("merged").map(crate::settings::truthy).unwrap_or(false) && approval.and_then(|a| a.get("commit")) == it.get("commit") && approval.is_some() {
            return Ok(());
        }
        if sv(&r, "phase") != "review" || !it.is_object() || sv(&it, "status") != "ok" {
            return Err(cerr("not-allowed"));
        }
        (r["repo"]["root"].as_str().unwrap_or("").to_string(), r["repo"]["sourceBranch"].as_str().unwrap_or("").to_string(), it, sv(ex, "targetHead").to_string(), sv(&r, "prompt").to_string(), ex.get("hooksDir").and_then(Value::as_str).map(str::to_string))
    };
    let refresh = "Reload the run and review it again.";
    let Some(approval) = approval.filter(|a| a.get("commit") == it.get("commit")) else { return Err(cerr_m("stale", refresh)) };
    let rp = Path::new(&repo);
    let int_err = |e: String| cerr_m("internal", e);
    let commit = sv(&it, "commit").to_string();
    if g::rev_parse(rp, sv(&it, "branch")).map_err(int_err)? != commit {
        return Err(cerr_m("stale", format!("The branch {} moved after review. {refresh}", sv(&it, "branch"))));
    }
    let head = g::rev_parse(rp, &source).map_err(int_err)?;
    // An old record has no targetHead: today's counts, tied to the commit only
    let legacy = target_head.is_empty();
    if legacy {
        run.lock().unwrap()["execution"]["targetHead"] = json!(head);
    } else if head != target_head {
        refresh_target(svc, run, &head).map_err(int_err)?;
        return Err(cerr_d("stale", Some(format!("{source} moved. The merge result was checked again. {refresh}")), json!({ "targetHead": head })));
    }
    if !legacy && approval.get("targetHead").and_then(Value::as_str) != Some(target_head.as_str()) {
        return Err(cerr_d("stale", Some(refresh.to_string()), json!({ "targetHead": target_head })));
    }
    {
        let r = run.lock().unwrap();
        let tc = &r["execution"]["targetCheck"];
        if sv(tc, "head") == head && sv(tc, "status") == "conflict" {
            return Err(cerr_d("merge-conflict", Some(sv(tc, "message").to_string()), json!({ "branch": source })));
        }
    }
    let tampered = guard_changes(run, &repo);
    if !tampered.is_empty() {
        return Err(cerr_m("git-config-changed", tampered.join("\n")));
    }
    let blockers = sh::merge_blockers(Some(&run.lock().unwrap()["execution"]));
    let why = approval.get("waive").and_then(Value::as_str).map(str::trim).filter(|w| !w.is_empty()).map(str::to_string);
    if !blockers.is_empty() && why.is_none() {
        return Err(cerr_d("merge-needs-waiver", Some(blockers.join("\n")), json!({ "blockers": blockers })));
    }
    let first = js_prefix(prompt.split('\n').next().unwrap_or("").trim_end_matches('\r'), 72);
    let id = run.lock().unwrap()["id"].as_str().unwrap_or("").to_string();
    match g::merge_branch(rp, &source, &commit, &format!("Merge Cowork {id}: {first}"), hooks.as_deref()).map_err(int_err)? {
        g::MergeResult::Failed { reason, message, .. } => Err(cerr_d(&format!("merge-{reason}"), Some(message), json!({ "branch": source }))),
        g::MergeResult::Ok { commit: merged, .. } => {
            let mut r = run.lock().unwrap();
            let at = now(svc);
            if let (false, Some(w)) = (blockers.is_empty(), &why) {
                let short = js_prefix(w, 2000);
                r["execution"]["mergeWaiver"] = json!({ "reason": short, "items": blockers, "at": at });
                log_push(&mut r, json!({ "t": "merge-waived", "reason": short, "items": blockers, "at": at }));
            }
            r["execution"]["merged"] = json!({ "into": source, "commit": merged, "at": at });
            r["phase"] = json!("completed");
            log_push(&mut r, json!({ "t": "merged", "into": source, "commit": merged, "at": at }));
            svc.save(&mut r);
            Ok(())
        }
    }
}

/// Worktrees go; branches stay for merging or looking at later
pub fn cleanup(svc: &Service, run: &Run) -> CR<()> {
    let (repo, root, paths, base) = {
        let r = run.lock().unwrap();
        let ex = &r["execution"];
        if !ex.is_object() {
            return Err(cerr("not-allowed"));
        }
        if ex["tasks"].as_object().unwrap().values().any(|t| sv(t, "status") == "running") {
            return Err(cerr("busy"));
        }
        (r["repo"]["root"].as_str().unwrap_or("").to_string(), sv(ex, "root").to_string(), ex["worktrees"].as_object().unwrap().values().map(|w| sv(w, "path").to_string()).collect::<Vec<_>>(), ex.get("worktreeBase").and_then(Value::as_str).map(str::to_string))
    };
    g::remove_worktrees(Path::new(&repo), Path::new(&root), &paths, base.as_deref()).map_err(|e| cerr_m("internal", e))?;
    let mut r = run.lock().unwrap();
    r["execution"]["cleaned"] = json!(true);
    r["phase"] = json!("completed");
    log_push(&mut r, json!({ "t": "cleaned", "at": now(svc) }));
    svc.save(&mut r);
    Ok(())
}

/// For the phone: capped, says so when cut, and masks common credential formats (best effort)
pub fn diff(run: &Run) -> CR<Value> {
    let (dir, base, commit, target) = {
        let r = run.lock().unwrap();
        let ex = &r["execution"];
        let it = &ex["integration"];
        if sv(it, "commit").is_empty() || sv(it, "worktree").is_empty() || ex.get("cleaned").map(crate::settings::truthy) == Some(true) {
            return Ok(Value::Null);
        }
        (sv(&ex["worktrees"][sv(it, "worktree")], "path").to_string(), r["repo"]["baseCommit"].as_str().unwrap_or("").to_string(), sv(it, "commit").to_string(), sv(ex, "targetHead").to_string())
    };
    let d = g::diff_text(Path::new(&dir), &base, &commit, 200_000).map_err(|e| cerr_m("internal", e))?;
    Ok(json!({ "text": sh::mask_secrets(&d.text), "truncated": d.truncated, "commit": commit, "targetHead": target }))
}

/// After a restart: tasks that were running count as interrupted and the whole execution pauses until the user continues
pub fn recover(svc: &Service, r: &mut Value) -> bool {
    let phase = sv(r, "phase").to_string();
    if !r["execution"].is_object() || (phase != "executing" && phase != "review") {
        return false;
    }
    let mut changed = false;
    if r["execution"].pointer("/review/status").and_then(Value::as_str) == Some("running") {
        r["execution"]["review"]["status"] = json!("failed");
        r["execution"]["review"]["error"] = json!("interrupted by app restart");
        changed = true;
    }
    for te in r["execution"]["tasks"].as_object_mut().unwrap().values_mut() {
        if sv(te, "status") != "running" {
            continue;
        }
        te["status"] = json!("cancelled");
        te["error"] = json!("interrupted by app restart");
        for t in te["turns"].as_array_mut().unwrap() {
            if t.get("running") == Some(&json!(true)) {
                t["running"] = json!(false);
            }
        }
        changed = true;
    }
    if let Some(since) = r["execution"]["activeSince"].as_f64().filter(|s| *s != 0.0) {
        let extra = (r["updatedAt"].as_f64().unwrap_or(since) - since).max(0.0);
        add_num(&mut r["execution"]["msUsed"], extra);
        r["execution"]["activeSince"] = Value::Null;
        changed = true;
    }
    if changed && r["execution"]["paused"] != json!(true) {
        r["execution"]["paused"] = json!(true);
        r["execution"]["pausedReason"] = json!("restart");
        log_push(r, json!({ "t": "exec-paused", "reason": "restart", "at": now(svc) }));
    }
    changed
}
