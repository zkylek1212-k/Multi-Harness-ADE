// Cowork's app wiring: what each CLI can do, which models it has, the skills to attach, and the operations a window or phone can call.
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use wb_proc::runner::run_process;
use wb_proc::{CancelToken, RunSpec};

use crate::core::Core;
use crate::cowork::{cerr, cerr_m, CoworkDeps, CoworkError, ResolvedCli, Service, StartOptions, CR};
use crate::cowork_exec as exec;
use crate::cowork_shared as sh;
use crate::ext::build_inventory_public;
use crate::util::{home, js_prefix, mtime_ms, p, read_text, sv};

/// Flags each CLI must have to plan; an older version cannot guarantee read-only (cowork.md §3)
fn required_flags(agent: &str) -> (&'static [&'static str], &'static [&'static str]) {
    match agent {
        "claude" => (&["--help"], &["--json-schema", "--restricted", "--safe-mode", "--strict-mcp-config", "--permission-prompts", "--no-session-persistence"]),
        "codex" => (&["exec", "--help"], &["--output-schema", "--ignore-user-config", "--ephemeral", "--sandbox"]),
        _ => (&["--help"], &["--input-format", "--json-schema", "--sandbox", "--effort"]),
    }
}

fn run_short(command: &str, args: &[String], stdin: &str, timeout_ms: u64, env: Option<Vec<(String, String)>>) -> wb_proc::RunResult {
    let spec = RunSpec { command: command.to_string(), args: args.to_vec(), cwd: p(&home()), stdin: stdin.to_string(), timeout_ms, max_bytes: 8 * 1024 * 1024, env: env.map(|e| std::env::vars().chain(e).collect()) };
    run_process(&spec, &CancelToken::default())
}

fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home().join(".codex"))
}

