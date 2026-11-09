// The Cowork meeting: R1 -> R2 -> R3/R4 -> awaiting approval, budgets, cancel, retry, board edits, approval. Discussion mode too.
// Deps are injected, so scripts/check-cowork.mts runs all of it with a fake CLI on a temporary repo.
//
// Concurrency: each run is one Mutex<Value>. A decision and the state it depends on change under the same lock; the lock is
// never held across a process or a slow git call. A generation counter lets a call that comes back late see it no longer owns the run.
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use wb_proc::runner::{run_process_with, OutputSink};
use wb_proc::{CancelToken, RunSpec};

use crate::cowork_exec as exec;
use crate::cowork_shared as sh;
use crate::util::{js_len, js_prefix, sv};

pub type Run = Arc<Mutex<Value>>;

#[derive(Debug, Clone)]
pub struct CoworkError {
    pub code: String,
    pub message: String,
    pub data: Option<Value>,
}

pub fn cerr(code: &str) -> CoworkError {
    CoworkError { code: code.into(), message: code.into(), data: None }
}

pub fn cerr_m(code: &str, message: impl Into<String>) -> CoworkError {
    CoworkError { code: code.into(), message: message.into(), data: None }
}

pub fn cerr_d(code: &str, message: Option<String>, data: Value) -> CoworkError {
    CoworkError { code: code.into(), message: message.unwrap_or_else(|| code.into()), data: Some(data) }
}

pub type CR<T> = Result<T, CoworkError>;

#[derive(Default, Clone)]
pub struct ResolvedCli {
    pub command: String,
    pub prefix_args: Vec<String>,
    pub windows_sandbox: Option<String>,
    pub env: Option<Vec<(String, String)>>,
    /// Used when the user picked nothing. Planning ignores codex's config.toml, which would silently change the model otherwise.
    pub default_model: String,
    pub default_effort: String,
    pub model_efforts: HashMap<String, Vec<String>>,
}

pub trait CoworkDeps: Send + Sync {
    fn data_dir(&self) -> PathBuf;
    fn resolve_cli(&self, agent: &str) -> Result<ResolvedCli, String>;
    /// The user's normal CLI with its full setup, no isolated home
    fn resolve_exec_cli(&self, agent: &str) -> Result<ResolvedCli, String>;
    fn emit(&self, run: &Value);
    fn now(&self) -> f64 {
        crate::util::now_ms()
    }
    /// A CLI just started: put it in the job, so a crash takes it along
    fn on_pid(&self, _pid: u32) {}
}

/// One headless run: prompt on stdin, output capped, timeout and cancel take the process tree.
pub struct Proc {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub cancelled: bool,
    pub truncated: bool,
    pub ms: u64,
    pub spawn_error: Option<String>,
    pub guard_reason: Option<String>,
}

struct LineSink<'a> {
    out: Vec<u8>,
    err: Vec<u8>,
    line: Vec<u8>,
    on_activity: &'a mut (dyn FnMut(usize) + Send),
    on_line: &'a mut (dyn FnMut(&str) -> Option<String> + Send),
    on_spawn: &'a mut (dyn FnMut() + Send),
    guard: Option<String>,
    cancel: CancelToken,
}

impl OutputSink for LineSink<'_> {
    fn on_spawn(&mut self, _pid: u32) {
        (self.on_spawn)();
    }
    fn on_output(&mut self, stream: u8, bytes: &[u8]) {
        if stream == 0 {
            self.out.extend_from_slice(bytes);
        } else {
            self.err.extend_from_slice(bytes);
        }
        (self.on_activity)(self.out.len() + self.err.len());
        if stream != 0 || self.guard.is_some() {
            return;
        }
        self.line.extend_from_slice(bytes);
        while let Some(i) = self.line.iter().position(|b| *b == b'\n') {
            let rest = self.line.split_off(i + 1);
            let full = std::mem::replace(&mut self.line, rest);
            let text = String::from_utf8_lossy(&full[..i]).into_owned();
            if let Some(why) = (self.on_line)(&text) {
                self.guard = Some(why);
                self.cancel.cancel();
                break;
            }
        }
    }
}

pub struct ProcHooks<'a> {
    pub on_activity: &'a mut (dyn FnMut(usize) + Send),
    pub on_line: &'a mut (dyn FnMut(&str) -> Option<String> + Send),
    pub on_spawn: &'a mut (dyn FnMut() + Send),
}

pub fn run_proc(spec: &RunSpec, cancel: &CancelToken, hooks: ProcHooks, on_pid: impl FnOnce(u32)) -> Proc {
    let mut sink = LineSink { out: Vec::new(), err: Vec::new(), line: Vec::new(), on_activity: hooks.on_activity, on_line: hooks.on_line, on_spawn: hooks.on_spawn, guard: None, cancel: cancel.clone() };
    let r = run_process_with(spec, cancel, &mut sink, on_pid);
    Proc {
        code: r.code,
        stdout: String::from_utf8_lossy(&sink.out).into_owned(),
        stderr: String::from_utf8_lossy(&sink.err).into_owned(),
        timed_out: r.timed_out,
        // A guard stop is not the user cancelling
        cancelled: r.cancelled && sink.guard.is_none(),
        truncated: r.truncated,
        ms: r.ms,
        spawn_error: r.spawn_error,
        guard_reason: sink.guard,
    }
}

/// Atomic manifest write: temp file, fsync, keep the previous copy, rename. Windows antivirus can hold a file briefly, so retry.
pub fn write_json_atomic(file: &Path, data: &Value) -> Result<(), String> {
    let dir = file.parent().ok_or("no parent")?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.{}.{}.tmp", std::process::id(), crate::util::now_ms() as u64));
    {
        let mut f = fs::File::create(&tmp).map_err(|e| e.to_string())?;
        f.write_all(serde_json::to_string_pretty(data).unwrap().as_bytes()).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
    }
    let retry = |op: &dyn Fn() -> std::io::Result<()>| -> std::io::Result<()> {
        let mut last = Ok(());
        for i in 0..6u64 {
            last = op();
            match &last {
                Ok(()) => return Ok(()),
                Err(e) if matches!(e.raw_os_error(), Some(5) | Some(32)) || e.kind() == std::io::ErrorKind::PermissionDenied => std::thread::sleep(Duration::from_millis(25 * (i + 1))),
                Err(_) => return last,
            }
        }
        last
    };
    let result = (|| -> std::io::Result<()> {
        if file.exists() {
            retry(&|| fs::copy(file, prev_path(file)).map(|_| ()))?;
        }
        retry(&|| fs::rename(&tmp, file))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|e| e.to_string())
}

pub fn prev_path(file: &Path) -> PathBuf {
    let s = file.to_string_lossy();
    PathBuf::from(format!("{}.prev.json", s.strip_suffix(".json").unwrap_or(&s)))
}

/// The main file, or the previous copy if it is damaged or the wrong shape
pub fn read_json_with_fallback(file: &Path, valid: &dyn Fn(&Value) -> bool) -> Option<(Value, bool)> {
    for (f, backup) in [(file.to_path_buf(), false), (prev_path(file), true)] {
        if let Some(v) = crate::util::read_text(&f).and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            if valid(&v) {
                return Some((v, backup));
            }
        }
    }
    None
}

fn is_run(v: &Value) -> bool {
    v["schemaVersion"] == json!(1) && v["id"].is_string() && v["log"].is_array() && v["calls"].is_array() && v["boards"].is_array()
}

fn real_path(p: &str) -> String {
    let abs = crate::util::resolve(p);
    let c = fs::canonicalize(&abs).map(|c| crate::util::p(&c)).unwrap_or(abs);
    match c.strip_prefix(r"\\?\UNC\") {
        Some(rest) => format!(r"\\{rest}"),
        None => c.strip_prefix(r"\\?\").unwrap_or(&c).to_string(),
    }
}

/// A repo's identity: the normalized git common dir, case-insensitive on Windows
pub fn repo_id_of(common_dir: &str) -> String {
    let mut p = real_path(common_dir);
    if cfg!(windows) {
        p = p.to_lowercase();
    }
    Sha256::digest(p.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn to_base36(mut n: u64) -> String {
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    while n > 0 {
        out.push(D[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

fn new_run_id() -> String {
    let bytes = uuid::Uuid::new_v4();
    format!("r{}{:02x}{:02x}", to_base36(crate::util::now_ms() as u64), bytes.as_bytes()[0], bytes.as_bytes()[1])
}

pub fn snapshot_dir_for(common_dir: &str, id: &str) -> String {
    crate::util::p(&Path::new(common_dir).join("cowork").join(id).join("snapshot"))
}

/// Git marks some files read-only on Windows, and a plain delete then fails
fn remove_tree(dir: &Path) -> std::io::Result<()> {
    if fs::remove_dir_all(dir).is_ok() || !dir.exists() {
        return Ok(());
    }
    fn walk(p: &Path) {
        for e in fs::read_dir(p).into_iter().flatten().flatten() {
            let t = e.file_type();
            if t.as_ref().map(|t| t.is_dir() && !t.is_symlink()).unwrap_or(false) {
                walk(&e.path());
            } else if let Ok(m) = e.metadata() {
                let mut perm = m.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                perm.set_readonly(false);
                let _ = fs::set_permissions(e.path(), perm);
            }
        }
    }
    walk(dir);
    fs::remove_dir_all(dir)
}

/// Only deletes under must_be_under, never the repo or where an outside link points
pub fn remove_snapshot(dir: &str, must_be_under: &str) -> Result<(), String> {
    if !Path::new(dir).exists() {
        return Ok(());
    }
    let (real, base) = (real_path(dir), real_path(must_be_under));
    let (r, b) = (real.to_lowercase(), base.to_lowercase());
    if r == b || !r.starts_with(&format!("{b}{}", std::path::MAIN_SEPARATOR)) {
        return Err(format!("refusing to delete {real}: not under {base}"));
    }
    remove_tree(Path::new(&real)).map_err(|e| e.to_string())?;
    // Empty run and cowork folders go too
    let mut p = crate::util::dirname(&real);
    while p.to_lowercase() != b && p.to_lowercase().starts_with(&b) {
        if fs::remove_dir(&p).is_err() {
            break;
        }
        p = crate::util::dirname(&p);
    }
    let _ = fs::remove_dir(&base);
    Ok(())
}

/// A scope path that leaves the tree through a symlink or junction. For a file not yet created, the longest existing prefix counts.
pub fn scope_escapes(tree_root: &str, rel: &str) -> Option<String> {
    let base = real_path(tree_root);
    let mut probe = PathBuf::from(tree_root).join(rel);
    while !probe.exists() {
        match probe.parent() {
            Some(x) if x != probe => probe = x.to_path_buf(),
            _ => return None,
        }
    }
    let real = real_path(&crate::util::p(&probe));
    let (r, b) = (real.to_lowercase(), base.to_lowercase());
    if r != b && !r.starts_with(&format!("{b}{}", std::path::MAIN_SEPARATOR)) {
        return Some(format!("{rel} resolves outside the repository ({real})"));
    }
    None
}

/// `.cowork/` in the local exclude file: git status stays clean and the tracked .gitignore is left alone
pub fn exclude_cowork_dir(common_dir: &str) {
    let file = Path::new(common_dir).join("info").join("exclude");
    let text = crate::util::read_text(&file).unwrap_or_default();
    if text.lines().any(|l| l.trim() == ".cowork/") {
        return;
    }
    if let Some(d) = file.parent() {
        let _ = fs::create_dir_all(d);
    }
    let sep = if !text.is_empty() && !text.ends_with('\n') { "\n" } else { "" };
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&file) {
        let _ = write!(f, "{sep}# Agent Workbench Cowork worktrees\n.cowork/\n");
    }
}

/// Added or changed since the baseline; removal counts too
pub fn diff_guard(base: &Value, now: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for (k, v) in now.as_object().into_iter().flatten() {
        if base.get(k) != Some(v) {
            out.push(k.clone());
        }
    }
    for k in base.as_object().into_iter().flatten().map(|x| x.0) {
        if now.get(k).is_none() {
            out.push(k.clone());
        }
    }
    out
}

const INSTRUCTION_FILES: [&str; 3] = ["CLAUDE.md", "AGENTS.md", "GEMINI.md"];
const MAX_CONTEXT_FILE: usize = 24 * 1024;
const MAX_CONTEXT_TOTAL: usize = 96 * 1024;
const RAW_LIMIT: usize = 256 * 1024;
const ACTIVITY_EMIT_MS: f64 = 1000.0;
const EXEC_REVIEW_MAX_DIFF: usize = 200_000;

pub struct StartOptions {
    pub mode: Option<String>,
    pub auto_effort: bool,
    pub summarizer: Option<String>,
    pub summarize_each_round: bool,
    pub workspace: String,
    pub prompt: Option<String>,
    pub chair: String,
    pub participants: Vec<String>,
    pub chair_executes: bool,
    pub language: String,
    pub limits: Value,
    pub models: Option<Value>,
    pub skills: Vec<Value>,
    pub project_instructions: bool,
}

#[derive(Debug)]
pub enum StepErr {
    Failed(String, Vec<String>),
    Budget(String, Vec<String>),
    SideEffects(String, Vec<String>),
    Cancelled,
}

impl StepErr {
    fn rank(&self) -> u8 {
        match self {
            StepErr::SideEffects(..) => 0,
            StepErr::Budget(..) => 1,
            StepErr::Failed(..) => 2,
            StepErr::Cancelled => 3,
        }
    }
    pub fn message(&self) -> String {
        match self {
            StepErr::Failed(m, _) | StepErr::Budget(m, _) | StepErr::SideEffects(m, _) => m.clone(),
            StepErr::Cancelled => "cancelled".into(),
        }
    }
}

type Validate<'a> = &'a (dyn Fn(&Value, &Value) -> sh::Check + Sync);

#[derive(Default)]
struct Tracking {
    gen: HashMap<String, u64>,
    tokens: HashMap<String, Vec<CancelToken>>,
    abort_reason: HashMap<String, &'static str>,
    active_calls: HashMap<String, u32>,
    inflight: HashMap<String, u32>,
    last_emit: HashMap<String, f64>,
    execution_starting: HashMap<String, String>,
}

pub struct Service {
    pub deps: Arc<dyn CoworkDeps>,
    runs: Mutex<HashMap<String, Run>>,
    dirs: Mutex<HashMap<String, PathBuf>>,
    t: Mutex<Tracking>,
    snapshot_lock: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    shutting_down: AtomicBool,
    pub exec: exec::ExecState,
    me: Weak<Service>,
}

fn del(o: &mut Value, k: &str) {
    if let Some(m) = o.as_object_mut() {
        m.shift_remove(k);
    }
}

pub fn log_push(run: &mut Value, entry: Value) {
    run["log"].as_array_mut().unwrap().push(entry);
}

pub fn add_num(slot: &mut Value, x: f64) {
    let v = slot.as_f64().unwrap_or(0.0) + x;
    *slot = if v.fract() == 0.0 && v.abs() < 9e15 { json!(v as i64) } else { json!(v) };
}

impl Service {
    pub fn new(deps: Arc<dyn CoworkDeps>) -> Arc<Self> {
        let svc = Arc::new_cyclic(|me| Service { deps, runs: Mutex::default(), dirs: Mutex::default(), t: Mutex::default(), snapshot_lock: Mutex::default(), shutting_down: AtomicBool::new(false), exec: exec::ExecState::default(), me: me.clone() });
        svc.start_clock();
        svc
    }

    pub fn arc(&self) -> Arc<Service> {
        self.me.upgrade().expect("service alive")
    }

    pub fn now(&self) -> f64 {
        self.deps.now()
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    /// Budget timers for all runs. One thread: it only looks at runs with a call in flight.
    fn start_clock(&self) {
        let weak = self.me.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            let Some(svc) = weak.upgrade() else { return };
            let active: Vec<String> = svc.t.lock().unwrap().active_calls.iter().filter(|(_, n)| **n > 0).map(|(k, _)| k.clone()).collect();
            for id in active {
                let Some(run) = svc.get_run(&id) else { continue };
                let over = {
                    let r = run.lock().unwrap();
                    svc.ms_used(&r) >= r["limits"]["maxPlanningMinutes"].as_f64().unwrap_or(0.0) * 60_000.0
                };
                if over {
                    svc.abort_all(&id, "budget");
                }
            }
            exec::tick(&svc);
        });
    }

    pub fn get_run(&self, id: &str) -> Option<Run> {
        self.runs.lock().unwrap().get(id).cloned()
    }

    pub fn all_runs(&self) -> Vec<Run> {
        self.runs.lock().unwrap().values().cloned().collect()
    }

    fn must_get(&self, id: &str) -> CR<Run> {
        self.get_run(id).ok_or_else(|| cerr("run-not-found"))
    }

    pub fn dir_of(&self, run: &Value) -> PathBuf {
        let id = sv(run, "id").to_string();
        let mut dirs = self.dirs.lock().unwrap();
        dirs.entry(id.clone()).or_insert_with(|| self.deps.data_dir().join(repo_id_of(run["repo"]["commonDir"].as_str().unwrap_or(""))).join(&id)).clone()
    }

    /// Startup: a meeting in progress turns paused, for the user to look at (cowork.md design rule 5)
    pub fn init(&self) {
        let data = self.deps.data_dir();
        let Ok(repos) = fs::read_dir(&data) else { return };
        for repo in repos.flatten().filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false)) {
            let Ok(rd) = fs::read_dir(repo.path()) else { continue };
            for e in rd.flatten() {
                let dir = e.path();
                let Some((mut run, from_backup)) = read_json_with_fallback(&dir.join("run.json"), &is_run) else { continue };
                if !run["models"].is_object() {
                    run["models"] = json!({});
                }
                let id = sv(&run, "id").to_string();
                let mut changed = from_backup;
                let updated = run["updatedAt"].clone();
                for c in run["calls"].as_array_mut().unwrap() {
                    if sv(c, "status") == "running" {
                        c["status"] = json!("interrupted");
                        if !c.get("endedAt").map(crate::settings::truthy).unwrap_or(false) {
                            c["endedAt"] = updated.clone();
                        }
                        changed = true;
                    }
                }
                if let Some(rv) = run["reviewers"].as_object_mut() {
                    for r in rv.values_mut() {
                        if sv(r, "status") == "running" {
                            r["status"] = json!("pending");
                            changed = true;
                        }
                    }
                }
                if let Some(since) = run["budget"]["activeSince"].as_f64().filter(|s| *s != 0.0) {
                    let extra = (run["updatedAt"].as_f64().unwrap_or(since) - since).max(0.0);
                    add_num(&mut run["budget"]["planningMsUsed"], extra);
                    run["budget"]["activeSince"] = Value::Null;
                    changed = true;
                }
                if sv(&run, "phase") == "meeting" {
                    let now = self.now();
                    run["phase"] = json!("paused");
                    let msg = "The app restarted while the meeting was running.";
                    run["block"] = json!({ "kind": "restart", "message": msg, "at": now as i64 });
                    log_push(&mut run, json!({ "t": "blocked", "kind": "restart", "message": msg, "at": now as i64 }));
                    if run["pending"].is_null() {
                        run["pending"] = next_step(&run).map(|s| json!({ "step": s })).unwrap_or(Value::Null);
                    }
                    changed = true;
                }
                if exec::recover(self, &mut run) {
                    changed = true;
                }
                self.dirs.lock().unwrap().insert(id.clone(), dir);
                if changed {
                    self.save_with(&mut run, false);
                }
                self.runs.lock().unwrap().insert(id, Arc::new(Mutex::new(run)));
            }
        }
    }

    /// App quit: every running CLI ends; the next start turns meetings paused
    pub fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        let t = self.t.lock().unwrap();
        for list in t.tokens.values() {
            for c in list {
                c.cancel();
            }
        }
    }

    pub fn list(&self, workspace: &str) -> CR<Value> {
        let (_, common) = discussion_identity(workspace).map_err(|e| cerr_m(&e, e.clone()))?;
        let repo_id = repo_id_of(&common);
        let ws_id = repo_id_of(workspace);
        let mut rows: Vec<Value> = Vec::new();
        for run in self.all_runs() {
            let r = run.lock().unwrap();
            if repo_id_of(r["repo"]["commonDir"].as_str().unwrap_or("")) == repo_id || (sv(&r, "mode") == "discussion" && repo_id_of(r["repo"]["root"].as_str().unwrap_or("")) == ws_id) {
                rows.push(sh::summarize_run(&r));
            }
        }
        rows.sort_by(|a, b| b["createdAt"].as_f64().unwrap_or(0.0).total_cmp(&a["createdAt"].as_f64().unwrap_or(0.0)));
        Ok(json!(rows))
    }

    pub fn get(&self, id: &str) -> Value {
        self.get_run(id).map(|r| r.lock().unwrap().clone()).unwrap_or(Value::Null)
    }

    pub fn save_with(&self, run: &mut Value, emit: bool) {
        add_num(&mut run["revision"], 1.0);
        run["updatedAt"] = json!(self.now() as i64);
        let file = self.dir_of(run).join("run.json");
        if let Err(e) = write_json_atomic(&file, run) {
            // Not writable means no moving forward: stop at blocked rather than drift from the disk
            eprintln!("[cowork] failed to save run {}: {e}", sv(run, "id"));
            if sv(run, "phase") == "meeting" {
                self.abort_all(sv(run, "id"), "cancel");
                run["phase"] = json!("blocked");
                run["block"] = json!({ "kind": "step-failed", "message": format!("Could not save the run: {e}"), "at": self.now() as i64 });
            }
        }
        if emit {
            self.deps.emit(run);
        }
    }

    pub fn save(&self, run: &mut Value) {
        self.save_with(run, true);
    }

    pub fn emit_soon(&self, run: &Value) {
        let now = self.now();
        let id = sv(run, "id").to_string();
        let mut t = self.t.lock().unwrap();
        let last = t.last_emit.get(&id).copied().unwrap_or(0.0);
        if now - last >= ACTIVITY_EMIT_MS {
            t.last_emit.insert(id, now);
            drop(t);
            self.deps.emit(run);
        }
    }

    fn gen_of(&self, id: &str) -> u64 {
        self.t.lock().unwrap().gen.get(id).copied().unwrap_or(0)
    }

    fn next_gen(&self, id: &str) {
        let mut t = self.t.lock().unwrap();
        *t.gen.entry(id.to_string()).or_insert(0) += 1;
        t.abort_reason.remove(id);
    }

    fn inflight(&self, id: &str) -> u32 {
        self.t.lock().unwrap().inflight.get(id).copied().unwrap_or(0)
    }

    pub fn register_token(&self, id: &str, token: &CancelToken) {
        self.t.lock().unwrap().tokens.entry(id.to_string()).or_default().push(token.clone());
    }

    pub fn unregister_token(&self, id: &str, token: &CancelToken) {
        if let Some(list) = self.t.lock().unwrap().tokens.get_mut(id) {
            list.retain(|c| !c.same(token));
        }
    }

    pub fn cancel_tokens(&self, id: &str) {
        if let Some(list) = self.t.lock().unwrap().tokens.get(id) {
            for c in list {
                c.cancel();
            }
        }
    }

    pub fn abort_all(&self, id: &str, reason: &'static str) {
        {
            let mut t = self.t.lock().unwrap();
            t.abort_reason.entry(id.to_string()).or_insert(reason);
        }
        self.cancel_tokens(id);
    }

    fn ms_used(&self, r: &Value) -> f64 {
        let used = r["budget"]["planningMsUsed"].as_f64().unwrap_or(0.0);
        match r["budget"]["activeSince"].as_f64() {
            Some(s) if s != 0.0 => used + self.now() - s,
            _ => used,
        }
    }

    fn clock_start(&self, r: &mut Value) {
        let id = sv(r, "id").to_string();
        let n = {
            let mut t = self.t.lock().unwrap();
            let n = t.active_calls.entry(id).or_insert(0);
            *n += 1;
            *n
        };
        if n == 1 {
            r["budget"]["activeSince"] = json!(self.now() as i64);
        }
    }

    fn clock_stop(&self, r: &mut Value) {
        let id = sv(r, "id").to_string();
        let n = {
            let mut t = self.t.lock().unwrap();
            let n = t.active_calls.entry(id).or_insert(1);
            *n = n.saturating_sub(1);
            *n
        };
        if n == 0 {
            if let Some(s) = r["budget"]["activeSince"].as_f64().filter(|s| *s != 0.0) {
                let d = self.now() - s;
                add_num(&mut r["budget"]["planningMsUsed"], d);
            }
            r["budget"]["activeSince"] = Value::Null;
        }
    }

    pub fn start(&self, o: StartOptions) -> CR<Value> {
        let requested = self.now();
        let discussion = o.mode.as_deref() == Some("discussion");
        let prompt = o.prompt.as_deref().unwrap_or("").trim().to_string();
        if prompt.is_empty() {
            return Err(cerr("empty-prompt"));
        }
        if js_len(&prompt) > 20000 {
            return Err(cerr("prompt-too-long"));
        }
        let mut participants: Vec<String> = Vec::new();
        for a in &o.participants {
            if !participants.contains(a) {
                participants.push(a.clone());
            }
        }
        if !participants.contains(&o.chair) {
            return Err(cerr("chair-not-participant"));
        }
        if participants.len() < 2 {
            return Err(cerr("need-two-participants"));
        }
        if let Some(s) = o.summarizer.as_ref().filter(|s| !participants.contains(s)) {
            let _ = s;
            return Err(cerr_m("not-allowed", "The recorder must be a participant."));
        }
        for a in &participants {
            if let Err(e) = self.deps.resolve_cli(a) {
                return Err(cerr_d("agent-not-eligible", Some(format!("{a}: {e}")), json!({ "agent": a, "reason": e })));
            }
        }
        let (root, common, head, branch, dirty) = if discussion {
            let (root, common) = discussion_identity(&o.workspace).map_err(|e| cerr(&e))?;
            (root, common, String::new(), String::new(), Vec::new())
        } else {
            let b = wb_git::read_baseline(Path::new(&o.workspace)).map_err(|e| cerr(&e))?;
            (b.root, b.common_dir, b.head, b.branch, b.dirty)
        };
        let repo_id = repo_id_of(&common);
        let context = if discussion { None } else { build_context(&root, &head, &o) };
        let id = new_run_id();
        let now = self.now() as i64;
        let models: Map<String, Value> = sh::sanitize_model_choices(o.models.as_ref()).as_object().unwrap().iter().filter(|(a, _)| participants.contains(a)).map(|(k, v)| (k.clone(), v.clone())).collect();
        let snapshot = if discussion { crate::util::p(&self.deps.data_dir().join(&repo_id).join(&id).join("conversation")) } else { snapshot_dir_for(&common, &id) };
        let mut run = Map::new();
        run.insert("schemaVersion".into(), json!(1));
        if let Some(m) = &o.mode {
            run.insert("mode".into(), json!(m));
            run.insert("autoEffort".into(), json!(o.auto_effort));
        }
        if discussion {
            let order: Vec<&String> = std::iter::once(&o.chair).chain(participants.iter().filter(|a| **a != o.chair)).collect();
            run.insert("discussion".into(), json!({ "messages": [{ "id": "m1", "agent": null, "message": prompt, "replyTo": null, "at": now }], "order": order, "cursor": 0, "excluded": [], "summarizer": o.summarizer.as_ref().unwrap_or(&o.chair), "summarizeEachRound": o.summarize_each_round, "summaries": [] }));
        }
        let dirty_kept: Vec<&String> = dirty.iter().take(500).collect();
        let fields = json!({
            "id": id, "revision": 0, "planRevision": 1, "approvedPlanRevision": null, "prompt": prompt, "createdAt": now, "updatedAt": now, "language": o.language,
            "repo": { "root": root, "commonDir": common, "sourceBranch": branch, "baseCommit": head, "excludedDirty": dirty_kept },
            "snapshotDir": snapshot, "chair": o.chair, "participants": participants, "chairExecutes": o.chair_executes, "models": models,
            "phase": "meeting", "block": null, "pending": null, "notes": [],
            "limits": { "maxPlanningCalls": o.limits["maxPlanningCalls"], "maxPlanningMinutes": o.limits["maxPlanningMinutes"], "maxExecutionMinutes": o.limits["maxExecutionMinutes"], "callTimeoutSec": sh::DEFAULT_CALL_TIMEOUT_SEC, "maxOutputBytes": sh::DEFAULT_MAX_OUTPUT_BYTES, "maxTasks": sh::DEFAULT_MAX_TASKS },
            "budget": { "planningCallsUsed": 0, "planningMsUsed": 0, "activeSince": null, "costUsd": 0, "tokens": 0 },
            "r1": null, "reviewers": {}, "boards": [], "calls": [],
        });
        for (k, v) in fields.as_object().unwrap() {
            run.insert(k.clone(), v.clone());
        }
        if let Some(c) = context {
            run.insert("context".into(), c);
        }
        run.insert("log".into(), json!([{ "t": "start", "at": now }, { "t": "round", "round": 1, "at": now, "planRevision": 1 }]));
        let mut run = Value::Object(run);
        self.dirs.lock().unwrap().insert(id.clone(), self.deps.data_dir().join(&repo_id).join(&id));
        self.t.lock().unwrap().gen.insert(id.clone(), 0);
        self.save(&mut run);
        let cell: Run = Arc::new(Mutex::new(run));
        self.runs.lock().unwrap().insert(id.clone(), cell.clone());
        let made = if discussion { fs::create_dir_all(&snapshot).map_err(|e| e.to_string()) } else { wb_git::create_snapshot(Path::new(&root), Path::new(&snapshot), &head) };
        let mut r = cell.lock().unwrap();
        if let Err(e) = made {
            r["phase"] = json!("failed");
            let msg = format!("Could not create the planning snapshot: {e}");
            r["block"] = json!({ "kind": "step-failed", "message": msg, "at": self.now() as i64 });
            log_push(&mut r, json!({ "t": "blocked", "kind": "step-failed", "message": msg, "at": self.now() as i64 }));
            self.save(&mut r);
            return Ok(r.clone());
        }
        r["preparationMs"] = json!((self.now() - requested) as i64);
        self.save(&mut r);
        let out = r.clone();
        drop(r);
        self.spawn_advance(&cell);
        Ok(out)
    }

    fn spawn_advance(&self, run: &Run) {
        let (svc, run) = (self.arc(), run.clone());
        std::thread::spawn(move || svc.advance(&run));
    }

    /// Decide and run the next step, tied to the current generation
    fn advance(&self, run: &Run) {
        loop {
            let step = {
                let r = run.lock().unwrap();
                if sv(&r, "phase") != "meeting" || self.is_shutting_down() {
                    return;
                }
                r.pointer("/pending/step").and_then(Value::as_str).map(str::to_string).or_else(|| next_step(&r).map(str::to_string))
            };
            let Some(step) = step else { return };
            let again = match step.as_str() {
                "discussion" => self.step_discussion(run),
                "summary" => self.step_summary(run),
                "conclusion" => self.step_conclusion(run),
                "r1" => self.step_r1(run),
                "r2" => self.step_r2(run),
                "r34" => self.step_r34(run),
                _ => self.step_revise(run),
            };
            if !again {
                return;
            }
        }
    }

    fn step_discussion(&self, run: &Run) -> bool {
        let (agent, prompt, id, messages_for_check) = {
            let mut r = run.lock().unwrap();
            if sh::discussion_needs_summary(&r) {
                r["discussion"]["summaryNext"] = json!("discussion");
                r["pending"] = json!({ "step": "summary" });
                self.save(&mut r);
                return true;
            }
            let cursor = r["discussion"]["cursor"].as_u64().unwrap_or(0) as usize;
            let agent = r["discussion"]["order"].get(cursor).and_then(Value::as_str).map(str::to_string);
            let Some(agent) = agent else {
                r["pending"] = Value::Null;
                r["phase"] = json!("completed");
                self.save(&mut r);
                return false;
            };
            (agent.clone(), sh::build_discussion_prompt(&r, &agent), sv(&r, "id").to_string(), r["discussion"]["messages"].clone())
        };
        let g = self.gen_of(&id);
        let list = messages_for_check.as_array().cloned().unwrap_or_default();
        let validate = move |v: &Value, _: &Value| sh::check_discussion(v, &list);
        let res = self.run_step(run, "discussion", &agent, &prompt, &validate, None);
        let mut r = run.lock().unwrap();
        if g != self.gen_of(&id) || sv(&r, "phase") != "meeting" {
            return false;
        }
        match res {
            Err(e) => {
                self.step_failed(&mut r, "discussion", e);
                false
            }
            Ok(v) => {
                let d = &mut r["discussion"];
                let n = d["messages"].as_array().unwrap().len();
                let msg = json!({ "id": format!("m{}", n + 1), "agent": agent, "message": v["message"], "replyTo": v["replyTo"], "at": self.now() as i64 });
                d["messages"].as_array_mut().unwrap().push(msg);
                add_num(&mut d["cursor"], 1.0);
                let done = d["cursor"].as_u64() == Some(d["order"].as_array().unwrap().len() as u64);
                let each = d["summarizeEachRound"] == json!(true);
                r["pending"] = Value::Null;
                if done {
                    if each {
                        r["discussion"]["summaryNext"] = json!("idle");
                        r["pending"] = json!({ "step": "summary" });
                    } else {
                        r["phase"] = json!("completed");
                    }
                }
                self.save(&mut r);
                sv(&r, "phase") == "meeting"
            }
        }
    }

    /// A summary covers only its captured prefix; a failed or interrupted call never advances the checkpoint
    fn step_summary(&self, run: &Run) -> bool {
        let (next, through, agent, prompt, id) = {
            let mut r = run.lock().unwrap();
            let next = Some(sv(&r["discussion"], "summaryNext")).filter(|s| !s.is_empty()).unwrap_or("idle").to_string();
            let (_, through) = sh::discussion_summary_chunk(&r);
            let covered = sh::latest_discussion_summary(&r).and_then(|s| s["through"].as_u64()).unwrap_or(0) as usize;
            let mut work = None;
            if through > covered {
                let agent = Some(sv(&r["discussion"], "summarizer")).filter(|s| !s.is_empty()).unwrap_or(sv(&r, "chair")).to_string();
                if sh::strs(&r["discussion"]["excluded"]).contains(&agent) {
                    self.block(&mut r, "step-failed", "Choose another recorder: this participant was skipped.", Some("summary"), Vec::new());
                    return false;
                }
                work = Some((agent, sh::build_discussion_summary_prompt(&r)));
            }
            let id = sv(&r, "id").to_string();
            match work {
                Some((agent, prompt)) => (next, through, Some(agent), prompt, id),
                None => (next, through, None, String::new(), id),
            }
        };
        if let Some(agent) = agent {
            let g = self.gen_of(&id);
            let validate = |v: &Value, _: &Value| sh::check_discussion_summary(v);
            let res = self.run_step(run, "summary", &agent, &prompt, &validate, None);
            let mut r = run.lock().unwrap();
            if g != self.gen_of(&id) || sv(&r, "phase") != "meeting" {
                return false;
            }
            match res {
                Err(e) => {
                    self.step_failed(&mut r, "summary", e);
                    return false;
                }
                Ok(v) => {
                    let mut row = v.as_object().unwrap().clone();
                    row.insert("agent".into(), json!(agent));
                    row.insert("through".into(), json!(through));
                    row.insert("at".into(), json!(self.now() as i64));
                    if !r["discussion"]["summaries"].is_array() {
                        r["discussion"]["summaries"] = json!([]);
                    }
                    r["discussion"]["summaries"].as_array_mut().unwrap().push(Value::Object(row));
                    let total = r["discussion"]["messages"].as_array().unwrap().len();
                    // Manual and final summaries take every uncovered message, including in long old meetings
                    if through < total && (next != "discussion" || sh::discussion_needs_summary(&r)) {
                        r["pending"] = json!({ "step": "summary" });
                        self.save(&mut r);
                        return true;
                    }
                }
            }
        }
        let mut r = run.lock().unwrap();
        del(&mut r["discussion"], "summaryNext");
        r["pending"] = if next == "idle" { Value::Null } else { json!({ "step": next }) };
        if next == "idle" {
            r["phase"] = json!("completed");
        }
        self.save(&mut r);
        sv(&r, "phase") == "meeting"
    }

    fn step_conclusion(&self, run: &Run) -> bool {
        let (prompt, chair, id, list) = {
            let r = run.lock().unwrap();
            (sh::build_discussion_conclusion_prompt(&r), sv(&r, "chair").to_string(), sv(&r, "id").to_string(), r["discussion"]["messages"].as_array().cloned().unwrap_or_default())
        };
        let g = self.gen_of(&id);
        let validate = move |v: &Value, _: &Value| sh::check_discussion(v, &list);
        let res = self.run_step(run, "conclusion", &chair, &prompt, &validate, None);
        let mut r = run.lock().unwrap();
        if g != self.gen_of(&id) || sv(&r, "phase") != "meeting" {
            return false;
        }
        match res {
            Err(e) => self.step_failed(&mut r, "conclusion", e),
            Ok(v) => {
                let through = r["discussion"]["messages"].as_array().unwrap().len();
                r["discussion"]["conclusion"] = json!({ "message": v["message"], "through": through, "at": self.now() as i64 });
                r["pending"] = Value::Null;
                r["phase"] = json!("completed");
                self.save(&mut r);
            }
        }
        false
    }

    pub fn set_summarizer(&self, id: &str, agent: &str) -> CR<()> {
        let run = self.must_get(id)?;
        let mut r = run.lock().unwrap();
        let ok = r.get("discussion").map(|d| d.is_object()).unwrap_or(false) && ["completed", "blocked", "paused"].contains(&sv(&r, "phase")) && sh::strs(&r["participants"]).iter().any(|a| a == agent) && !sh::strs(&r["discussion"]["excluded"]).iter().any(|a| a == agent) && self.inflight(id) == 0;
        if !ok {
            return Err(cerr("not-allowed"));
        }
        r["discussion"]["summarizer"] = json!(agent);
        self.save(&mut r);
        Ok(())
    }

    pub fn summarize_discussion(&self, id: &str, conclude: bool) -> CR<()> {
        let run = self.must_get(id)?;
        {
            let mut r = run.lock().unwrap();
            if sv(&r, "mode") != "discussion" || !r["discussion"].is_object() || sv(&r, "phase") != "completed" {
                return Err(cerr("not-allowed"));
            }
            if self.inflight(id) > 0 {
                return Err(cerr("busy"));
            }
            self.next_gen(id);
            r["discussion"]["summaryNext"] = json!(if conclude { "conclusion" } else { "idle" });
            r["pending"] = json!({ "step": "summary" });
            r["phase"] = json!("meeting");
            r["block"] = Value::Null;
            self.save(&mut r);
        }
        self.spawn_advance(&run);
        Ok(())
    }

    pub fn discuss(&self, id: &str, text: &str) -> CR<()> {
        let run = self.must_get(id)?;
        {
            let mut r = run.lock().unwrap();
            if sv(&r, "mode") != "discussion" || !r["discussion"].is_object() || sv(&r, "phase") != "completed" {
                return Err(cerr("not-allowed"));
            }
            if self.inflight(id) > 0 {
                return Err(cerr("busy"));
            }
            let message = text.trim();
            if message.is_empty() {
                return Err(cerr("empty-prompt"));
            }
            if js_len(message) > 12000 {
                return Err(cerr("prompt-too-long"));
            }
            let chair = sv(&r, "chair").to_string();
            let excluded = sh::strs(&r["discussion"]["excluded"]);
            let order: Vec<String> = std::iter::once(chair.clone()).chain(sh::strs(&r["participants"]).into_iter().filter(|a| *a != chair)).filter(|a| !excluded.contains(a)).collect();
            if order.is_empty() {
                r["discussion"]["order"] = json!(order);
                return Err(cerr("no-reviewer-left"));
            }
            r["discussion"]["order"] = json!(order);
            let n = r["discussion"]["messages"].as_array().unwrap().len();
            let at = self.now() as i64;
            r["discussion"]["messages"].as_array_mut().unwrap().push(json!({ "id": format!("m{}", n + 1), "agent": null, "message": message, "replyTo": null, "at": at }));
            r["discussion"]["cursor"] = json!(0);
            self.next_gen(id);
            r["phase"] = json!("meeting");
            r["block"] = Value::Null;
            r["pending"] = json!({ "step": "discussion" });
            self.save(&mut r);
        }
        self.spawn_advance(&run);
        Ok(())
    }

    fn step_r1(&self, run: &Run) -> bool {
        let (prompt, id, assignable, max) = {
            let r = run.lock().unwrap();
            (sh::build_r1_prompt(&r, &sh::strs(&r["notes"])), sv(&r, "id").to_string(), sh::assignable_agents(&r), r["limits"]["maxTasks"].as_u64().unwrap_or(20) as usize)
        };
        let g = self.gen_of(&id);
        let chair = run.lock().unwrap()["chair"].as_str().unwrap_or("").to_string();
        let validate = |v: &Value, r: &Value| with_scope_check(r, sh::check_r1(v, &assignable, max), |x| x["tasks"].clone());
        let res = self.run_step(run, "r1", &chair, &prompt, &validate, None);
        let mut r = run.lock().unwrap();
        if g != self.gen_of(&id) || sv(&r, "phase") != "meeting" {
            return false;
        }
        match res {
            Err(e) => {
                self.step_failed(&mut r, "r1", e);
                false
            }
            Ok(v) => {
                r["r1"] = v;
                r["pending"] = Value::Null;
                for a in sh::reviewers_of(&r) {
                    r["reviewers"][a] = json!({ "status": "pending" });
                }
                let now = self.now() as i64;
                log_push(&mut r, json!({ "t": "r1", "at": now }));
                let pr = r["planRevision"].clone();
                log_push(&mut r, json!({ "t": "round", "round": 2, "at": now, "planRevision": pr }));
                self.save(&mut r);
                true
            }
        }
    }

    fn step_r2(&self, run: &Run) -> bool {
        let (todo, prompts, id) = {
            let mut r = run.lock().unwrap();
            let todo: Vec<String> = sh::reviewers_of(&r).into_iter().filter(|a| matches!(r["reviewers"][a]["status"].as_str(), None | Some("pending") | Some("running"))).collect();
            for a in &todo {
                r["reviewers"][a] = json!({ "status": "running" });
            }
            self.save(&mut r);
            let r1 = r["r1"].clone();
            let notes = sh::strs(&r["notes"]);
            let prompts: Vec<String> = todo.iter().map(|a| sh::build_r2_prompt(&r, a, &r1, &notes)).collect();
            (todo, prompts, sv(&r, "id").to_string())
        };
        let g = self.gen_of(&id);
        let validate = |v: &Value, _: &Value| sh::check_r2(v);
        let results: Vec<(String, Result<Value, StepErr>)> = std::thread::scope(|s| {
            let handles: Vec<_> = todo
                .iter()
                .zip(prompts.iter())
                .map(|(agent, prompt)| {
                    let validate = &validate;
                    s.spawn(move || (agent.clone(), self.run_step(run, "r2", agent, prompt, validate, None)))
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap_or_else(|_| (String::new(), Err(StepErr::Failed("reviewer crashed".into(), Vec::new()))))).collect()
        });
        let mut r = run.lock().unwrap();
        if g != self.gen_of(&id) || sv(&r, "phase") != "meeting" {
            return false;
        }
        let mut worst: Option<StepErr> = None;
        for (agent, res) in results {
            let now = self.now() as i64;
            match res {
                Ok(v) => {
                    r["reviewers"][&agent] = json!({ "status": "ok", "output": v });
                    log_push(&mut r, json!({ "t": "r2", "agent": agent, "at": now }));
                }
                Err(e) => {
                    let msg = e.message();
                    r["reviewers"][&agent] = json!({ "status": "failed", "error": msg });
                    if !matches!(e, StepErr::Cancelled) {
                        log_push(&mut r, json!({ "t": "error", "step": "r2", "agent": agent, "message": msg, "at": now }));
                    }
                    if worst.as_ref().map(|w| e.rank() < w.rank()).unwrap_or(true) {
                        worst = Some(e);
                    }
                }
            }
        }
        if let Some(w) = worst {
            if !matches!(w, StepErr::Failed(..)) {
                self.step_failed(&mut r, "r2", w);
                return false;
            }
        }
        let reviewers = sh::reviewers_of(&r);
        let failed: Vec<&String> = reviewers.iter().filter(|a| sv(&r["reviewers"][a], "status") == "failed").collect();
        if !failed.is_empty() {
            // A failed review is not agreement: the user retries or explicitly goes on with fewer
            let details: Vec<String> = failed.iter().map(|a| format!("{a}: {}", sv(&r["reviewers"][*a], "error"))).collect();
            let names: Vec<&str> = failed.iter().map(|a| a.as_str()).collect();
            self.block(&mut r, "reviewers-failed", &format!("Review failed for: {}", names.join(", ")), Some("r2"), details);
            return false;
        }
        r["pending"] = Value::Null;
        let pr = r["planRevision"].clone();
        log_push(&mut r, json!({ "t": "round", "round": 3, "at": self.now() as i64, "planRevision": pr }));
        self.save(&mut r);
        true
    }

    fn step_r34(&self, run: &Run) -> bool {
        let (prompt, id, required, assignable, max, chair) = {
            let r = run.lock().unwrap();
            let issues = sh::collect_issues(sv(&r, "chair"), &r["r1"], &r["reviewers"]);
            let prompt = sh::build_r34_prompt(&r, &r["r1"], &r["reviewers"], &issues, &sh::strs(&r["notes"]));
            let required: Vec<String> = issues.iter().map(|i| sv(i, "id").to_string()).collect();
            (prompt, sv(&r, "id").to_string(), required, sh::assignable_agents(&r), r["limits"]["maxTasks"].as_u64().unwrap_or(20) as usize, sv(&r, "chair").to_string())
        };
        let g = self.gen_of(&id);
        let validate = |v: &Value, r: &Value| with_scope_check(r, sh::check_resolution(v, &assignable, max, &required), |x| x["tasks"].clone());
        let res = self.run_step(run, "r34", &chair, &prompt, &validate, None);
        let mut r = run.lock().unwrap();
        if g != self.gen_of(&id) || sv(&r, "phase") != "meeting" {
            return false;
        }
        match res {
            Err(e) => self.step_failed(&mut r, "r34", e),
            Ok(v) => self.adopt_board(&mut r, v, "chair", false),
        }
        false
    }

    fn step_revise(&self, run: &Run) -> bool {
        let (prompt, id, required, assignable, max, chair, feedback, board) = {
            let mut r = run.lock().unwrap();
            let Some(board) = sh::current_board(&r).cloned() else {
                self.block(&mut r, "step-failed", "There is no plan to revise.", None, Vec::new());
                return false;
            };
            let feedback = r.pointer("/pending/feedback").and_then(Value::as_str).unwrap_or("").to_string();
            let required: Vec<String> = board["unresolved"].as_array().into_iter().flatten().map(|u| sv(u, "issueId").to_string()).filter(|x| !x.is_empty()).collect();
            let prompt = sh::build_revise_prompt(&r, &board, &feedback, &sh::strs(&r["notes"]));
            (prompt, sv(&r, "id").to_string(), required, sh::assignable_agents(&r), r["limits"]["maxTasks"].as_u64().unwrap_or(20) as usize, sv(&r, "chair").to_string(), feedback, board)
        };
        let g = self.gen_of(&id);
        let validate = |v: &Value, r: &Value| with_scope_check(r, sh::check_resolution(v, &assignable, max, &required), |x| x["tasks"].clone());
        let res = self.run_step(run, "revise", &chair, &prompt, &validate, None);
        let mut r = run.lock().unwrap();
        if g != self.gen_of(&id) || sv(&r, "phase") != "meeting" {
            return false;
        }
        match res {
            Err(e) => {
                r["pending"] = json!({ "step": "revise", "feedback": feedback });
                self.step_failed(&mut r, "revise", e);
            }
            Ok(mut v) => {
                // Earlier rulings still count: new decisions go after them
                let mut all = board["decisions"].as_array().cloned().unwrap_or_default();
                all.extend(v["decisions"].as_array().cloned().unwrap_or_default());
                v["decisions"] = json!(all);
                self.adopt_board(&mut r, v, "chair", true);
            }
        }
        false
    }

    /// Unresolved items stop at blocked; otherwise on to approval
    fn adopt_board(&self, r: &mut Value, res: Value, source: &str, bump: bool) {
        if bump {
            add_num(&mut r["planRevision"], 1.0);
        }
        let mut board = res.as_object().unwrap().clone();
        board.insert("planRevision".into(), r["planRevision"].clone());
        board.insert("source".into(), json!(source));
        board.insert("at".into(), json!(self.now() as i64));
        r["boards"].as_array_mut().unwrap().push(Value::Object(board));
        r["pending"] = Value::Null;
        let pr = r["planRevision"].clone();
        log_push(r, json!({ "t": "board", "planRevision": pr, "at": self.now() as i64 }));
        let open = res["unresolved"].as_array().map(Vec::len).unwrap_or(0);
        if open > 0 {
            self.block(r, "unresolved", &format!("{open} issue(s) need your decision."), None, Vec::new());
            return;
        }
        r["phase"] = json!("awaiting-approval");
        r["block"] = Value::Null;
        self.save(r);
    }

    fn step_failed(&self, r: &mut Value, step: &str, e: StepErr) {
        if matches!(e, StepErr::Cancelled) {
            return;
        }
        let msg = e.message();
        // R2's errors were already recorded per reviewer
        if step != "r2" {
            let agent = match step {
                "summary" => Some(sv(&r["discussion"], "summarizer")).filter(|s| !s.is_empty()).unwrap_or(sv(r, "chair")).to_string(),
                "discussion" => {
                    let c = r["discussion"]["cursor"].as_u64().unwrap_or(0) as usize;
                    r["discussion"]["order"].get(c).and_then(Value::as_str).unwrap_or("").to_string()
                }
                _ => sv(r, "chair").to_string(),
            };
            log_push(r, json!({ "t": "error", "step": step, "agent": agent, "message": msg, "at": self.now() as i64 }));
        }
        let (kind, details) = match e {
            StepErr::Budget(_, d) => ("budget", d),
            StepErr::SideEffects(_, d) => ("side-effects", d),
            StepErr::Failed(_, d) => ("step-failed", d),
            StepErr::Cancelled => unreachable!(),
        };
        self.block(r, kind, &msg, Some(step), details);
    }

    fn block(&self, r: &mut Value, kind: &str, message: &str, step: Option<&str>, details: Vec<String>) {
        r["phase"] = json!("blocked");
        let mut b = json!({ "kind": kind, "message": message, "at": self.now() as i64 });
        if !details.is_empty() {
            b["details"] = json!(details.into_iter().take(200).collect::<Vec<_>>());
        }
        r["block"] = b;
        if let Some(s) = step {
            let fb = r.pointer("/pending/feedback").and_then(Value::as_str).filter(|f| !f.is_empty() && s == "revise").map(str::to_string);
            r["pending"] = match fb {
                Some(f) => json!({ "step": s, "feedback": f }),
                None => json!({ "step": s }),
            };
        }
        log_push(r, json!({ "t": "blocked", "kind": kind, "message": message, "at": self.now() as i64 }));
        self.save(r);
    }

    /// Call, parse, validate; an invalid answer gets one repair call
    pub fn run_step(&self, run: &Run, step: &str, agent: &str, prompt: &str, validate: Validate, target: Option<(&str, &str)>) -> Result<Value, StepErr> {
        let check = |value: &Value| -> sh::Check {
            let r = run.lock().unwrap();
            if sv(&r, "mode") == "project" && step != "r1" {
                let bad = value.get("message").and_then(Value::as_str).map(|m| m.trim().is_empty() || js_len(m) > 12000).unwrap_or(true);
                if bad {
                    return Err(vec!["message must contain a brief public response, at most 12000 characters".into()]);
                }
            }
            validate(value, &r)
        };
        let (value, raw) = self.call_agent(run, agent, step, prompt, false, target)?;
        let errors = match check(&value) {
            Ok(v) => return Ok(v),
            Err(e) => e,
        };
        // A repair call counts against the budget too. Without budget, report the validation errors.
        let repaired = self.call_agent(run, agent, step, &sh::build_repair_prompt(prompt, &raw, &errors), true, target);
        let (value2, _) = match repaired {
            Ok(x) => x,
            Err(StepErr::Budget(..)) => {
                let head: Vec<&str> = errors.iter().take(3).map(String::as_str).collect();
                return Err(StepErr::Budget(format!("Invalid output and no budget left to repair it: {}", head.join("; ")), errors.clone()));
            }
            Err(e) => return Err(e),
        };
        match check(&value2) {
            Ok(v) => Ok(v),
            Err(e2) => {
                let head: Vec<&str> = e2.iter().take(3).map(String::as_str).collect();
                Err(StepErr::Failed(format!("Output still invalid after one repair: {}", head.join("; ")), e2.clone()))
            }
        }
    }

    fn call_agent(&self, run: &Run, agent: &str, step: &str, prompt: &str, repair: bool, target: Option<(&str, &str)>) -> Result<(Value, String), StepErr> {
        let id = {
            let r = run.lock().unwrap();
            // Counted before the call, failed calls included (cowork.md §8). Review after implementation counts as execution budget.
            if target.is_none() {
                let max = r["limits"]["maxPlanningCalls"].as_i64().unwrap_or(0);
                if r["budget"]["planningCallsUsed"].as_i64().unwrap_or(0) >= max {
                    return Err(StepErr::Budget(format!("Planning call limit reached ({max})."), Vec::new()));
                }
                let mins = r["limits"]["maxPlanningMinutes"].as_f64().unwrap_or(0.0);
                if self.ms_used(&r) >= mins * 60_000.0 {
                    return Err(StepErr::Budget(format!("Planning time limit reached ({} min).", r["limits"]["maxPlanningMinutes"]), Vec::new()));
                }
            }
            sv(&r, "id").to_string()
        };
        let cli = self.deps.resolve_cli(agent).map_err(|e| StepErr::Failed(format!("{agent} is not available: {e}"), Vec::new()))?;
        if target.is_none() {
            self.ensure_snapshot(run).map_err(|e| StepErr::Failed(format!("Could not prepare the planning snapshot: {e}"), Vec::new()))?;
        }
        *self.t.lock().unwrap().inflight.entry(id.clone()).or_insert(0) += 1;
        let out = self.call_agent_inner(run, agent, step, prompt, repair, &cli, target);
        if let Some(n) = self.t.lock().unwrap().inflight.get_mut(&id) {
            *n = n.saturating_sub(1);
        }
        out
    }

    /// Two reviewers can find the snapshot missing at once: they wait on the same build
    fn ensure_snapshot(&self, run: &Run) -> Result<(), String> {
        let (id, discussion, dir, root, base) = {
            let r = run.lock().unwrap();
            (sv(&r, "id").to_string(), sv(&r, "mode") == "discussion", sv(&r, "snapshotDir").to_string(), r["repo"]["root"].as_str().unwrap_or("").to_string(), r["repo"]["baseCommit"].as_str().unwrap_or("").to_string())
        };
        if discussion {
            return fs::create_dir_all(&dir).map_err(|e| e.to_string());
        }
        let gate = self.snapshot_lock.lock().unwrap().entry(id).or_default().clone();
        let _g = gate.lock().unwrap();
        if Path::new(&dir).join(".git").exists() {
            return Ok(());
        }
        wb_git::create_snapshot(Path::new(&root), Path::new(&dir), &base)
    }

    fn call_agent_inner(&self, run: &Run, agent: &str, step: &str, prompt: &str, repair: bool, cli: &ResolvedCli, target: Option<(&str, &str)>) -> Result<(Value, String), StepErr> {
        let (id, cwd, base_commit, meeting_dir, call_id, inv, sent, timeout_ms, max_bytes) = {
            let mut r = run.lock().unwrap();
            let id = sv(&r, "id").to_string();
            let cwd = target.map(|t| t.0.to_string()).unwrap_or_else(|| sv(&r, "snapshotDir").to_string());
            let base = target.map(|t| t.1.to_string()).unwrap_or_else(|| r["repo"]["baseCommit"].as_str().unwrap_or("").to_string());
            let call_id = format!("c{}", r["calls"].as_array().unwrap().len() + 1);
            // This meeting's choice, then the CLI's default (codex: the user's config.toml)
            let choice = r["models"].get(agent).cloned().unwrap_or(Value::Null);
            let model = Some(sv(&choice, "model")).filter(|m| !m.is_empty()).unwrap_or(&cli.default_model).to_string();
            let supported = cli.model_efforts.get(&model);
            let is_haiku = model.to_lowercase().contains("haiku");
            let auto = r.get("autoEffort").map(crate::settings::truthy).unwrap_or(false) && supported.map(|s| !s.is_empty()).unwrap_or(true) && !(agent == "claude" && is_haiku) && !(agent == "antigravity" && !model.is_empty() && supported.is_none());
            let preferred = match step {
                "discussion" => "low",
                "r2" | "r34" | "conclusion" => "high",
                _ => "medium",
            };
            let chosen = sv(&choice, "effort");
            let effort = if !chosen.is_empty() {
                chosen.to_string()
            } else if auto {
                match supported {
                    Some(s) if !s.iter().any(|x| x == preferred) => s.first().cloned().unwrap_or_default(),
                    _ => preferred.to_string(),
                }
            } else if !cli.default_effort.is_empty() {
                cli.default_effort.clone()
            } else if agent == "antigravity" && model.is_empty() {
                sh::AGY_DEFAULT_EFFORT.to_string()
            } else {
                String::new()
            };
            let meeting_dir = self.dir_of(&r).join("meeting");
            fs::create_dir_all(&meeting_dir).map_err(|e| StepErr::Failed(e.to_string(), Vec::new()))?;
            let schema = sh::schema_for(step, &sh::assignable_agents(&r));
            let schema_file = crate::util::p(&meeting_dir.join(format!("{call_id}.schema.json")));
            let out_file = crate::util::p(&meeting_dir.join(format!("{call_id}.last.txt")));
            fs::write(&schema_file, schema.to_string()).map_err(|e| StepErr::Failed(e.to_string(), Vec::new()))?;
            let discussion = sv(&r, "mode") == "discussion";
            let inv = sh::planner_invocation(agent, &sh::PlannerOpts { cwd: &cwd, schema: &schema, schema_file: &schema_file, out_file: &out_file, windows_sandbox: cli.windows_sandbox.as_deref(), model: Some(&model), effort: Some(&effort), discussion });
            let sent = if discussion { prompt.to_string() } else { sh::with_tool_note(prompt, agent) };
            let timeout_ms = r["limits"]["callTimeoutSec"].as_u64().unwrap_or(180).max(inv.min_timeout_sec.unwrap_or(0)) * 1000;
            let mut call = json!({ "id": call_id, "agent": agent, "step": step, "repair": repair, "planRevision": r["planRevision"], "startedAt": self.now() as i64, "outputBytes": 0, "timeoutMs": timeout_ms, "status": "running" });
            if !model.is_empty() {
                call["model"] = json!(model);
            }
            if !effort.is_empty() {
                call["effort"] = json!(effort);
            }
            r["calls"].as_array_mut().unwrap().push(call);
            if target.is_none() {
                add_num(&mut r["budget"]["planningCallsUsed"], 1.0);
                self.clock_start(&mut r);
            }
            self.save(&mut r);
            let max_bytes = r["limits"]["maxOutputBytes"].as_u64().unwrap_or(1 << 20) as usize;
            (id, cwd, base, meeting_dir, call_id, inv, sent, timeout_ms, max_bytes)
        };
        let token = CancelToken::default();
        self.register_token(&id, &token);
        // Cancelled during the setup above: a token registered late would miss it
        if self.is_shutting_down() || self.t.lock().unwrap().abort_reason.contains_key(&id) {
            token.cancel();
        }
        let call_slot = |r: &mut Value, f: &mut dyn FnMut(&mut Value)| {
            if let Some(c) = r["calls"].as_array_mut().unwrap().iter_mut().find(|c| sv(c, "id") == call_id) {
                f(c);
            }
        };
        let spec = RunSpec {
            command: cli.command.clone(),
            args: cli.prefix_args.iter().cloned().chain(inv.args.iter().cloned()).collect(),
            cwd: cwd.clone(),
            stdin: if inv.stdin_ndjson { sh::agy_stdin(&sent) } else { sent.clone() },
            timeout_ms,
            max_bytes,
            env: cli.env.as_ref().map(|e| std::env::vars().chain(e.iter().cloned()).collect()),
        };
        let guard_agy = inv.guard_agy;
        let mut on_activity = |bytes: usize| {
            let mut r = run.lock().unwrap();
            let now = self.now() as i64;
            call_slot(&mut r, &mut |c| {
                if c.get("firstOutputAt").is_none() {
                    c["firstOutputAt"] = json!(now);
                }
                c["outputBytes"] = json!(bytes);
                c["lastOutputAt"] = json!(now);
            });
            self.emit_soon(&r);
        };
        let mut on_line = |line: &str| if guard_agy { sh::agy_tool_guard(line) } else { None };
        let mut on_spawn = || {
            let mut r = run.lock().unwrap();
            let now = self.now() as i64;
            call_slot(&mut r, &mut |c| c["spawnedAt"] = json!(now));
            self.deps.emit(&r);
        };
        let result = run_proc(&spec, &token, ProcHooks { on_activity: &mut on_activity, on_line: &mut on_line, on_spawn: &mut on_spawn }, |pid| self.deps.on_pid(pid));
        self.unregister_token(&id, &token);
        let mut r = run.lock().unwrap();
        if target.is_none() {
            self.clock_stop(&mut r);
        }
        let now = self.now() as i64;
        call_slot(&mut r, &mut |c| c["endedAt"] = json!(now));

        let timeout_s = (timeout_ms as f64 / 1000.0).round() as i64;
        let parsed = if let Some(e) = &result.spawn_error {
            sh::Parsed { value: None, error: Some(format!("Could not start {agent}: {e}")), raw: String::new(), usage: None, model: None }
        } else if let Some(g) = &result.guard_reason {
            sh::Parsed { value: None, error: Some(g.clone()), raw: result.stdout.clone(), usage: None, model: None }
        } else if result.cancelled {
            sh::Parsed { value: None, error: Some("cancelled".into()), raw: result.stdout.clone(), usage: None, model: None }
        } else if result.timed_out {
            sh::Parsed { value: None, error: Some(format!("{agent} timed out after {timeout_s}s")), raw: result.stdout.clone(), usage: None, model: None }
        } else if result.truncated {
            sh::Parsed { value: None, error: Some(format!("{agent} produced more output than the {} byte limit", r["limits"]["maxOutputBytes"])), raw: String::new(), usage: None, model: None }
        } else {
            let last = inv.out_file.as_ref().filter(|f| Path::new(f).exists()).and_then(|f| crate::util::read_text(Path::new(f)));
            let mut p = match agent {
                "codex" => sh::parse_codex_output(&result.stdout, last.as_deref()),
                "antigravity" => sh::parse_agy_output(&result.stdout),
                _ => sh::parse_claude_output(&result.stdout),
            };
            if p.error.is_some() && result.code != Some(0) && !result.stderr.trim().is_empty() {
                let tail: String = {
                    let t = result.stderr.trim();
                    let chars: Vec<char> = t.chars().collect();
                    chars[chars.len().saturating_sub(400)..].iter().collect()
                };
                let code = result.code.map(|c| c.to_string()).unwrap_or_else(|| "null".into());
                p.error = Some(format!("{} (exit {code}: {tail})", p.error.unwrap()));
            }
            p
        };
        if let Some(m) = &parsed.model {
            call_slot(&mut r, &mut |c| c["model"] = json!(m));
        }
        if let Some(u) = &parsed.usage {
            call_slot(&mut r, &mut |c| c["usage"] = u.clone());
            add_num(&mut r["budget"]["costUsd"], u["costUsd"].as_f64().unwrap_or(0.0));
            add_num(&mut r["budget"]["tokens"], u["inputTokens"].as_f64().unwrap_or(0.0) + u["outputTokens"].as_f64().unwrap_or(0.0));
        }
        let parsed_json = match (&parsed.value, &parsed.error) {
            (Some(v), _) => json!({ "ok": true, "value": v, "raw": parsed.raw, "usage": parsed.usage, "model": parsed.model }),
            (_, e) => json!({ "ok": false, "error": e, "raw": parsed.raw, "usage": parsed.usage, "model": parsed.model }),
        };
        let stderr_tail: String = {
            let c: Vec<char> = result.stderr.chars().collect();
            c[c.len().saturating_sub(16 * 1024)..].iter().collect()
        };
        let debug = json!({ "agent": agent, "step": step, "repair": repair, "prompt": sent, "stdout": js_prefix(&result.stdout, RAW_LIMIT), "stderr": stderr_tail, "parsed": parsed_json });
        let _ = fs::write(meeting_dir.join(format!("{call_id}.json")), serde_json::to_string_pretty(&debug).unwrap());
        let is_discussion = sv(&r, "mode") == "discussion";
        drop(r);

        // After every call: any change means the CLI did not stay read-only, so this plan is void and the diff is kept
        let changes: Vec<String> = if is_discussion {
            Vec::new()
        } else {
            wb_git::snapshot_changes(Path::new(&cwd), &base_commit).unwrap_or_else(|e| vec![format!("could not verify the snapshot: {e}")])
        };
        let reason = self.t.lock().unwrap().abort_reason.get(&id).copied();
        let mut r = run.lock().unwrap();
        if !changes.is_empty() {
            call_slot(&mut r, &mut |c| {
                c["status"] = json!("failed");
                c["error"] = json!("side effects detected");
            });
            let _ = fs::write(meeting_dir.join(format!("{call_id}.side-effects.txt")), changes.join("\n"));
            if target.is_none() {
                self.abort_all(&id, "side-effects");
            }
            self.save(&mut r);
            return Err(StepErr::SideEffects(format!("{agent} changed the read-only snapshot during planning; this plan is void."), changes));
        }
        if result.cancelled {
            let budget = reason == Some("budget");
            call_slot(&mut r, &mut |c| {
                c["status"] = json!("cancelled");
                c["error"] = json!(if budget { "planning time limit reached" } else { "cancelled" });
            });
            self.save(&mut r);
            if budget {
                return Err(StepErr::Budget(format!("Planning time limit reached ({} min).", r["limits"]["maxPlanningMinutes"]), Vec::new()));
            }
            return Err(StepErr::Cancelled);
        }
        match parsed.value {
            None => {
                let e = parsed.error.unwrap_or_default();
                call_slot(&mut r, &mut |c| {
                    c["status"] = json!("failed");
                    c["error"] = json!(e);
                });
                self.save(&mut r);
                Err(StepErr::Failed(e, Vec::new()))
            }
            Some(v) => {
                call_slot(&mut r, &mut |c| c["status"] = json!("ok"));
                self.save(&mut r);
                Ok((v, parsed.raw))
            }
        }
    }

    /// Wait for every running call of this run to end, so the snapshot is only removed once processes have stopped
    fn settle(&self, id: &str) {
        for _ in 0..300 {
            if self.inflight(id) == 0 {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn cancel(&self, id: &str) -> CR<()> {
        let run = self.must_get(id)?;
        {
            let mut r = run.lock().unwrap();
            if sh::TERMINAL_PHASES.contains(&sv(&r, "phase")) {
                return Ok(());
            }
            // Execution started: pause or clean up instead. A run with worktrees cannot just be marked cancelled.
            if r.get("execution").map(|e| !e.is_null()).unwrap_or(false) {
                return Err(cerr("not-allowed"));
            }
            self.abort_all(id, "cancel");
            self.next_gen(id);
            r["phase"] = json!("cancelled");
            r["block"] = Value::Null;
            r["pending"] = Value::Null;
            if let Some(m) = r["reviewers"].as_object_mut() {
                for v in m.values_mut() {
                    if sv(v, "status") == "running" {
                        *v = json!({ "status": "pending" });
                    }
                }
            }
            log_push(&mut r, json!({ "t": "cancelled", "at": self.now() as i64 }));
            self.save(&mut r);
        }
        self.settle(id);
        let r = run.lock().unwrap();
        self.drop_snapshot(&r);
        Ok(())
    }

    /// Retry the step where it stopped. A retry counts against the budget too.
    pub fn retry(&self, id: &str) -> CR<()> {
        let run = self.must_get(id)?;
        {
            let mut r = run.lock().unwrap();
            let phase = sv(&r, "phase");
            if phase != "blocked" && phase != "paused" {
                return Err(cerr("not-retryable"));
            }
            let kind = r.pointer("/block/kind").and_then(Value::as_str).unwrap_or("").to_string();
            if kind == "unresolved" {
                return Err(cerr("not-retryable"));
            }
            if self.inflight(id) > 0 {
                return Err(cerr("busy"));
            }
            // The snapshot was touched: rebuild it for a clean baseline
            if kind == "side-effects" {
                self.drop_snapshot(&r);
            }
            for a in sh::reviewers_of(&r) {
                if sv(&r["reviewers"][&a], "status") == "failed" {
                    r["reviewers"][&a] = json!({ "status": "pending" });
                }
            }
            self.next_gen(id);
            r["phase"] = json!("meeting");
            r["block"] = Value::Null;
            log_push(&mut r, json!({ "t": "resumed", "at": self.now() as i64 }));
            self.save(&mut r);
        }
        self.spawn_advance(&run);
        Ok(())
    }

    /// The user explicitly goes on without a failed reviewer (cowork.md §4.2)
    pub fn drop_failed_reviewers(&self, id: &str) -> CR<()> {
        let run = self.must_get(id)?;
        let advance = {
            let mut r = run.lock().unwrap();
            if sv(&r, "mode") == "discussion" && r["discussion"].is_object() {
                let ok = sv(&r, "phase") == "blocked" && r.pointer("/block/kind").and_then(Value::as_str) == Some("step-failed") && r.pointer("/pending/step").and_then(Value::as_str) == Some("discussion") && self.inflight(id) == 0;
                if !ok {
                    return Err(cerr("not-allowed"));
                }
                let c = r["discussion"]["cursor"].as_u64().unwrap_or(0) as usize;
                let Some(agent) = r["discussion"]["order"].get(c).and_then(Value::as_str).map(str::to_string) else { return Err(cerr("not-allowed")) };
                r["discussion"]["excluded"].as_array_mut().unwrap().push(json!(agent));
                add_num(&mut r["discussion"]["cursor"], 1.0);
                self.next_gen(id);
                let pr = r["planRevision"].clone();
                log_push(&mut r, json!({ "t": "drop", "agents": [agent], "at": self.now() as i64, "planRevision": pr }));
                let len = r["discussion"]["order"].as_array().unwrap().len();
                let cursor = r["discussion"]["cursor"].as_u64().unwrap_or(0) as usize;
                let summarize = cursor == len && r["discussion"]["summarizeEachRound"] == json!(true);
                let meeting = cursor < len || summarize;
                r["phase"] = json!(if meeting { "meeting" } else { "completed" });
                r["block"] = Value::Null;
                if summarize {
                    r["discussion"]["summaryNext"] = json!("idle");
                }
                r["pending"] = if meeting { json!({ "step": if summarize { "summary" } else { "discussion" } }) } else { Value::Null };
                self.save(&mut r);
                meeting
            } else {
                if sv(&r, "phase") != "blocked" || r.pointer("/block/kind").and_then(Value::as_str) != Some("reviewers-failed") {
                    return Err(cerr("not-allowed"));
                }
                let reviewers = sh::reviewers_of(&r);
                let failed: Vec<String> = reviewers.iter().filter(|a| sv(&r["reviewers"][*a], "status") == "failed").cloned().collect();
                if !reviewers.iter().any(|a| sv(&r["reviewers"][a], "status") == "ok") {
                    return Err(cerr("no-reviewer-left"));
                }
                for a in &failed {
                    r["reviewers"][a]["status"] = json!("dropped");
                }
                self.next_gen(id);
                add_num(&mut r["planRevision"], 1.0);
                let (pr, now) = (r["planRevision"].clone(), self.now() as i64);
                log_push(&mut r, json!({ "t": "drop", "agents": failed, "at": now, "planRevision": pr }));
                log_push(&mut r, json!({ "t": "round", "round": 3, "at": now, "planRevision": pr }));
                r["phase"] = json!("meeting");
                r["block"] = Value::Null;
                r["pending"] = json!({ "step": "r34" });
                self.save(&mut r);
                true
            }
        };
        if advance {
            self.spawn_advance(&run);
        }
        Ok(())
    }

    pub fn add_note(&self, id: &str, text: &str) -> CR<()> {
        let run = self.must_get(id)?;
        let t = text.trim();
        if t.is_empty() {
            return Ok(());
        }
        let mut r = run.lock().unwrap();
        if sh::TERMINAL_PHASES.contains(&sv(&r, "phase")) {
            return Err(cerr("not-allowed"));
        }
        let short = js_prefix(t, 4000);
        r["notes"].as_array_mut().unwrap().push(json!(short));
        log_push(&mut r, json!({ "t": "note", "text": short, "at": self.now() as i64 }));
        self.save(&mut r);
        Ok(())
    }

    fn board_editable(r: &Value) -> bool {
        let phase = sv(r, "phase");
        (phase == "awaiting-approval" || phase == "approved" || (phase == "blocked" && r.pointer("/block/kind").and_then(Value::as_str) == Some("unresolved"))) && sh::current_board(r).is_some()
    }

    /// Feedback on the plan: the chair revises once, and the new revision needs approval again
    pub fn feedback(&self, id: &str, text: &str) -> CR<()> {
        let run = self.must_get(id)?;
        let t = text.trim();
        if t.is_empty() {
            return Err(cerr("empty-feedback"));
        }
        {
            let mut r = run.lock().unwrap();
            if !Self::board_editable(&r) {
                return Err(cerr("not-allowed"));
            }
            self.next_gen(id);
            let short = js_prefix(t, 4000);
            let pr = r["planRevision"].clone();
            log_push(&mut r, json!({ "t": "feedback", "text": short, "at": self.now() as i64, "planRevision": pr }));
            r["phase"] = json!("meeting");
            r["block"] = Value::Null;
            r["pending"] = json!({ "step": "revise", "feedback": short });
            self.save(&mut r);
        }
        self.spawn_advance(&run);
        Ok(())
    }

    /// A direct edit passes the same scope, assignee and DAG checks, and is tied to a revision so it cannot overwrite a newer plan
    pub fn edit_board(&self, id: &str, base_revision: f64, tasks: &Value) -> CR<Value> {
        let run = self.must_get(id)?;
        let mut r = run.lock().unwrap();
        if !Self::board_editable(&r) {
            return Err(cerr("not-allowed"));
        }
        if Some(base_revision) != r["planRevision"].as_f64() {
            return Err(cerr_d("stale-revision", None, json!({ "current": r["planRevision"] })));
        }
        let assignable = sh::assignable_agents(&r);
        let max = r["limits"]["maxTasks"].as_u64().unwrap_or(20) as usize;
        let checked = with_scope_check(&r, sh::check_tasks(Some(tasks), &sh::TaskCtx { assignable: &assignable, max_tasks: max, require_reason: false }), |x| x.clone());
        let value = match checked {
            Err(errors) => return Ok(json!({ "ok": false, "errors": errors })),
            Ok(v) => v,
        };
        let board = sh::current_board(&r).unwrap().clone();
        self.next_gen(id);
        add_num(&mut r["planRevision"], 1.0);
        let pr = r["planRevision"].clone();
        log_push(&mut r, json!({ "t": "edit", "planRevision": pr, "at": self.now() as i64 }));
        let mut next = json!({ "tasks": value });
        if let Some(c) = board.get("checks").filter(|c| !c.is_null()) {
            next["checks"] = c.clone();
        }
        next["decisions"] = board["decisions"].clone();
        next["unresolved"] = board["unresolved"].clone();
        self.adopt_board(&mut r, next, "user", false);
        Ok(json!({ "ok": true }))
    }

    /// The user's explicit decision: stop chasing the open items and take this plan to approval
    pub fn dismiss_unresolved(&self, id: &str) -> CR<()> {
        let run = self.must_get(id)?;
        let mut r = run.lock().unwrap();
        let Some(board) = sh::current_board(&r).cloned() else { return Err(cerr("not-allowed")) };
        if sv(&r, "phase") != "blocked" || r.pointer("/block/kind").and_then(Value::as_str) != Some("unresolved") {
            return Err(cerr("not-allowed"));
        }
        self.next_gen(id);
        let count = board["unresolved"].as_array().map(Vec::len).unwrap_or(0);
        let pr = r["planRevision"].as_i64().unwrap_or(0) + 1;
        log_push(&mut r, json!({ "t": "dismiss", "count": count, "at": self.now() as i64, "planRevision": pr }));
        add_num(&mut r["planRevision"], 1.0);
        self.adopt_board(&mut r, json!({ "tasks": board["tasks"], "decisions": board["decisions"], "unresolved": [] }), "user", false);
        Ok(())
    }

    pub fn approve(&self, id: &str, plan_revision: f64) -> CR<()> {
        let run = self.must_get(id)?;
        let mut r = run.lock().unwrap();
        let Some(board) = sh::current_board(&r).cloned() else { return Err(cerr("not-allowed")) };
        if sv(&r, "phase") != "awaiting-approval" {
            return Err(cerr("not-allowed"));
        }
        if Some(plan_revision) != r["planRevision"].as_f64() || board["planRevision"] != r["planRevision"] {
            return Err(cerr_d("stale-revision", None, json!({ "current": r["planRevision"] })));
        }
        if board["unresolved"].as_array().map(|u| !u.is_empty()).unwrap_or(false) {
            return Err(cerr("has-unresolved"));
        }
        r["approvedPlanRevision"] = r["planRevision"].clone();
        r["phase"] = json!("approved");
        r["block"] = Value::Null;
        let pr = r["planRevision"].clone();
        log_push(&mut r, json!({ "t": "approved", "planRevision": pr, "at": self.now() as i64 }));
        self.save(&mut r);
        // No more read-only snapshot needed; a later revision rebuilds it
        self.drop_snapshot(&r);
        Ok(())
    }

    /// Only by the user's explicit action; settings defaults cannot quietly relax a run
    pub fn raise_limits(&self, id: &str, calls: f64, minutes: f64) -> CR<()> {
        let run = self.must_get(id)?;
        let mut r = run.lock().unwrap();
        if sh::TERMINAL_PHASES.contains(&sv(&r, "phase")) {
            return Err(cerr("not-allowed"));
        }
        let c = (r["limits"]["maxPlanningCalls"].as_f64().unwrap_or(0.0).max((calls + 0.5).floor())).min(60.0) as i64;
        let m = (r["limits"]["maxPlanningMinutes"].as_f64().unwrap_or(0.0).max((minutes + 0.5).floor())).min(240.0) as i64;
        r["limits"]["maxPlanningCalls"] = json!(c);
        r["limits"]["maxPlanningMinutes"] = json!(m);
        log_push(&mut r, json!({ "t": "limits", "maxPlanningCalls": c, "maxPlanningMinutes": m, "at": self.now() as i64 }));
        self.save(&mut r);
        Ok(())
    }

    pub fn log_dispatch(&self, id: &str, task_id: &str, target: &str) -> CR<()> {
        let run = self.must_get(id)?;
        let mut r = run.lock().unwrap();
        if sv(&r, "phase") != "approved" {
            return Err(cerr("not-allowed"));
        }
        if !sh::current_board(&r).and_then(|b| b["tasks"].as_array()).map(|t| t.iter().any(|x| sv(x, "id") == task_id)).unwrap_or(false) {
            return Err(cerr("task-not-found"));
        }
        log_push(&mut r, json!({ "t": "dispatch", "taskId": task_id, "target": js_prefix(target, 200), "at": self.now() as i64 }));
        self.save(&mut r);
        Ok(())
    }

    /// Post-implementation review: a model that did not implement, on a read-only snapshot of the integration result. A different CLI
    /// is not proof of a different model; unknown identity is labelled unconfirmed. A failure just leaves it marked not reviewed.
    pub fn review_execution(&self, run: &Run) {
        let (implementers, participants, base, commit, patch, wt_path, root, common, id, base_commit) = {
            let r = run.lock().unwrap();
            let ex = &r["execution"];
            let mut implementers: Vec<String> = Vec::new();
            for t in ex["tasks"].as_object().unwrap().values() {
                let a = sv(t, "agent").to_string();
                if !implementers.contains(&a) {
                    implementers.push(a);
                }
            }
            let it = &ex["integration"];
            let commit = sv(it, "commit").to_string();
            let patch = sv(it, "patchId").to_string();
            let base = json!({ "implementers": implementers, "findings": [], "commit": commit, "patchId": patch, "truncated": false, "at": self.now() as i64 });
            let mut people = vec![sv(&r, "chair").to_string()];
            for a in sh::strs(&r["participants"]) {
                if !people.contains(&a) {
                    people.push(a);
                }
            }
            let wt = sv(&ex["worktrees"][sv(it, "worktree")], "path").to_string();
            (implementers, people, base, commit, patch, wt, r["repo"]["root"].as_str().unwrap_or("").to_string(), r["repo"]["commonDir"].as_str().unwrap_or("").to_string(), sv(&r, "id").to_string(), r["repo"]["baseCommit"].as_str().unwrap_or("").to_string())
        };
        let _ = patch;
        let usable: Vec<String> = participants.into_iter().filter(|a| self.deps.resolve_cli(a).is_ok()).collect();
        let model_of = |a: &str| -> String {
            let r = run.lock().unwrap();
            sv(&r["models"][a], "model").to_string()
        };
        let others: Vec<String> = usable.iter().filter(|a| !implementers.contains(a)).cloned().collect();
        let distinct: Vec<&String> = others.iter().filter(|a| model_of(a).is_empty() || !implementers.iter().any(|i| model_of(i) == model_of(a))).collect();
        let agent = distinct.first().map(|a| (*a).clone()).or_else(|| others.first().cloned()).or_else(|| implementers.iter().find(|a| usable.contains(a)).cloned());
        let with = |extra: Value| -> Value {
            let mut o = base.clone();
            for (k, v) in extra.as_object().unwrap() {
                o[k] = v.clone();
            }
            o
        };
        let Some(agent) = agent else {
            let mut r = run.lock().unwrap();
            r["execution"]["review"] = with(json!({ "status": "failed", "error": "no reviewer is available" }));
            log_push(&mut r, json!({ "t": "exec-review", "status": "failed", "at": self.now() as i64 }));
            return;
        };
        let independence = if !others.contains(&agent) {
            "same-source"
        } else if !model_of(&agent).is_empty() && implementers.iter().all(|i| !model_of(i).is_empty() && model_of(i) != model_of(&agent)) {
            "cross-model"
        } else {
            "unconfirmed"
        };
        let dir = format!("{}-review", snapshot_dir_for(&common, &id));
        let under = crate::util::p(&Path::new(&common).join("cowork"));
        {
            let mut r = run.lock().unwrap();
            r["execution"]["review"] = with(json!({ "status": "running", "agent": agent, "independence": independence }));
            log_push(&mut r, json!({ "t": "exec-review", "status": "running", "agent": agent, "at": self.now() as i64 }));
            self.save(&mut r);
        }
        let attempt = || -> Result<Value, String> {
            wb_git::create_snapshot(Path::new(&root), Path::new(&dir), &commit)?;
            let d = wb_git::diff_text(Path::new(&wt_path), &base_commit, &commit, EXEC_REVIEW_MAX_DIFF)?;
            let prompt = {
                let r = run.lock().unwrap();
                let waivers: Vec<String> = r["execution"]["tasks"].as_object().unwrap().iter().filter(|(_, t)| !t["waiver"].is_null() && t.get("waiver").is_some()).map(|(k, t)| format!("{k}: {}", sv(&t["waiver"], "reason"))).collect();
                let board = sh::current_board(&r).unwrap().clone();
                let checks = r["execution"]["integration"]["checkRuns"].as_array().cloned().unwrap_or_default();
                sh::build_exec_review_prompt(&r, &board, &d.text, d.truncated, &checks, &waivers)
            };
            let validate = |v: &Value, _: &Value| sh::check_exec_review(v);
            let res = self.run_step(run, "exec-review", &agent, &prompt, &validate, Some((&dir, &commit)));
            let r = run.lock().unwrap();
            let model = r["calls"].as_array().unwrap().iter().rev().find(|c| sv(c, "step") == "exec-review").and_then(|c| c.get("model").cloned());
            let now = self.now() as i64;
            Ok(match res {
                Ok(v) => {
                    let mut o = with(json!({ "status": "ok", "agent": agent, "independence": independence, "truncated": d.truncated, "verdict": v["verdict"], "message": v["message"], "findings": v["findings"], "at": now }));
                    if let Some(m) = model {
                        o["model"] = m;
                    }
                    o
                }
                Err(e) => with(json!({ "status": "failed", "agent": agent, "independence": independence, "truncated": d.truncated, "error": e.message(), "at": now })),
            })
        };
        let review = attempt().unwrap_or_else(|e| with(json!({ "status": "failed", "agent": agent, "independence": independence, "error": js_prefix(&e, 500), "at": self.now() as i64 })));
        let _ = remove_snapshot(&dir, &under);
        let mut r = run.lock().unwrap();
        let (status, verdict) = (review["status"].clone(), review.get("verdict").cloned());
        r["execution"]["review"] = review;
        let mut entry = json!({ "t": "exec-review", "status": status, "agent": agent, "at": self.now() as i64 });
        if let Some(v) = verdict {
            entry["verdict"] = v;
        }
        log_push(&mut r, entry);
        self.save(&mut r);
    }

    pub fn exec_start(&self, id: &str, o: &exec::StartOpts) -> CR<()> {
        let run = self.must_get(id)?;
        let repo = {
            let r = run.lock().unwrap();
            repo_id_of(r["repo"]["commonDir"].as_str().unwrap_or(""))
        };
        {
            let mut t = self.t.lock().unwrap();
            if let Some(other) = t.execution_starting.get(&repo) {
                return Err(cerr_d("active-run-exists", None, json!({ "runId": other })));
            }
            t.execution_starting.insert(repo.clone(), id.to_string());
        }
        let res = exec::start(self, &run, o);
        self.t.lock().unwrap().execution_starting.remove(&repo);
        res
    }

    /// Same-repo execution stays exclusive until its worktrees are merged or cleaned; planning may run alongside
    pub fn other_active_run(&self, id: &str, common_dir: &str) -> Option<String> {
        let repo = repo_id_of(common_dir);
        for run in self.all_runs() {
            let r = run.lock().unwrap();
            let ex = r.get("execution");
            let open = ex.map(|e| e.is_object() && e.get("cleaned").map(crate::settings::truthy) != Some(true) && !e.get("merged").map(crate::settings::truthy).unwrap_or(false)).unwrap_or(false);
            if sv(&r, "id") != id && open && repo_id_of(r["repo"]["commonDir"].as_str().unwrap_or("")) == repo {
                return Some(sv(&r, "id").to_string());
            }
        }
        None
    }

    pub fn must(&self, id: &str) -> CR<Run> {
        self.must_get(id)
    }

    pub fn delete(&self, id: &str) -> CR<()> {
        let run = self.must_get(id)?;
        let r = run.lock().unwrap();
        if sv(&r, "phase") == "meeting" || self.inflight(id) > 0 {
            return Err(cerr("busy"));
        }
        // Worktrees still there: finish first, or .cowork/ keeps some nobody owns
        if let Some(ex) = r.get("execution").filter(|e| e.is_object()) {
            if ex.get("cleaned").map(crate::settings::truthy) != Some(true) && !ex.get("merged").map(crate::settings::truthy).unwrap_or(false) {
                return Err(cerr("has-worktrees"));
            }
        }
        if exec::is_running(self, id) {
            return Err(cerr("busy"));
        }
        self.drop_snapshot(&r);
        let dir = self.dir_of(&r);
        let base = crate::util::resolve(&crate::util::p(&self.deps.data_dir()));
        let target = crate::util::resolve(&crate::util::p(&dir));
        if target.to_lowercase().starts_with(&format!("{}{}", base.to_lowercase(), std::path::MAIN_SEPARATOR)) {
            let _ = fs::remove_dir_all(&dir);
        }
        drop(r);
        self.runs.lock().unwrap().remove(id);
        self.dirs.lock().unwrap().remove(id);
        Ok(())
    }

    fn drop_snapshot(&self, r: &Value) {
        if sv(r, "mode") == "discussion" {
            return; // The conversation folder goes with the run
        }
        let under = crate::util::p(&Path::new(r["repo"]["commonDir"].as_str().unwrap_or("")).join("cowork"));
        if let Err(e) = remove_snapshot(sv(r, "snapshotDir"), &under) {
            eprintln!("[cowork] could not remove snapshot {}: {e}", sv(r, "snapshotDir"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_keeps_the_previous_copy_and_falls_back() {
        let dir = std::env::temp_dir().join(format!("wb-store-{}", uuid::Uuid::new_v4()));
        let file = dir.join("run.json");
        let valid = |v: &Value| v["v"].is_number();
        write_json_atomic(&file, &json!({ "v": 1 })).unwrap();
        write_json_atomic(&file, &json!({ "v": 2 })).unwrap();
        assert_eq!(read_json_with_fallback(&file, &valid).unwrap(), (json!({ "v": 2 }), false));
        fs::write(&file, "{ torn").unwrap();
        assert_eq!(read_json_with_fallback(&file, &valid).unwrap(), (json!({ "v": 1 }), true));
        fs::write(&file, "{\"other\":1}").unwrap();
        assert_eq!(read_json_with_fallback(&file, &valid).unwrap().1, true, "wrong shape counts as damaged");
        fs::remove_file(prev_path(&file)).unwrap();
        assert!(read_json_with_fallback(&file, &valid).is_none());
        let leftovers: Vec<_> = fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).collect();
        assert!(leftovers.is_empty(), "no temp files stay behind");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn repo_id_matches_what_is_on_disk() {
        // Sixteen hex characters of a sha-256, so existing runs stay under the folders they were saved in
        assert_eq!(repo_id_of("C:\\no\\such\\path").len(), 16);
    }
}

fn next_step(run: &Value) -> Option<&'static str> {
    if sv(run, "mode") == "discussion" {
        let d = &run["discussion"];
        return (d.is_object() && d["cursor"].as_u64().unwrap_or(0) < d["order"].as_array().map(|o| o.len() as u64).unwrap_or(0)).then_some("discussion");
    }
    if run["r1"].is_null() {
        return Some("r1");
    }
    if sh::reviewers_of(run).iter().any(|a| matches!(run["reviewers"][a]["status"].as_str().unwrap_or("pending"), "pending" | "running" | "failed")) {
        return Some("r2");
    }
    if run["boards"].as_array().map(Vec::is_empty).unwrap_or(true) {
        return Some("r34");
    }
    None
}

/// A scope path that crosses out of the snapshot through a symlink is refused
fn with_scope_check(r: &Value, checked: sh::Check, tasks_of: impl Fn(&Value) -> Value) -> sh::Check {
    let v = checked?;
    let tree = if Path::new(sv(r, "snapshotDir")).exists() { sv(r, "snapshotDir").to_string() } else { r["repo"]["root"].as_str().unwrap_or("").to_string() };
    let mut errors = Vec::new();
    for t in tasks_of(&v).as_array().into_iter().flatten() {
        for x in sh::strs(&t["scope"]) {
            if let Some(why) = scope_escapes(&tree, &x) {
                errors.push(format!("task {}: {why}", sv(t, "id")));
            }
        }
    }
    if errors.is_empty() {
        Ok(v)
    } else {
        Err(errors)
    }
}

/// A discussion only needs a workspace identity, not HEAD, dirty files or a worktree. Plain folders can host one.
fn discussion_identity(workspace: &str) -> Result<(String, String), String> {
    if workspace.is_empty() || !fs::metadata(workspace).map(|m| m.is_dir()).unwrap_or(false) {
        return Err("not-a-git-repo".into());
    }
    let root = crate::util::resolve(workspace);
    let found = (|| -> Result<(String, String), String> {
        let top = wb_git::git(Path::new(&root), &["rev-parse", "--show-toplevel"])?.trim().to_string();
        let common = wb_git::git(Path::new(&top), &["rev-parse", "--git-common-dir"])?.trim().to_string();
        let abs = if Path::new(&common).is_absolute() { common } else { crate::util::p(&Path::new(&top).join(common)) };
        Ok((top.replace('/', "\\"), crate::util::resolve(&abs)))
    })();
    Ok(found.unwrap_or_else(|_| (root.clone(), root)))
}

/// Instructions at the base commit plus the skills read for this meeting, within size limits
fn build_context(root: &str, head: &str, o: &StartOptions) -> Option<Value> {
    let mut total = 0usize;
    let mut take = |text: &str| -> Option<String> {
        let t = js_prefix(text, MAX_CONTEXT_FILE);
        let n = js_len(&t);
        if total + n > MAX_CONTEXT_TOTAL {
            return None;
        }
        total += n;
        Some(t)
    };
    let mut instructions = Vec::new();
    if o.project_instructions {
        for file in INSTRUCTION_FILES {
            let text = wb_git::git(Path::new(root), &["show", &format!("{head}:{file}")]).unwrap_or_default();
            if !text.trim().is_empty() {
                if let Some(t) = take(&text) {
                    instructions.push(json!({ "file": file, "content": t }));
                }
            }
        }
    }
    let mut skills = Vec::new();
    for k in &o.skills {
        let (Some(content), Some(name)) = (k.get("content").and_then(Value::as_str), k.get("name").and_then(Value::as_str)) else { continue };
        if let Some(t) = take(content) {
            skills.push(json!({ "key": sv(k, "key"), "name": js_prefix(name, 200), "content": t }));
        }
    }
    (!instructions.is_empty() || !skills.is_empty()).then(|| json!({ "instructions": instructions, "skills": skills }))
}