/// Planning drops the user's config, including the Windows sandbox setting, without which read-only cannot even read (tested 2026-10-07)
pub fn read_codex_windows_sandbox() -> Option<String> {
    let text = read_text(&codex_home().join("config.toml"))?;
    let section = regex::Regex::new(r"^\s*\[([^\]]+)\]\s*$").unwrap();
    let key = regex::Regex::new(r#"^\s*sandbox\s*=\s*"([^"]*)""#).unwrap();
    // This value goes into -c windows.sandbox="...": safe characters only
    let safe = regex::RegexBuilder::new(r"^[a-z][a-z0-9_-]{0,31}$").case_insensitive(true).build().unwrap();
    let mut in_windows = false;
    for line in text.lines() {
        if let Some(m) = section.captures(line) {
            in_windows = m[1].trim() == "windows";
            continue;
        }
        if in_windows {
            if let Some(m) = key.captures(line) {
                if safe.is_match(&m[1]) {
                    return Some(m[1].to_string());
                }
            }
        }
    }
    None
}

/// model and model_reasoning_effort at the top level of config.toml, not in [profiles.x]
pub fn read_codex_user_model() -> (String, String) {
    let Some(text) = read_text(&codex_home().join("config.toml")) else { return Default::default() };
    let re = regex::Regex::new(r#"^\s*(model|model_reasoning_effort)\s*=\s*"([^"]*)""#).unwrap();
    let (mut model, mut effort) = (String::new(), String::new());
    for line in text.lines() {
        if line.trim_start().starts_with('[') {
            break;
        }
        if let Some(m) = re.captures(line) {
            if &m[1] == "model" {
                model = m[2].to_string();
            } else {
                effort = m[2].to_string();
            }
        }
    }
    let c = sh::sanitize_model_choice(Some(&json!({ "model": model, "effort": effort })));
    (sv(&c, "model").to_string(), sv(&c, "effort").to_string())
}

fn read_claude_user_model() -> String {
    let Some(j) = read_text(&home().join(".claude").join("settings.json")).and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { return String::new() };
    sv(&sh::sanitize_model_choice(Some(&json!({ "model": j.get("model"), "effort": "" }))), "model").to_string()
}

const CAP_TTL: Duration = Duration::from_secs(15);
const CATALOG_TTL: Duration = Duration::from_secs(600);

#[derive(Default)]
pub struct CoworkApp {
    help: Mutex<HashMap<String, bool>>,
    caps: Mutex<Option<(Instant, Vec<Value>)>>,
    catalogs: Mutex<HashMap<String, (Instant, Value)>>,
    known: Mutex<HashMap<String, Value>>,
    service: Mutex<Option<Arc<Service>>>,
}

struct Deps {
    core: Weak<Core>,
}

impl CoworkDeps for Deps {
    fn data_dir(&self) -> PathBuf {
        self.core.upgrade().map(|c| c.data_dir.join("cowork")).unwrap_or_default()
    }
    fn resolve_cli(&self, agent: &str) -> Result<ResolvedCli, String> {
        let core = self.core.upgrade().ok_or("shutting down")?;
        resolve_cli(&core, agent)
    }
    fn resolve_exec_cli(&self, agent: &str) -> Result<ResolvedCli, String> {
        let core = self.core.upgrade().ok_or("shutting down")?;
        let caps = capabilities(&core, false);
        let cap = caps.iter().find(|c| sv(c, "agent") == agent).ok_or("unknown agent")?;
        if cap["enabled"] != json!(true) {
            return Err("disabled in Settings".into());
        }
        match cap["path"].as_str() {
            Some(x) => Ok(ResolvedCli { command: x.to_string(), ..Default::default() }),
            None => Err(Some(sv(cap, "reason")).filter(|r| !r.is_empty()).unwrap_or("not installed").to_string()),
        }
    }
    fn emit(&self, run: &Value) {
        if let Some(core) = self.core.upgrade() {
            core.host.send_all("cowork:update", json!([run]));
            crate::remote::on_cowork_update(&core, run);
        }
    }
    fn on_pid(&self, pid: u32) {
        if let Some(core) = self.core.upgrade() {
            core.pty.assign_job(pid);
        }
    }
}

fn supports_flags(app: &CoworkApp, cli: &str, agent: &str) -> bool {
    let key = format!("{cli}:{}", mtime_ms(Path::new(cli)).unwrap_or(0.0));
    if let Some(hit) = app.help.lock().unwrap().get(&key) {
        return *hit;
    }
    let (args, flags) = required_flags(agent);
    let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let r = run_short(cli, &a, "", 20000, None);
    let text = format!("{}{}", r.stdout, r.stderr);
    let ok = r.code == Some(0) && flags.iter().all(|f| text.contains(f));
    app.help.lock().unwrap().insert(key, ok);
    ok
}

/// Shared by concurrent callers through the cache; the CLIs are not asked twice in a burst
pub fn capabilities(core: &Arc<Core>, force: bool) -> Vec<Value> {
    let app = &core.features().cowork;
    if !force {
        if let Some((at, caps)) = app.caps.lock().unwrap().as_ref() {
            if at.elapsed() < CAP_TTL {
                return caps.clone();
            }
        }
    }
    let settings = core.load_settings();
    let caps: Vec<Value> = std::thread::scope(|s| {
        let handles: Vec<_> = sh::COWORK_AGENTS
            .iter()
            .map(|agent| {
                let settings = &settings;
                s.spawn(move || {
                    let enabled = settings["cliEnabled"][agent] != json!(false);
                    let Some(path) = core.find_agent_cli(agent, true) else {
                        return json!({ "agent": agent, "enabled": enabled, "path": null, "planning": false, "reason": "not-installed" });
                    };
                    if !supports_flags(app, &path, agent) {
                        return json!({ "agent": agent, "enabled": enabled, "path": path, "planning": false, "reason": "cli-too-old" });
                    }
                    if *agent == "codex" && cfg!(windows) && read_codex_windows_sandbox().is_none() {
                        return json!({ "agent": agent, "enabled": enabled, "path": path, "planning": false, "reason": "codex-no-windows-sandbox" });
                    }
                    json!({ "agent": agent, "enabled": enabled, "path": path, "planning": true })
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    *app.caps.lock().unwrap() = Some((Instant::now(), caps.clone()));
    caps
}

/// agy cannot turn off the user's MCP, plugins and hooks for one run, so planning gets an isolated home.
/// Credentials live in the OS store and are unaffected (tested 2026-10-08). The rules are rewritten every time.
fn prepare_agy_home(core: &Core) -> Result<Vec<(String, String)>, String> {
    let dir = core.data_dir.join("cowork").join("agy-home");
    let settings = dir.join(".gemini").join("antigravity-cli").join("settings.json");
    if let Some(d) = settings.parent() {
        fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    for stale in [dir.join(".gemini").join("config"), dir.join(".gemini").join("antigravity-cli").join("mcp")] {
        let _ = fs::remove_dir_all(stale);
    }
    fs::write(&settings, serde_json::to_string_pretty(&sh::agy_isolation_settings(&p(&dir))).unwrap()).map_err(|e| e.to_string())?;
    Ok(vec![("USERPROFILE".into(), p(&dir)), ("HOME".into(), p(&dir))])
}

fn resolve_cli(core: &Arc<Core>, agent: &str) -> Result<ResolvedCli, String> {
    let caps = capabilities(core, false);
    let cap = caps.iter().find(|c| sv(c, "agent") == agent).ok_or("unknown agent")?;
    if cap["enabled"] != json!(true) {
        return Err("disabled in Settings".into());
    }
    let Some(path) = cap["path"].as_str().filter(|_| cap["planning"] == json!(true)) else {
        return Err(Some(sv(cap, "reason")).filter(|r| !r.is_empty()).unwrap_or("not available").to_string());
    };
    let app = &core.features().cowork;
    let known = app.known.lock().unwrap().get(agent).cloned();
    let mut model_efforts = HashMap::new();
    for o in known.as_ref().and_then(|k| k["options"].as_array()).into_iter().flatten() {
        if let Some(e) = o.get("efforts").filter(|e| e.is_array()) {
            let list = sh::strs(e);
            for id in [sv(o, "id"), sv(o, "resolvedModel")] {
                if !id.is_empty() {
                    model_efforts.insert(id.to_string(), list.clone());
                }
            }
        }
    }
    let mut out = ResolvedCli { command: path.to_string(), model_efforts, ..Default::default() };
    match agent {
        "antigravity" => out.env = Some(prepare_agy_home(core).map_err(|e| format!("could not prepare the isolated Antigravity home: {e}"))?),
        "codex" => {
            // No user choice falls back to config.toml, or --ignore-user-config would quietly change the model
            let (m, e) = read_codex_user_model();
            out.windows_sandbox = if cfg!(windows) { read_codex_windows_sandbox() } else { None };
            out.default_model = m;
            out.default_effort = e;
        }
        _ => {
            let user = read_claude_user_model();
            out.default_model = if user.is_empty() { known.map(|k| sv(&k["fallback"], "model").to_string()).unwrap_or_default() } else { user };
        }
    }
    Ok(out)
}

fn load_catalog(core: &Arc<Core>, agent: &str) -> Value {
    let caps = capabilities(core, false);
    let path = caps.iter().find(|c| sv(c, "agent") == agent).and_then(|c| c["path"].as_str()).map(str::to_string);
    let failed = |fallback: Value, e: &str| json!({ "agent": agent, "options": [], "fallback": fallback, "error": js_prefix(e, 200) });
    match agent {
        "claude" => {
            let user = read_claude_user_model();
            let fallback = json!({ "model": user, "effort": "", "source": if user.is_empty() { "cli-default" } else { "user-config" } });
            let Some(path) = path else { return failed(fallback, "not-installed") };
            let args: Vec<String> = ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--no-session-persistence", "--safe-mode", "--strict-mcp-config"].iter().map(|s| s.to_string()).collect();
            let stdin = json!({ "type": "control_request", "request_id": "cowork-models", "request": { "subtype": "initialize" } }).to_string() + "\n";
            let r = run_short(&path, &args, &stdin, 20000, None);
            if r.spawn_error.is_some() || r.timed_out || r.code != Some(0) {
                return failed(fallback, r.spawn_error.as_deref().unwrap_or("Could not read Claude models"));
            }
            sh::parse_claude_model_catalog(&r.stdout, &user).unwrap_or_else(|e| failed(fallback, &e))
        }
        "codex" => {
            let (model, effort) = read_codex_user_model();
            let fallback = json!({ "model": model, "effort": effort, "source": if model.is_empty() { "cli-default" } else { "user-config" } });
            let Some(path) = path else { return failed(fallback, "not-installed") };
            // A model catalog with each model's efforts (tested 2026-10-08)
            let r = run_short(&path, &["debug".into(), "models".into()], "", 45000, None);
            if r.code != Some(0) {
                return failed(fallback, r.spawn_error.as_deref().unwrap_or("Could not read Codex models"));
            }
            let Ok(j) = serde_json::from_str::<Value>(&r.stdout) else { return failed(fallback, "Could not read Codex models") };
            let options: Vec<Value> = j["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| sv(m, "visibility") == "list" && m["slug"].is_string())
                .map(|m| {
                    let efforts: Vec<&str> = m["supported_reasoning_levels"].as_array().into_iter().flatten().filter_map(|x| x.as_str().or(x["effort"].as_str())).collect();
                    let mut o = json!({ "id": m["slug"], "label": m.get("display_name").filter(|d| d.is_string()).unwrap_or(&m["slug"]), "efforts": efforts });
                    if let Some(d) = m.get("default_reasoning_level").filter(|d| d.is_string()) {
                        o["defaultEffort"] = d.clone();
                    }
                    o
                })
                .collect();
            json!({ "agent": agent, "options": options, "fallback": fallback })
        }
        _ => {
            let fallback = json!({ "model": "", "effort": sh::AGY_DEFAULT_EFFORT, "source": "cli-default" });
            let Some(path) = path else { return failed(fallback, "not-installed") };
            let env = match prepare_agy_home(core) {
                Ok(e) => e,
                Err(e) => return failed(fallback, &e),
            };
            let r = run_short(&path, &["models".into()], "", 45000, Some(env));
            if r.code != Some(0) {
                return failed(fallback, r.spawn_error.as_deref().unwrap_or("Could not read Antigravity models"));
            }
            // Lines of id<TAB>name. Ids carry the effort, merged here into one model with its efforts.
            let list: Vec<(String, String)> = r
                .stdout
                .lines()
                .filter_map(|l| {
                    let (a, b) = l.split_once('\t')?;
                    let b = b.split('\t').next().unwrap_or(b);
                    let valid = !sv(&sh::sanitize_model_choice(Some(&json!({ "model": a.trim() }))), "model").is_empty();
                    valid.then(|| (a.trim().to_string(), b.trim().to_string()))
                })
                .collect();
            json!({ "agent": agent, "options": sh::group_agy_models(&list), "fallback": fallback })
        }
    }
}

fn model_catalog(core: &Arc<Core>, agent: &str, force: bool) -> Value {
    let app = &core.features().cowork;
    if !force {
        if let Some((at, v)) = app.catalogs.lock().unwrap().get(agent) {
            if at.elapsed() < CATALOG_TTL {
                return v.clone();
            }
        }
    }
    let v = load_catalog(core, agent);
    app.known.lock().unwrap().insert(agent.to_string(), v.clone());
    // A failure should not be remembered for long
    if v.get("error").is_some() {
        app.catalogs.lock().unwrap().remove(agent);
    } else {
        app.catalogs.lock().unwrap().insert(agent.to_string(), (Instant::now(), v.clone()));
    }
    v
}

const MAX_SKILL_BYTES: usize = 24 * 1024;

/// Paths come from the inventory here, never from the page
fn read_selected_skills(ws: &str, keys: &[String]) -> Vec<Value> {
    if keys.is_empty() {
        return Vec::new();
    }
    let items = build_inventory_public(ws);
    let mut out = Vec::new();
    for key in keys {
        let Some(item) = items.iter().find(|i| sv(i, "kind") == "skill" && sv(i, "id") == key) else { continue };
        for a in item["agents"].as_array().into_iter().flatten() {
            let d = sv(a, "detail");
            if crate::util::basename(d) != "SKILL.md" {
                continue;
            }
            if let Some(text) = read_text(Path::new(d)) {
                out.push(json!({ "key": key, "name": item["name"], "content": js_prefix(&text, MAX_SKILL_BYTES) }));
                break;
            }
        }
    }
    out
}

/// codex's Windows sandbox cannot read the user folder (tested): warn up front if the repo is under it
fn repo_warnings(common_dir: &str) -> Vec<String> {
    if !cfg!(windows) {
        return Vec::new();
    }
    let h = p(&home()).to_lowercase();
    if common_dir.to_lowercase().starts_with(&format!("{h}\\")) {
        vec!["codex-repo-in-profile".to_string()]
    } else {
        Vec::new()
    }
}

fn baseline_info(ws: &str) -> Value {
    match wb_git::read_baseline(Path::new(ws)) {
        Ok(b) => {
            let dirty: Vec<&String> = b.dirty.iter().take(200).collect();
            let mut warnings = b.warnings.clone();
            warnings.extend(repo_warnings(&b.common_dir));
            json!({ "ok": true, "root": b.root, "branch": b.branch, "head": b.head, "dirty": dirty, "dirtyCount": b.dirty.len(), "warnings": warnings })
        }
        Err(e) => json!({ "ok": false, "code": e }),
    }
}

fn svc(core: &Core) -> CR<Arc<Service>> {
    core.features().cowork.service.lock().unwrap().clone().ok_or_else(|| cerr("not-ready"))
}

pub fn start_service(core: &Arc<Core>) {
    let service = Service::new(Arc::new(Deps { core: Arc::downgrade(core) }));
    service.init();
    *core.features().cowork.service.lock().unwrap() = Some(service);
}

pub fn shutdown(core: &Core) {
    if let Ok(s) = svc(core) {
        s.shutdown();
    }
}

fn start_run(core: &Arc<Core>, ws: &str, req: &Value) -> CR<Value> {
    let cw = sh::sanitize_cowork_settings(core.load_settings().get("cowork"));
    let participants: Vec<String> = sh::strs(&req["participants"]).into_iter().filter(|a| sh::COWORK_AGENTS.contains(&a.as_str())).collect();
    let discussion = sv(req, "mode") == "discussion";
    let recorder = req.get("summarizer").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string).or_else(|| cw["summarizer"].as_str().filter(|s| participants.iter().any(|x| x == s)).map(str::to_string));
    let run = svc(core)?.start(StartOptions {
        mode: Some(if discussion { "discussion" } else { "project" }.to_string()),
        auto_effort: req.get("autoEffort") != Some(&json!(false)),
        summarizer: recorder,
        summarize_each_round: req.get("summarizeEachRound").and_then(Value::as_bool).unwrap_or(cw["summarizeEachRound"] == json!(true)),
        workspace: ws.to_string(),
        skills: if discussion { Vec::new() } else { read_selected_skills(ws, &sh::strs(&cw["skills"])) },
        project_instructions: cw["projectInstructions"] != json!(false),
        prompt: req.get("prompt").and_then(Value::as_str).map(str::to_string),
        chair: sv(req, "chair").to_string(),
        participants,
        chair_executes: cw["chairExecutes"] == json!(true),
        language: if sv(req, "language") == "zh-TW" { "zh-TW" } else { "en" }.to_string(),
        limits: cw["limits"].clone(),
        // A phone sends no models: keep the remembered ones, or remembering below would clear them
        models: Some(sh::sanitize_model_choices(Some(req.get("models").filter(|m| !m.is_null()).unwrap_or(&cw["models"])))),
    })?;
    // Remember this meeting's choices as next time's defaults (cowork.md §8). Going back to default clears the old value.
    let mut models = cw["models"].clone();
    for a in sh::strs(&run["participants"]) {
        match run["models"].get(&a) {
            Some(m) => models[&a] = m.clone(),
            None => {
                if let Some(o) = models.as_object_mut() {
                    o.shift_remove(&a);
                }
            }
        }
    }
    let d = &run["discussion"];
    let summary_changed = d.is_object() && (cw["summarizer"] != d["summarizer"] || cw["summarizeEachRound"] != d["summarizeEachRound"]);
    if cw["chair"] != run["chair"] || cw["participants"] != run["participants"] || cw["models"].to_string() != models.to_string() || summary_changed {
        let mut next = cw.clone();
        next["chair"] = run["chair"].clone();
        next["participants"] = run["participants"].clone();
        next["models"] = models;
        if d.is_object() {
            next["summarizer"] = d["summarizer"].clone();
            next["summarizeEachRound"] = json!(d["summarizeEachRound"] == json!(true));
        }
        let mut s = core.load_settings();
        s["cowork"] = next;
        let _ = core.save_settings(&s);
    }
    Ok(run)
}

fn exec_choice(o: Option<&Value>) -> (String, bool) {
    let mode = if o.map(|x| sv(x, "mode")) == Some("parallel") { "parallel" } else { "sequential" };
    (mode.to_string(), o.and_then(|x| x.get("linkDeps")) != Some(&json!(false)))
}

/// Every Cowork operation. The desktop calls cowork:<op>, the phone goes through its allowlist; both land here.
pub fn invoke(core: &Arc<Core>, op: &str, ws: &str, args: &[Value]) -> CR<Value> {
    let a = |i: usize| args.get(i).unwrap_or(&Value::Null);
    let text = |i: usize| -> String {
        match a(i) {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            v => v.to_string(),
        }
    };
    let id = text(0);
    let done = |r: CR<()>| r.map(|_| Value::Null);
    let cw = || sh::sanitize_cowork_settings(core.load_settings().get("cowork"));
    match op {
        "capabilities" => {
            let c = cw();
            Ok(json!({ "agents": capabilities(core, a(0) == &json!(true)), "baseline": baseline_info(ws), "defaults": { "chair": c["chair"], "participants": c["participants"], "models": c["models"], "summarizer": c["summarizer"], "summarizeEachRound": c["summarizeEachRound"], "limits": c["limits"] } }))
        }
        "models" => {
            let force = a(0) == &json!(true);
            Ok(std::thread::scope(|s| {
                let hs: Vec<_> = sh::COWORK_AGENTS.iter().map(|ag| s.spawn(move || model_catalog(core, ag, force))).collect();
                Value::Array(hs.into_iter().map(|h| h.join().unwrap()).collect())
            }))
        }
        "list" => svc(core)?.list(ws),
        "get" => Ok(svc(core)?.get(&id)),
        "start" => start_run(core, ws, a(0)),
        "cancel" => done(svc(core)?.cancel(&id)),
        "discuss" => done(svc(core)?.discuss(&id, &text(1))),
        "summarizer" => done(svc(core)?.set_summarizer(&id, &text(1))),
        "summarize" => done(svc(core)?.summarize_discussion(&id, a(1) == &json!(true))),
        "retry" => done(svc(core)?.retry(&id)),
        "drop" => done(svc(core)?.drop_failed_reviewers(&id)),
        "note" => done(svc(core)?.add_note(&id, &text(1))),
        "feedback" => done(svc(core)?.feedback(&id, &text(1))),
        "editBoard" => svc(core)?.edit_board(&id, a(1).as_f64().unwrap_or(f64::NAN), a(2)),
        "dismiss" => done(svc(core)?.dismiss_unresolved(&id)),
        "approve" => done(svc(core)?.approve(&id, a(1).as_f64().unwrap_or(f64::NAN))),
        "raiseLimits" => done(svc(core)?.raise_limits(&id, a(1)["maxPlanningCalls"].as_f64().unwrap_or(0.0), a(1)["maxPlanningMinutes"].as_f64().unwrap_or(0.0))),
        "logDispatch" => done(svc(core)?.log_dispatch(&id, &text(1), &text(2))),
        "delete" => done(svc(core)?.delete(&id)),
        "execPreview" => {
            let (mode, link) = exec_choice(args.get(1));
            let s = svc(core)?;
            let run = s.must(&id)?;
            let r = run.lock().unwrap();
            let c = cw();
            exec::preview(&s, &r, &mode, link, core.is_cli_bypass(), c["worktreeRoot"].as_str())
        }
        "execStart" => {
            let (mode, link_deps) = exec_choice(args.get(1));
            let c = cw();
            let o = exec::StartOpts { mode, link_deps, bypass: core.is_cli_bypass(), approval: a(1).get("approval").and_then(Value::as_str).map(str::to_string), review: c["execReview"] != json!(false), worktree_root: c["worktreeRoot"].as_str().map(str::to_string), runaway_stop: c["runawayStop"] == json!(true) };
            done(svc(core)?.exec_start(&id, &o))
        }
        "execDiff" => exec::diff(&svc(core)?.must(&id)?),
        "execWaive" => {
            let s = svc(core)?;
            done(exec::waive(&s, &s.must(&id)?, &text(1), &text(2), false))
        }
        // Not on the phone's allowlist: a changed git config is only let through at the computer
        "execWaiveGitConfig" => {
            let s = svc(core)?;
            done(exec::waive(&s, &s.must(&id)?, &text(1), &text(2), true))
        }
        "execMessage" => {
            let s = svc(core)?;
            done(exec::message(&s, &s.must(&id)?, &text(1), &text(2)))
        }
        "execRetry" => {
            let s = svc(core)?;
            done(exec::retry(&s, &s.must(&id)?, &text(1)))
        }
        "execAcceptScope" => {
            let s = svc(core)?;
            done(exec::accept_scope(&s, &s.must(&id)?, &text(1), a(2).as_str()))
        }
        "execRevertScope" => {
            let s = svc(core)?;
            done(exec::revert_scope(&s, &s.must(&id)?, &text(1)))
        }
        "execPause" => {
            let s = svc(core)?;
            done(exec::pause(&s, &s.must(&id)?))
        }
        "execResume" => {
            let s = svc(core)?;
            done(exec::resume(&s, &s.must(&id)?))
        }
        "execMerge" => {
            let s = svc(core)?;
            let approval = a(1).get("commit").filter(|c| c.is_string()).map(|c| {
                let mut o = json!({ "commit": c, "targetHead": a(1).get("targetHead").and_then(Value::as_str).unwrap_or("") });
                if let Some(w) = a(1).get("waive").and_then(Value::as_str) {
                    o["waive"] = json!(w);
                }
                o
            });
            done(exec::merge(&s, &s.must(&id)?, approval.as_ref()))
        }
        "execCleanup" => {
            let s = svc(core)?;
            done(exec::cleanup(&s, &s.must(&id)?))
        }
        "bypass" => Ok(json!(core.is_cli_bypass())),
        _ => Err(cerr_m("unknown-op", op)),
    }
}

pub fn wrap(r: CR<Value>) -> Value {
    match r {
        Ok(data) => json!({ "ok": true, "data": data }),
        Err(CoworkError { code, message, data }) => {
            let mut o = json!({ "ok": false, "code": code, "message": message });
            if let Some(d) = data {
                o["data"] = d;
            }
            o
        }
    }
}

pub fn call(core: &Arc<Core>, window: u32, channel: &str, args: &[Value]) -> Result<Value, String> {
    let op = channel.strip_prefix("cowork:").unwrap_or("");
    let ws = core.workspace_for(Some(window));
    // A panic in a handler must come back as an error, not take the app along
    Ok(wrap(invoke(core, op, &ws, args)))
}
