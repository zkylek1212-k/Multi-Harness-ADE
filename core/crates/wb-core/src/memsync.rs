// Memory sync: each project's .project-memory lives in the user's own private GitHub repo or synced cloud folder, and the
// project links to it (a junction on Windows, no admin needed). The project repo stops tracking memory.
// Same flows as before; git and gh are behind Tools so scripts/check-memsync.mts runs the whole thing with real git.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use regex::{Regex, RegexBuilder};
use serde_json::{json, Map, Value};
use wb_proc::runner::run_process;
use wb_proc::{CancelToken, RunSpec};

use crate::core::{arg_str, Core};
use crate::paths::find_cli;
use crate::util::{dirname, exists, home, iso, now_ms, p, read_text, resolve, sv};

pub const DEFAULT_REPO_NAME: &str = "ai-project-memory";
pub const MEMORY_DIR: &str = ".project-memory";
/// Entry files generated from the handoff, which Codex and Antigravity read at start. They hold memory, so never in a project repo.
const ENTRY_FILES: [(&str, &str); 2] = [("AGENTS.md", "AGENTS.md"), (".agents/rules/shared-memory.md", "shared-memory.md")];
const BACKUP_DIR: &str = ".local-backups";
pub const FOLDER_MARKER: &str = ".workbench-memory.json";
pub const PLUGIN: &str = "shared-project-memory";
pub const MARKETPLACE: &str = "agent-workbench";
const BUNDLED_MARKER: &str = ".agent-workbench";
pub const PLUGIN_AGENTS: [&str; 3] = ["claude", "codex", "antigravity"];
const SYNC_INTERVAL_SECS: u64 = 10 * 60;

fn bundled_key() -> String {
    format!("{PLUGIN}@{MARKETPLACE}")
}

#[derive(Debug, Clone)]
pub struct MemErr {
    pub code: String,
    pub message: String,
}

pub fn err<T>(code: &str, message: impl Into<String>) -> Result<T, MemErr> {
    Err(MemErr { code: code.to_string(), message: message.into() })
}

type R<T> = Result<T, MemErr>;

impl From<std::io::Error> for MemErr {
    fn from(e: std::io::Error) -> Self {
        MemErr { code: "internal".into(), message: e.to_string() }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RunResult {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Default, Clone)]
pub struct RunOpts {
    pub cwd: Option<String>,
    pub timeout_ms: Option<u64>,
    pub env: Vec<(String, String)>,
    pub input: Option<String>,
}

fn at(cwd: &str) -> RunOpts {
    RunOpts { cwd: Some(cwd.to_string()), ..Default::default() }
}

/// Everything outside this process. Injected, so the tests use real git with a fake GitHub.
pub trait Runner: Send + Sync {
    fn run(&self, cmd: &str, args: &[String], opts: &RunOpts) -> RunResult;
    /// Status of an unauthenticated GET; decides whether a repo is public
    fn http_status(&self, url: &str) -> Result<u16, String>;
    fn github_api(&self, method: &str, path: &str, token: &str, body: Option<&Value>) -> Result<(u16, Value), String>;
}

pub struct Tools {
    pub git: String,
    pub gh: Option<String>,
    pub runner: Arc<dyn Runner>,
}

/// git and gh never prompt: in the background nobody can answer, and failing beats hanging
pub struct RealRunner;

impl Runner for RealRunner {
    fn run(&self, cmd: &str, args: &[String], opts: &RunOpts) -> RunResult {
        let mut env: Vec<(String, String)> = std::env::vars().collect();
        env.extend([("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()), ("GH_PROMPT_DISABLED".into(), "1".into()), ("GH_NO_UPDATE_NOTIFIER".into(), "1".into())]);
        env.extend(opts.env.iter().cloned());
        let spec = RunSpec {
            command: cmd.to_string(),
            args: args.to_vec(),
            cwd: opts.cwd.clone().unwrap_or_else(|| p(&home())),
            stdin: opts.input.clone().unwrap_or_default(),
            timeout_ms: opts.timeout_ms.unwrap_or(120_000),
            max_bytes: 8 * 1024 * 1024,
            env: Some(env),
        };
        let r = run_process(&spec, &CancelToken::default());
        let stderr = if r.stderr.is_empty() { r.spawn_error.clone().unwrap_or_default() } else { r.stderr };
        RunResult { code: r.code.unwrap_or(1), stdout: r.stdout, stderr }
    }

    fn http_status(&self, url: &str) -> Result<u16, String> {
        let res = crate::http::get(url, &[("Accept", "application/vnd.github+json")]);
        res.map(|r| r.status)
    }

    fn github_api(&self, method: &str, path: &str, token: &str, body: Option<&Value>) -> Result<(u16, Value), String> {
        let url = format!("https://api.github.com{path}");
        let auth = format!("Bearer {token}");
        let headers = [("Accept", "application/vnd.github+json"), ("Authorization", auth.as_str())];
        let res = if method == "POST" { crate::http::post_json(&url, &headers, body.unwrap_or(&Value::Null)) } else { crate::http::get(&url, &headers) }?;
        Ok((res.status, serde_json::from_slice(&res.body).unwrap_or(Value::Null)))
    }
}

fn git_at(t: &Tools, cwd: &str, args: &[&str]) -> RunResult {
    let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    t.runner.run(&t.git, &a, &at(cwd))
}

fn must(t: &Tools, cmd: &str, args: &[&str], cwd: Option<&str>, code: &str) -> R<String> {
    let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let opts = RunOpts { cwd: cwd.map(str::to_string), ..Default::default() };
    let r = t.runner.run(cmd, &a, &opts);
    if r.code != 0 {
        let body = if r.stderr.is_empty() { &r.stdout } else { &r.stderr };
        return err(code, format!("{} {}: {}", crate::util::basename(cmd), args.join(" "), crate::util::js_prefix(body.trim(), 400)));
    }
    Ok(r.stdout)
}

fn git_must(t: &Tools, cwd: &str, args: &[&str]) -> R<String> {
    must(t, &t.git, args, Some(cwd), "git-failed")
}

fn need_gh(t: &Tools) -> R<&str> {
    match &t.gh {
        Some(g) => Ok(g),
        None => err("gh-missing", "GitHub CLI (gh) is not installed"),
    }
}

// ── Cloud folder mode: a folder the drive's own client syncs. Workbench does no network work here. ──

/// Cloud drive roots found on this machine
pub fn detect_cloud_roots() -> Value {
    let h = home();
    let var = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
    let uniq = |list: Vec<String>| -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for x in list {
            if !x.is_empty() && exists(&x) {
                let r = resolve(&x);
                if !out.contains(&r) {
                    out.push(r);
                }
            }
        }
        out
    };
    let onedrive: Vec<String> = ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"].iter().filter_map(|k| var(k)).collect();
    let mut gdrive = vec![p(&h.join("Google Drive")), p(&h.join("My Drive"))];
    if cfg!(windows) {
        for c in b'D'..=b'Z' {
            for n in ["My Drive", "我的雲端硬碟"] {
                gdrive.push(format!("{}:\\{n}", c as char));
            }
        }
    }
    json!({ "onedrive": uniq(onedrive), "gdrive": uniq(gdrive) })
}

/// Whether the client is installed, which is not the same as signed in. exe is what opens it to sign in.
pub fn detect_cloud_apps() -> Value {
    let var = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
    let (pf, pf86) = (var("ProgramFiles", "C:\\Program Files"), var("ProgramFiles(x86)", "C:\\Program Files (x86)"));
    let local = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| p(&home().join("AppData").join("Local")));
    let onedrive = [format!("{local}\\Microsoft\\OneDrive\\OneDrive.exe"), format!("{pf}\\Microsoft OneDrive\\OneDrive.exe"), format!("{pf86}\\Microsoft OneDrive\\OneDrive.exe")].into_iter().find(|x| exists(x));
    let dfs = format!("{pf}\\Google\\Drive File Stream");
    let mut gdrive = Some(format!("{dfs}\\launch.bat")).filter(|x| exists(x));
    if gdrive.is_none() && exists(&dfs) {
        let mut versions: Vec<String> = fs::read_dir(&dfs).map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        versions.sort();
        versions.reverse();
        gdrive = versions.into_iter().map(|v| format!("{dfs}\\{v}\\GoogleDriveFS.exe")).find(|x| exists(x));
    }
    let row = |e: Option<String>| match e {
        Some(x) => json!({ "installed": true, "exe": x }),
        None => json!({ "installed": false }),
    };
    json!({ "onedrive": row(onedrive), "gdrive": row(gdrive) })
}

fn is_inside(child: &str, parent: &str) -> bool {
    let (c, pr) = (resolve(child).to_lowercase(), resolve(parent).to_lowercase());
    c != pr && (c.starts_with(&format!("{pr}\\")) || c.starts_with(&format!("{pr}/")))
}

pub fn is_folder_root(root: &str) -> bool {
    Path::new(root).join(FOLDER_MARKER).exists()
}

/// Has to be under a detected drive root, not the root itself: all memory and backups stay inside it
pub fn ensure_folder(root: &str, provider: &str, cloud_roots: &[String]) -> R<Value> {
    if !Path::new(root).is_absolute() {
        return err("bad-folder", root);
    }
    if cloud_roots.is_empty() {
        return err(if provider == "onedrive" { "onedrive-missing" } else { "gdrive-missing" }, if provider == "onedrive" { "onedrive-missing" } else { "gdrive-missing" });
    }
    if cloud_roots.iter().any(|r| resolve(r).to_lowercase() == resolve(root).to_lowercase()) {
        return err("pick-subfolder", root);
    }
    if !cloud_roots.iter().any(|r| is_inside(root, r)) {
        return err("folder-outside-cloud", root);
    }
    let dir = Path::new(root);
    if dir.join(".git").exists() {
        return err("folder-is-git-repo", root);
    }
    fs::create_dir_all(dir)?;
    let marker = dir.join(FOLDER_MARKER);
    if !marker.exists() {
        fs::write(marker, serde_json::to_string_pretty(&json!({ "backend": "folder", "provider": provider, "createdAt": iso(now_ms()) })).unwrap() + "\n")?;
    }
    let readme = dir.join("README.md");
    if !readme.exists() {
        fs::write(readme, "# Project memory (synced folder)\n\nShared `.project-memory/` folders for Agent Workbench projects, one folder per project (`<owner>/<repo>/`), synced by your cloud drive. Each project links its `.project-memory` here.\n")?;
    }
    Ok(json!({ "root": root }))
}

pub fn root_ready(root: &str) -> bool {
    Path::new(root).join(".git").exists() || is_folder_root(root)
}

/// Safe characters only, so an origin URL cannot produce `..` or an odd folder name
fn safe_segment(s: &str) -> String {
    let mut v: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' }).collect();
    if v.starts_with('.') {
        v = format!("_{}", v.trim_start_matches('.'));
    }
    if v.is_empty() {
        "_".into()
    } else {
        v
    }
}

fn origin_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| RegexBuilder::new(r"github\.com[/:]([^/]+)/([^/]+?)(?:\.git)?/?$").case_insensitive(true).build().unwrap())
}

/// owner/repo for a GitHub origin, otherwise local/<folder>. The raw config value: `remote get-url` applies insteadOf rewrites.
pub fn project_key(t: &Tools, workspace: &str) -> String {
    let r = git_at(t, workspace, &["config", "--get", "remote.origin.url"]);
    let url = if r.code == 0 { r.stdout.trim().to_string() } else { String::new() };
    if let Some(m) = origin_re().captures(&url) {
        return format!("{}/{}", safe_segment(&m[1]), safe_segment(&m[2]));
    }
    format!("local/{}", safe_segment(&crate::util::basename(&resolve(workspace))))
}

pub fn memory_target(root: &str, key: &str) -> String {
    let mut f = PathBuf::from(root);
    for part in key.split('/') {
        f.push(part);
    }
    p(&f)
}

pub fn gh_status(t: &Tools) -> Value {
    let Some(gh) = &t.gh else { return json!({ "installed": false, "loggedIn": false }) };
    let r = t.runner.run(gh, &["api".into(), "user".into(), "--jq".into(), ".login".into()], &RunOpts::default());
    let login = if r.code == 0 { r.stdout.trim().to_string() } else { String::new() };
    let mut o = json!({ "installed": true, "loggedIn": !login.is_empty() });
    if !login.is_empty() {
        o["login"] = json!(login);
    }
    o
}

fn repo_privacy(t: &Tools, full: &str) -> R<&'static str> {
    let gh = need_gh(t)?;
    let a: Vec<String> = ["repo", "view", full, "--json", "isPrivate", "--jq", ".isPrivate"].iter().map(|s| s.to_string()).collect();
    let r = t.runner.run(gh, &a, &RunOpts::default());
    Ok(if r.code != 0 {
        "missing"
    } else if r.stdout.trim() == "true" {
        "private"
    } else {
        "public"
    })
}

/// Commit identity for the memory repo only: the GitHub noreply address if nothing is set
fn ensure_identity(t: &Tools, dir: &str, login: Option<&str>) -> R<()> {
    let name = git_at(t, dir, &["config", "user.name"]).stdout.trim().to_string();
    let email = git_at(t, dir, &["config", "user.email"]).stdout.trim().to_string();
    let who = login.unwrap_or("workbench");
    if name.is_empty() {
        git_must(t, dir, &["config", "user.name", who])?;
    }
    if email.is_empty() {
        git_must(t, dir, &["config", "user.email", &format!("{who}@users.noreply.github.com")])?;
    }
    Ok(())
}

fn repo_name_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^([A-Za-z0-9-]{1,39}/)?[A-Za-z0-9._-]{1,100}$").unwrap())
}

/// Make sure the private repo exists and has a clone here. An existing public repo is refused: memory is never public.
pub fn ensure_repo(t: &Tools, name: &str, clone_path: &str) -> R<Value> {
    let gh = need_gh(t)?.to_string();
    let st = gh_status(t);
    if st["loggedIn"] != json!(true) {
        return err("gh-not-logged-in", "Log in to GitHub first (gh auth login)");
    }
    let login = sv(&st, "login").to_string();
    let name = if name.trim().is_empty() { DEFAULT_REPO_NAME } else { name.trim() };
    if !repo_name_re().is_match(name) {
        return err("bad-repo-name", name);
    }
    let full = if name.contains('/') { name.to_string() } else { format!("{login}/{name}") };
    let mut created = false;
    match repo_privacy(t, &full)? {
        "public" => return err("repo-public", format!("{full} is public; memory must go to a private repository")),
        "missing" => {
            must(t, &gh, &["repo", "create", &full, "--private", "--description", "Agent Workbench project memory (private)"], None, "repo-create-failed")?;
            created = true;
            if repo_privacy(t, &full)? != "private" {
                return err("repo-public", format!("{full} was not created as private"));
            }
        }
        _ => {}
    }
    // git gets its GitHub credentials through gh
    t.runner.run(&gh, &["auth".into(), "setup-git".into()], &RunOpts::default());
    let cloned = prepare_clone(t, &full, clone_path, Some(&login))?;
    Ok(json!({ "repo": full, "created": created, "cloned": cloned, "clonePath": clone_path }))
}

/// A GitHub URL (https or git@) or owner/repo, as owner/repo
pub fn parse_repo_input(input: &str) -> Option<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    let re = R.get_or_init(|| RegexBuilder::new(r"^(?:https?://(?:www\.)?github\.com/|git@github\.com:)?([A-Za-z0-9-]{1,39})/([A-Za-z0-9._-]{1,100}?)(?:\.git)?/?$").case_insensitive(true).build().unwrap());
    let m = re.captures(input.trim())?;
    Some(format!("{}/{}", &m[1], &m[2]))
}

/// The route without gh: a repo the user made themselves. Asked without logging in: 200 means public, refused; 404 means
/// private or missing, and git then confirms it can reach it. Anything else (rate limit, offline) is refused, not guessed.
pub fn connect_repo_by_url(t: &Tools, input: &str, clone_path: &str) -> R<Value> {
    let Some(full) = parse_repo_input(input) else { return err("bad-repo-url", input) };
    let status = match t.runner.http_status(&format!("https://api.github.com/repos/{full}")) {
        Ok(s) => s,
        Err(e) => return err("privacy-check-failed", e),
    };
    if status == 200 {
        return err("repo-public", format!("{full} is public; memory must go to a private repository"));
    }
    if status != 404 {
        return err("privacy-check-failed", format!("GitHub API answered {status}"));
    }
    let a: Vec<String> = vec!["ls-remote".into(), "--heads".into(), format!("https://github.com/{full}.git")];
    let ls = t.runner.run(&t.git, &a, &RunOpts { timeout_ms: Some(300_000), env: vec![("GCM_INTERACTIVE".into(), "always".into())], ..Default::default() });
    if ls.code != 0 {
        let body = if ls.stderr.is_empty() { &ls.stdout } else { &ls.stderr };
        return err("repo-unreachable", crate::util::js_prefix(body.trim(), 400));
    }
    let owner = full.split('/').next().unwrap_or("").to_string();
    let cloned = prepare_clone(t, &full, clone_path, Some(&owner))?;
    Ok(json!({ "repo": full, "created": false, "cloned": cloned, "clonePath": clone_path }))
}

/// One click: ask Git Credential Manager for the github.com login, create the private repo with that token, and confirm it is
/// private. The token only exists in memory during this call; Git's credential manager already keeps it.
pub fn create_repo_via_git_login(t: &Tools, name: &str, clone_path: &str) -> R<Value> {
    let name = if name.trim().is_empty() { DEFAULT_REPO_NAME } else { name.trim() };
    let name_ok = Regex::new(r"^[A-Za-z0-9._-]{1,100}$").unwrap().is_match(name) && name != "." && name != "..";
    if !name_ok {
        return err("bad-repo-name", name);
    }
    let cred_opts = RunOpts { input: Some("protocol=https\nhost=github.com\n\n".into()), timeout_ms: Some(300_000), env: vec![("GCM_INTERACTIVE".into(), "always".into())], ..Default::default() };
    let cred = t.runner.run(&t.git, &["credential".into(), "fill".into()], &cred_opts);
    let fields: HashMap<&str, &str> = cred.stdout.lines().filter_map(|l| l.split_once('=')).collect();
    let token = fields.get("password").copied().unwrap_or("").to_string();
    if cred.code != 0 || token.is_empty() {
        let msg = if cred.stderr.is_empty() { "no GitHub login" } else { &cred.stderr };
        return err("git-login-failed", crate::util::js_prefix(msg.trim(), 300));
    }
    let api = |method: &str, path: &str, body: Option<&Value>| t.runner.github_api(method, path, &token, body).map_err(|e| MemErr { code: "github-api-failed".into(), message: e });
    let (status, me) = api("GET", "/user", None)?;
    if status == 401 {
        // An expired login: give it back, so the next try signs in again
        let input = format!("protocol=https\nhost=github.com\nusername={}\npassword={token}\n\n", fields.get("username").copied().unwrap_or(""));
        t.runner.run(&t.git, &["credential".into(), "reject".into()], &RunOpts { input: Some(input), ..Default::default() });
        return err("git-login-failed", "GitHub login expired; try again to sign in");
    }
    let login = sv(&me, "login").to_string();
    if status != 200 || login.is_empty() {
        return err("github-api-failed", format!("GET /user answered {status}"));
    }
    let full = format!("{login}/{name}");
    let mut created = false;
    let (st, existing) = api("GET", &format!("/repos/{full}"), None)?;
    if st == 200 {
        if existing.get("private") != Some(&json!(true)) {
            return err("repo-public", format!("{full} is public; memory must go to a private repository"));
        }
    } else if st == 404 {
        let body = json!({ "name": name, "private": true, "description": "Agent Workbench project memory (private)" });
        let (made, res) = api("POST", "/user/repos", Some(&body))?;
        if made != 201 {
            return err("repo-create-failed", format!("GitHub answered {made}: {}", sv(&res, "message")));
        }
        created = true;
        let (cs, check) = api("GET", &format!("/repos/{full}"), None)?;
        if cs != 200 || check.get("private") != Some(&json!(true)) {
            return err("repo-public", format!("{full} was not created as private"));
        }
    } else {
        return err("github-api-failed", format!("GET /repos/{full} answered {st}"));
    }
    let cloned = prepare_clone(t, &full, clone_path, Some(&login))?;
    Ok(json!({ "repo": full, "created": created, "cloned": cloned, "clonePath": clone_path }))
}

/// A clone pointing at full; a new repo gets a readme and .gitignore as its first commit
fn prepare_clone(t: &Tools, full: &str, clone_path: &str, login: Option<&str>) -> R<bool> {
    let dir = Path::new(clone_path);
    let mut cloned = false;
    if dir.join(".git").exists() {
        let origin = git_at(t, clone_path, &["config", "--get", "remote.origin.url"]).stdout.trim().to_string();
        let re = RegexBuilder::new(&format!(r"[/:]{}(\.git)?/?$", regex::escape(full))).case_insensitive(true).build().unwrap();
        if !re.is_match(&origin) {
            return err("clone-other-repo", format!("{clone_path} is a clone of {origin}, not {full}"));
        }
    } else {
        if dir.exists() && fs::read_dir(dir).map(|mut r| r.next().is_some()).unwrap_or(false) {
            return err("clone-path-not-empty", format!("{clone_path} exists and is not empty"));
        }
        if let Some(d) = dir.parent() {
            fs::create_dir_all(d)?;
        }
        must(t, &t.git, &["clone", "-q", &format!("https://github.com/{full}.git"), clone_path], None, "clone-failed")?;
        cloned = true;
    }
    ensure_identity(t, clone_path, login)?;
    let ignore = dir.join(".gitignore");
    if !read_text(&ignore).unwrap_or_default().contains(&format!("{BACKUP_DIR}/")) {
        append(&ignore, &format!("# 本機搬移前的備份，只留在這台電腦\n{BACKUP_DIR}/\n"))?;
    }
    let readme = dir.join("README.md");
    if !readme.exists() {
        fs::write(readme, "# Project memory (private)\n\nShared `.project-memory/` folders for Agent Workbench projects, one folder per project (`<owner>/<repo>/`). Each project links its `.project-memory` here; do not make this repository public.\n")?;
    }
    if !git_at(t, clone_path, &["status", "--porcelain"]).stdout.trim().is_empty() {
        git_must(t, clone_path, &["add", "-A"])?;
        git_must(t, clone_path, &["commit", "-q", "-m", "memory: initialize"])?;
        git_must(t, clone_path, &["push", "-q", "-u", "origin", "HEAD"])?;
    }
    Ok(cloned)
}

fn append(file: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    fs::OpenOptions::new().create(true).append(true).open(file)?.write_all(text.as_bytes())
}

fn real_or_none(path: &Path) -> Option<String> {
    let c = fs::canonicalize(path).ok()?;
    let s = p(&c);
    Some(match s.strip_prefix(r"\\?\UNC\") {
        Some(rest) => format!(r"\\{rest}"),
        None => s.strip_prefix(r"\\?\").unwrap_or(&s).to_string(),
    })
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (real_or_none(a), real_or_none(b)) {
        (Some(x), Some(y)) => x.to_lowercase() == y.to_lowercase(),
        _ => false,
    }
}

fn lines(text: &str) -> Vec<String> {
    text.lines().filter(|l| !l.is_empty()).map(str::to_string).collect()
}

pub fn project_state(t: &Tools, workspace: &str, root: &str) -> Value {
    let key = project_key(t, workspace);
    let target = memory_target(root, &key);
    let mem = Path::new(workspace).join(MEMORY_DIR);
    let (is_link, is_dir) = match fs::symlink_metadata(&mem) {
        Ok(m) => (m.file_type().is_symlink(), !m.file_type().is_symlink() && m.is_dir()),
        Err(_) => (false, false),
    };
    let tgt = Path::new(&target);
    let linked = is_link && same_dir(&mem, tgt);
    let linked_to = if is_link && !linked { real_or_none(&mem) } else { None };
    let mut args: Vec<&str> = vec!["ls-files", "--", MEMORY_DIR];
    args.extend(ENTRY_FILES.iter().map(|e| e.0));
    let r = git_at(t, workspace, &args);
    let all = if r.code == 0 { lines(&r.stdout) } else { Vec::new() };
    let entries: Vec<&str> = ENTRY_FILES.iter().map(|e| e.0).collect();
    let has_local = is_dir && mem.join("handoff.md").exists();
    let has_remote = tgt.join("handoff.md").exists();
    let mut o = json!({ "key": key, "target": target, "linked": linked, "linkedElsewhere": is_link && !linked });
    if let Some(to) = &linked_to {
        o["linkedTo"] = json!(to);
        // A link Workbench made itself into another store's <owner>/<repo>: moving it is safe to automate
        let tail = format!("/{}", key.to_lowercase());
        o["linkedToOldStore"] = json!(to.replace('\\', "/").to_lowercase().ends_with(&tail) && Path::new(to).join("handoff.md").exists());
    }
    o["hasLocalMemory"] = json!(has_local);
    o["hasRemoteMemory"] = json!(has_remote);
    o["tracked"] = json!(all.iter().filter(|f| !entries.contains(&f.as_str())).collect::<Vec<_>>());
    o["trackedEntries"] = json!(all.iter().filter(|f| entries.contains(&f.as_str())).collect::<Vec<_>>());
    o["memoryDiffers"] = json!(has_local && has_remote && !same_memory(&mem, tgt));
    o["branch"] = json!(git_at(t, workspace, &["symbolic-ref", "-q", "--short", "HEAD"]).stdout.trim());
    o
}

/// Memory content by relative path; not the scripts Workbench overwrites, entry templates or backups
fn memory_files(dir: &Path, base: &Path, out: &mut BTreeMap<String, String>) {
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            if !["entry", BACKUP_DIR, ".git"].contains(&name.as_str()) || dir != base {
                memory_files(&path, base, out);
            }
        } else if !name.ends_with(".sh") {
            let rel = path.strip_prefix(base).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            out.insert(rel, read_text(&path).unwrap_or_default().replace("\r\n", "\n"));
        }
    }
}

fn same_memory(a: &Path, b: &Path) -> bool {
    let (mut x, mut y) = (BTreeMap::new(), BTreeMap::new());
    memory_files(a, a, &mut x);
    memory_files(b, b, &mut y);
    x == y
}

/// What stops an automatic link. It may never touch the project repo or overwrite different content.
pub fn auto_link_block(st: &Value) -> Value {
    if st["tracked"].as_array().map(|a| !a.is_empty()).unwrap_or(false) {
        return json!("tracked");
    }
    if st["linked"] != json!(true) && st["memoryDiffers"] == json!(true) {
        return json!("differs");
    }
    Value::Null
}

fn ensure_project_ignore(workspace: &str) -> std::io::Result<()> {
    let file = Path::new(workspace).join(".gitignore");
    let text = read_text(&file).unwrap_or_default();
    if text.lines().map(str::trim).any(|l| l == ".project-memory" || l == ".project-memory/") {
        return Ok(());
    }
    let sep = if !text.is_empty() && !text.ends_with('\n') { "\n" } else { "" };
    fs::write(file, format!("{text}{sep}\n# 專案記憶放在記憶同步的存放位置（Agent Workbench：設定 → 記憶同步），不進這個 repo\n.project-memory/\n"))
}

/// Ignored in this clone only: .git/info/exclude, no commit, nothing pushed. Tracked files are unaffected,
/// and the leading / means an AGENTS.md in a subfolder is too.
pub fn ensure_local_exclude(t: &Tools, workspace: &str) -> R<()> {
    let out = git_must(t, workspace, &["rev-parse", "--git-path", "info/exclude"])?;
    let file = PathBuf::from(resolve(&p(&Path::new(workspace).join(out.trim()))));
    let text = read_text(&file).unwrap_or_default();
    let have: HashSet<&str> = text.lines().map(str::trim).collect();
    let missing: Vec<String> = std::iter::once(MEMORY_DIR).chain(ENTRY_FILES.iter().map(|e| e.0)).map(|x| format!("/{x}")).filter(|l| !have.contains(l.as_str())).collect();
    if missing.is_empty() {
        return Ok(());
    }
    if let Some(d) = file.parent() {
        fs::create_dir_all(d)?;
    }
    let sep = if !text.is_empty() && !text.ends_with('\n') { "\n" } else { "" };
    append(&file, &format!("{sep}# Agent Workbench memory sync (this clone only)\n{}\n", missing.join("\n")))?;
    Ok(())
}

fn bash_for(git_path: &str) -> Option<String> {
    if !cfg!(windows) {
        return Some("bash".into());
    }
    // ...\Git\cmd\git.exe -> ...\Git\bin\bash.exe
    let root = dirname(&dirname(git_path));
    ["bin\\bash.exe", "usr\\bin\\bash.exe"].iter().map(|rel| p(&Path::new(&root).join(rel))).find(|x| exists(x))
}

/// An entry file without the block generated from the handoff. Each machine regenerates that block;
/// stored in the memory repo it would conflict on every handoff.
pub fn entry_template(text: &str) -> String {
    let mut out = Vec::new();
    let mut skip = false;
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.contains("<!-- BEGIN AUTO-MEMORY") {
            skip = true;
        }
        if !skip {
            out.push(line);
        }
        if line.contains("<!-- END AUTO-MEMORY -->") {
            skip = false;
        }
    }
    out.join("\n")
}

/// Restore entry files from entry/ and regenerate their memory block. An entry file the project tracks itself is left alone:
/// a handoff written into it would become a change to commit.
fn restore_entry_files(t: &Tools, workspace: &str, target: &str) -> R<()> {
    let r = git_at(t, workspace, &["ls-files", "--", ENTRY_FILES[0].0, ENTRY_FILES[1].0]);
    let tracked = if r.code == 0 { lines(&r.stdout) } else { Vec::new() };
    for (project, stored) in ENTRY_FILES {
        if tracked.iter().any(|f| f == project) {
            continue;
        }
        let dst = Path::new(workspace).join(project);
        let saved = Path::new(target).join("entry").join(stored);
        if !dst.exists() && saved.exists() {
            if let Some(d) = dst.parent() {
                fs::create_dir_all(d)?;
            }
            fs::copy(&saved, &dst)?;
        } else if dst.exists() && !saved.exists() {
            if let Some(d) = saved.parent() {
                fs::create_dir_all(d)?;
            }
            fs::write(&saved, entry_template(&read_text(&dst).unwrap_or_default()))?;
        }
    }
    let sync = Path::new(workspace).join(MEMORY_DIR).join("sync-entry-files.sh");
    if let Some(bash) = bash_for(&t.git).filter(|_| sync.exists()) {
        t.runner.run(&bash, &[p(&sync)], &at(workspace));
    }
    Ok(())
}

/// Replace the memory folder's scripts with the bundled ones (the old commit-handoff.sh would `git add -f` memory into the
/// project repo). Written only if different. Returns what changed.
pub fn ensure_memory_scripts(target: &str, scripts: &BTreeMap<String, String>) -> R<Vec<String>> {
    static R: OnceLock<Regex> = OnceLock::new();
    let name_ok = R.get_or_init(|| Regex::new(r"^[A-Za-z0-9._-]+\.sh$").unwrap());
    let mut updated = Vec::new();
    for (name, content) in scripts {
        if !name_ok.is_match(name) {
            continue;
        }
        let file = Path::new(target).join(name);
        let want = content.replace("\r\n", "\n");
        if read_text(&file).map(|h| h.replace("\r\n", "\n")).as_deref() == Some(want.as_str()) {
            continue;
        }
        fs::write(&file, want)?;
        updated.push(name.clone());
    }
    Ok(updated)
}

fn copy_dir(from: &Path, to: &Path, skip: &dyn Fn(&str) -> bool) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if skip(&name) {
            continue;
        }
        let dst = to.join(&name);
        if e.path().is_dir() {
            copy_dir(&e.path(), &dst, skip)?;
        } else {
            fs::copy(e.path(), dst)?;
        }
    }
    Ok(())
}

fn link_dir(target: &str, link: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        junction::create(target, link)
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link)
    }
}

/// Removes only the link, never what it points to
fn unlink_dir(link: &Path) -> std::io::Result<()> {
    if cfg!(windows) {
        fs::remove_dir(link)
    } else {
        fs::remove_file(link)
    }
}

#[derive(Default)]
pub struct LinkOpts {
    pub login: Option<String>,
    pub scripts: Option<BTreeMap<String, String>>,
    pub migrate: bool,
    pub allow_commit: bool,
    pub prefer: Option<String>,
}

/// First machine: move this project's memory in and push. Other machines: link what is already there.
/// Only a confirmed allow_commit may produce a "stop tracking" commit; differing memory needs prefer.
pub fn link_project(t: &Tools, workspace: &str, root: &str, o: &LinkOpts) -> R<Value> {
    if !root_ready(root) {
        return err("no-clone", "Set up the memory storage first");
    }
    let st = project_state(t, workspace, root);
    let tracked: Vec<String> = st["tracked"].as_array().unwrap().iter().filter_map(Value::as_str).map(str::to_string).collect();
    let flag = |k: &str| st[k] == json!(true);
    if !tracked.is_empty() && !o.allow_commit {
        return err("needs-review", format!("The project repository tracks {} memory file(s)", tracked.len()));
    }
    if !flag("linked") && !flag("linkedElsewhere") && flag("memoryDiffers") && o.prefer.is_none() {
        return err("memory-differs", "Local memory differs from the shared copy");
    }
    let mem = Path::new(workspace).join(MEMORY_DIR);
    let key = sv(&st, "key").to_string();
    let target = sv(&st, "target").to_string();
    let tgt = Path::new(&target);
    let mut source = "already-linked";
    let mut backup: Option<String> = None;
    let new_backup = || p(&Path::new(root).join(BACKUP_DIR).join(key.replacen('/', "__", 1)).join(iso(now_ms()).replace([':', '.'], "-")));
    let none = |_: &str| false;
    if flag("linkedElsewhere") {
        // A different store now: copy the memory over and point the link there; the old copy stays
        if !o.migrate {
            return err("linked-elsewhere", format!("{MEMORY_DIR} already links somewhere else"));
        }
        let from = sv(&st, "linkedTo").to_string();
        if from.is_empty() || !Path::new(&from).join("handoff.md").exists() {
            return err("linked-elsewhere", format!("{MEMORY_DIR} links to a folder without memory"));
        }
        let b = new_backup();
        copy_dir(Path::new(&from), Path::new(&b), &none)?;
        backup = Some(b);
        if !flag("hasRemoteMemory") {
            copy_dir(Path::new(&from), tgt, &none)?;
            source = "local";
        } else {
            source = "repo";
        }
        unlink_dir(&mem)?;
        link_dir(&target, &mem)?;
    } else if !flag("linked") {
        if !flag("hasLocalMemory") && !flag("hasRemoteMemory") {
            return err("no-memory", "This project has no .project-memory yet");
        }
        if flag("hasLocalMemory") {
            let b = new_backup();
            copy_dir(&mem, Path::new(&b), &none)?;
            if !flag("hasRemoteMemory") || (flag("memoryDiffers") && o.prefer.as_deref() == Some("local")) {
                if flag("hasRemoteMemory") {
                    // This clone's memory replaces the shared one, which gets a backup first
                    copy_dir(tgt, Path::new(&format!("{b}-shared")), &none)?;
                    fs::remove_dir_all(tgt)?;
                }
                copy_dir(&mem, tgt, &none)?;
                source = "local";
            } else {
                source = "repo";
            }
            backup = Some(b);
            fs::remove_dir_all(&mem)?;
        } else {
            source = "repo";
            if mem.exists() {
                fs::remove_dir_all(&mem)?; // An empty leftover, or one with no handoff
            }
        }
        link_dir(&target, &mem)?;
    }
    if let Some(s) = &o.scripts {
        ensure_memory_scripts(&target, s)?;
    }
    restore_entry_files(t, workspace, &target)?;
    // The memory repo: push this project's memory. A cloud folder's client uploads it instead.
    if Path::new(root).join(".git").exists() && !git_at(t, root, &["status", "--porcelain", "--", &key]).stdout.trim().is_empty() {
        ensure_identity(t, root, o.login.as_deref())?;
        git_must(t, root, &["add", "-A", "--", &key])?;
        git_must(t, root, &["commit", "-q", "-m", &format!("memory: link {key}"), "--", &key])?;
        git_must(t, root, &["push", "-q", "-u", "origin", "HEAD"])?;
    }
    ensure_local_exclude(t, workspace)?;
    let mut out = json!({ "key": key, "target": target, "source": source });
    if let Some(b) = backup {
        out["backup"] = json!(b);
    }
    if !tracked.is_empty() {
        ensure_project_ignore(workspace)?;
        out["projectCommit"] = json!(commit_untrack(t, workspace, &tracked, o.login.as_deref())?);
    }
    Ok(out)
}

/// The "stop tracking memory" commit, built with a temporary index: git rm refuses paths through the link,
/// and anything the user has staged must not come along. The same change is then applied to the real index.
fn commit_untrack(t: &Tools, workspace: &str, tracked: &[String], login: Option<&str>) -> R<String> {
    let git_dir = PathBuf::from(resolve(&p(&Path::new(workspace).join(git_must(t, workspace, &["rev-parse", "--git-dir"])?.trim()))));
    let tmp = git_dir.join(format!("memsync-index-{}", std::process::id()));
    let opts = RunOpts { cwd: Some(workspace.to_string()), env: vec![("GIT_INDEX_FILE".into(), p(&tmp))], ..Default::default() };
    let run = |args: Vec<String>| -> R<String> {
        let r = t.runner.run(&t.git, &args, &opts);
        if r.code != 0 {
            let body = if r.stderr.is_empty() { &r.stdout } else { &r.stderr };
            return err("project-commit-failed", format!("git {}: {}", args.join(" "), crate::util::js_prefix(body.trim(), 400)));
        }
        Ok(r.stdout.trim().to_string())
    };
    let s = |a: &[&str]| -> Vec<String> { a.iter().map(|x| x.to_string()).collect() };
    let result = (|| -> R<String> {
        let parent = git_must(t, workspace, &["rev-parse", "HEAD"])?.trim().to_string();
        run(s(&["read-tree", &parent]))?;
        let mut remove = s(&["update-index", "--force-remove", "--"]);
        remove.extend(tracked.iter().cloned());
        run(remove.clone())?;
        let blob = run(s(&["hash-object", "-w", "--", ".gitignore"]))?;
        let entry = format!("100644,{blob},.gitignore");
        run(s(&["update-index", "--add", "--cacheinfo", &entry]))?;
        let tree = run(s(&["write-tree"]))?;
        let name = git_at(t, workspace, &["config", "user.name"]).stdout.trim().to_string();
        let email = git_at(t, workspace, &["config", "user.email"]).stdout.trim().to_string();
        let who = login.unwrap_or("workbench");
        let mut commit_args = Vec::new();
        if name.is_empty() {
            commit_args.extend(s(&["-c", &format!("user.name={who}")]));
        }
        if email.is_empty() {
            commit_args.extend(s(&["-c", &format!("user.email={who}@users.noreply.github.com")]));
        }
        commit_args.extend(s(&["commit-tree", &tree, "-p", &parent, "-m", "chore: move project memory to a private repository"]));
        let commit = run(commit_args)?;
        git_must(t, workspace, &["update-ref", "-m", "memsync: untrack project memory", "HEAD", &commit, &parent])?;
        let refs: Vec<&str> = remove.iter().map(String::as_str).collect();
        git_must(t, workspace, &refs)?;
        git_must(t, workspace, &["update-index", "--add", "--cacheinfo", &entry])?;
        Ok(commit[..7.min(commit.len())].to_string())
    })();
    let _ = fs::remove_file(&tmp);
    result
}

pub fn clone_state(t: &Tools, clone_path: &str) -> Value {
    if !Path::new(clone_path).join(".git").exists() {
        return json!({ "exists": false });
    }
    let origin = git_at(t, clone_path, &["config", "--get", "remote.origin.url"]).stdout.trim().to_string();
    let counts = git_at(t, clone_path, &["rev-list", "--left-right", "--count", "HEAD...@{u}"]).stdout;
    let mut c = counts.split_whitespace();
    let n = |x: Option<&str>| x.and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let (ahead, behind) = (n(c.next()), n(c.next()));
    let dirty = !git_at(t, clone_path, &["status", "--porcelain"]).stdout.trim().is_empty();
    json!({ "exists": true, "origin": origin, "ahead": ahead, "behind": behind, "dirty": dirty })
}

/// Commit what this machine has not committed, pull with rebase, push. pull_only only pulls.
pub fn sync_now(t: &Tools, root: &str, workspace: Option<&str>, pull_only: bool, login: Option<&str>) -> R<Value> {
    let mut res = json!({ "committed": false, "pulled": false, "pushed": false });
    let refresh = |t: &Tools| -> R<()> {
        if let Some(ws) = workspace {
            let st = project_state(t, ws, root);
            if st["linked"] == json!(true) {
                restore_entry_files(t, ws, sv(&st, "target"))?;
            }
        }
        Ok(())
    };
    // A cloud folder is synced by its own client; only the entry files are regenerated
    if is_folder_root(root) {
        refresh(t)?;
        return Ok(res);
    }
    if !Path::new(root).join(".git").exists() {
        return err("no-clone", "Set up the private memory repository first");
    }
    if !pull_only && !git_at(t, root, &["status", "--porcelain"]).stdout.trim().is_empty() {
        ensure_identity(t, root, login)?;
        git_must(t, root, &["add", "-A"])?;
        let host = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "this computer".into());
        git_must(t, root, &["commit", "-q", "-m", &format!("memory: sync from {host}")])?;
        res["committed"] = json!(true);
    }
    let pull = git_at(t, root, &["pull", "-q", "--rebase", "--autostash"]);
    if pull.code != 0 {
        git_at(t, root, &["rebase", "--abort"]);
        let body = if pull.stderr.is_empty() { &pull.stdout } else { &pull.stderr };
        let msg = crate::util::js_prefix(body.trim(), 400);
        res["error"] = json!(if msg.is_empty() { "pull failed".to_string() } else { msg });
        return Ok(res);
    }
    res["pulled"] = json!(true);
    if !pull_only {
        let ahead = git_at(t, root, &["rev-list", "--count", "@{u}..HEAD"]).stdout.trim().to_string();
        if !ahead.is_empty() && ahead != "0" {
            let push = git_at(t, root, &["push", "-q"]);
            if push.code != 0 {
                let body = if push.stderr.is_empty() { &push.stdout } else { &push.stderr };
                let msg = crate::util::js_prefix(body.trim(), 400);
                res["error"] = json!(if msg.is_empty() { "push failed".to_string() } else { msg });
                return Ok(res);
            }
            res["pushed"] = json!(true);
        }
    }
    refresh(t)?;
    Ok(res)
}

// ── The shared-project-memory plugin that ships with the app ──
// Copied into a local marketplace "agent-workbench" and installed from there, so its version follows the app. The names differ
// from the GitHub version, so the new one goes in and is confirmed before the old one comes out. Antigravity has no marketplace:
// `agy plugin install <folder>` replaces a same-named copy, and a marker file identifies ours.

#[derive(Clone)]
pub struct Homes {
    pub claude: PathBuf,
    pub codex: PathBuf,
    pub antigravity: PathBuf,
}

pub fn default_homes() -> Homes {
    let h = home();
    let var = |k: &str, d: PathBuf| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or(d);
    Homes { claude: var("CLAUDE_CONFIG_DIR", h.join(".claude")), codex: var("CODEX_HOME", h.join(".codex")), antigravity: h.join(".gemini").join("config") }
}

fn read_json(path: &Path) -> Value {
    read_text(path).and_then(|t| serde_json::from_str(&t).ok()).filter(|v: &Value| !v.is_null()).unwrap_or(json!({}))
}

fn codex_plugin_keys(dir: &Path) -> Vec<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    let re = R.get_or_init(|| Regex::new(r#"(?m)^\[plugins\."([^"]+)"\]"#).unwrap());
    re.captures_iter(&read_text(&dir.join("config.toml")).unwrap_or_default()).map(|m| m[1].to_string()).collect()
}

fn claude_plugin_keys(dir: &Path) -> Vec<String> {
    read_json(&dir.join("plugins").join("installed_plugins.json"))["plugins"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default()
}

fn row(key: &str, marketplace: &str, version: Option<String>, scope: &str) -> Value {
    json!({ "key": key, "marketplace": marketplace, "version": version, "scope": scope })
}

pub fn installed_plugins(agent: &str, homes: &Homes) -> Vec<Value> {
    let prefix = format!("{PLUGIN}@");
    match agent {
        "claude" => {
            let doc = read_json(&homes.claude.join("plugins").join("installed_plugins.json"));
            let mut out = Vec::new();
            for (key, entries) in doc["plugins"].as_object().into_iter().flatten() {
                let Some(market) = key.strip_prefix(&prefix) else { continue };
                let list = entries.as_array().filter(|a| !a.is_empty()).cloned().unwrap_or_else(|| vec![json!({})]);
                for e in list {
                    let scope = Some(sv(&e, "scope")).filter(|s| !s.is_empty()).unwrap_or("user");
                    out.push(row(key, market, e.get("version").and_then(Value::as_str).map(str::to_string), scope));
                }
            }
            out
        }
        "codex" => codex_plugin_keys(&homes.codex)
            .into_iter()
            .filter_map(|key| {
                let market = key.strip_prefix(&prefix)?.to_string();
                let mut versions: Vec<String> = fs::read_dir(homes.codex.join("plugins").join("cache").join(&market).join(PLUGIN)).map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
                versions.sort_by(|a, b| numeric_cmp(a, b));
                Some(row(&key, &market, versions.pop(), "user"))
            })
            .collect(),
        _ => fs::read_dir(homes.antigravity.join("plugins"))
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| {
                        let d = e.file_name().to_string_lossy().into_owned();
                        let m = read_json(&e.path().join("plugin.json"));
                        // Before 1.2.0 the name had a typo
                        if sv(&m, "name") != PLUGIN && sv(&m, "name") != "share-project-memory" {
                            return None;
                        }
                        let key = if e.path().join(BUNDLED_MARKER).exists() { bundled_key() } else { format!("{PLUGIN}@{d}") };
                        Some(row(&key, &d, m.get("version").and_then(Value::as_str).map(str::to_string), "user"))
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// localeCompare with numeric: 1.10.0 sorts after 1.9.0
fn numeric_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let parts = |s: &str| -> Vec<u64> { s.split('.').map(|x| x.parse().unwrap_or(0)).collect() };
    parts(a).cmp(&parts(b))
}

fn marketplace_known(agent: &str, homes: &Homes) -> bool {
    if agent == "claude" {
        return read_json(&homes.claude.join("plugins").join("known_marketplaces.json")).get(MARKETPLACE).is_some();
    }
    Regex::new(&format!(r"(?m)^\[marketplaces\.{MARKETPLACE}\]")).unwrap().is_match(&read_text(&homes.codex.join("config.toml")).unwrap_or_default())
}

fn marketplace_in_use(agent: &str, homes: &Homes, marketplace: &str) -> bool {
    let keys = if agent == "claude" { claude_plugin_keys(&homes.claude) } else { codex_plugin_keys(&homes.codex) };
    keys.iter().any(|k| k.ends_with(&format!("@{marketplace}")))
}

pub fn plugin_version(dir: &Path) -> R<String> {
    match read_json(&dir.join(".claude-plugin").join("plugin.json")).get("version").and_then(Value::as_str) {
        Some(v) => Ok(v.to_string()),
        None => err("plugin-missing", format!("no plugin manifest in {}", dir.display())),
    }
}

pub fn plugin_action(installed: &[Value], bundled_version: &str) -> &'static str {
    let ours = bundled_key();
    if installed.iter().any(|x| sv(x, "key") != ours && sv(x, "scope") == "user") {
        return "migrate";
    }
    let mine: Vec<&Value> = installed.iter().filter(|x| sv(x, "key") == ours).collect();
    if mine.is_empty() {
        return "install";
    }
    if mine.iter().all(|x| sv(x, "version") == bundled_version) {
        "ok"
    } else {
        "update"
    }
}

/// The bundled plugin as a local marketplace. CLIs copy from here into their own cache, so a running session never locks it.
pub fn prepare_marketplace(bundled: &Path, dir: &Path) -> R<()> {
    let target = dir.join(PLUGIN);
    let version = plugin_version(bundled)?;
    if plugin_version(&target).ok().as_deref() != Some(version.as_str()) {
        if target.exists() {
            fs::remove_dir_all(&target)?;
        }
        copy_dir(bundled, &target, &|n| n == "tests")?;
    }
    fs::write(target.join(BUNDLED_MARKER), format!("Installed by Agent Workbench ({MARKETPLACE}).\n"))?;
    let write = |rel: &str, data: Value| -> std::io::Result<()> {
        let f = dir.join(rel);
        if let Some(d) = f.parent() {
            fs::create_dir_all(d)?;
        }
        fs::write(f, serde_json::to_string_pretty(&data).unwrap() + "\n")
    };
    let description = "Cross-tool shared project memory, bundled with Agent Workbench.";
    write(".claude-plugin/marketplace.json", json!({ "name": MARKETPLACE, "owner": { "name": "Agent Workbench" }, "plugins": [{ "name": PLUGIN, "source": format!("./{PLUGIN}"), "description": description }] }))?;
    write(
        ".agents/plugins/marketplace.json",
        json!({ "name": MARKETPLACE, "interface": { "displayName": "Agent Workbench" }, "plugins": [{ "name": PLUGIN, "source": { "source": "local", "path": format!("./{PLUGIN}") }, "policy": { "installation": "AVAILABLE", "authentication": "ON_INSTALL" }, "category": "Productivity" }] }),
    )?;
    Ok(())
}

pub type CliExec<'a> = dyn Fn(&[String]) -> RunResult + 'a;

/// Install the bundled version, confirm it, and only then remove others. If the new one fails the old one stays.
/// Only user scope: a project-scope install is the user's. Returns the commands that ran.
pub fn apply_bundled_plugin(agent: &str, exec: &CliExec, marketplace_dir: &Path, homes: &Homes) -> R<Vec<String>> {
    let log = Mutex::new(Vec::new());
    let step = |args: Vec<String>, allow_fail: bool| -> R<()> {
        let line = format!("{agent} {}", args.join(" "));
        log.lock().unwrap().push(line.clone());
        let r = exec(&args);
        if r.code != 0 && !allow_fail {
            let body = if r.stderr.is_empty() { &r.stdout } else { &r.stderr };
            return err("plugin-install-failed", format!("{line}: {}", crate::util::js_prefix(body.trim(), 400)));
        }
        Ok(())
    };
    let s = |a: &[&str]| -> Vec<String> { a.iter().map(|x| x.to_string()).collect() };
    let key = bundled_key();
    let bundled_version = plugin_version(&marketplace_dir.join(PLUGIN))?;
    let before = installed_plugins(agent, homes);
    let ours = before.iter().find(|x| sv(x, "key") == key).cloned();
    let dir_str = p(marketplace_dir);
    if agent == "antigravity" {
        // A same-named old version is replaced whole; any other copy left afterwards gets removed
        step(s(&["plugin", "install", &p(&marketplace_dir.join(PLUGIN))]), false)?;
        let after = installed_plugins(agent, homes);
        if !after.iter().any(|x| sv(x, "key") == key && sv(x, "version") == bundled_version) {
            return err("plugin-install-failed", format!("{agent}: {key} {bundled_version} is not listed as installed after the install"));
        }
        for old in after.iter().filter(|x| sv(x, "key") != key) {
            step(s(&["plugin", "uninstall", sv(old, "marketplace")]), false)?;
        }
        return Ok(log.into_inner().unwrap());
    }
    let stale = ours.as_ref().map(|o| sv(o, "version") != bundled_version).unwrap_or(false);
    if agent == "claude" {
        step(if marketplace_known(agent, homes) { s(&["plugin", "marketplace", "update", MARKETPLACE]) } else { s(&["plugin", "marketplace", "add", &dir_str]) }, false)?;
        if ours.is_none() {
            step(s(&["plugin", "install", &key, "--scope", "user"]), false)?;
        } else if stale {
            step(s(&["plugin", "update", &key]), false)?;
        }
    } else {
        if !marketplace_known(agent, homes) {
            step(s(&["plugin", "marketplace", "add", &dir_str]), false)?;
        }
        // A local marketplace has no upgrade: remove, then add
        if stale {
            step(s(&["plugin", "remove", &key]), false)?;
        }
        if ours.is_none() || stale {
            step(s(&["plugin", "add", &key]), false)?;
        }
    }
    if !installed_plugins(agent, homes).iter().any(|x| sv(x, "key") == key) {
        return err("plugin-install-failed", format!("{agent}: {key} is not listed as installed after the install"));
    }
    for old in before.iter().filter(|x| sv(x, "key") != key && sv(x, "scope") == "user") {
        let k = sv(old, "key");
        step(if agent == "claude" { s(&["plugin", "uninstall", k, "--scope", "user"]) } else { s(&["plugin", "remove", k]) }, false)?;
        if !marketplace_in_use(agent, homes, sv(old, "marketplace")) {
            step(s(&["plugin", "marketplace", "remove", sv(old, "marketplace")]), true)?;
        }
    }
    Ok(log.into_inner().unwrap())
}

// ── Wiring ──

const COMMIT_HANDOFF: &str = include_str!("../../../../plugins/shared-project-memory/templates/commit-handoff.sh");
const STATUS: &str = include_str!("../../../../plugins/shared-project-memory/templates/status.sh");
const SYNC_ENTRY: &str = include_str!("../../../../plugins/shared-project-memory/templates/sync-entry-files.sh");

fn memory_scripts() -> BTreeMap<String, String> {
    BTreeMap::from([("commit-handoff.sh".to_string(), COMMIT_HANDOFF.to_string()), ("status.sh".to_string(), STATUS.to_string()), ("sync-entry-files.sh".to_string(), SYNC_ENTRY.to_string())])
}

const BACKENDS: [&str; 3] = ["github", "onedrive", "gdrive"];

#[derive(Default)]
pub struct MemSync {
    /// One thing at a time may touch the memory root: background sync, manual sync, link
    lock: Mutex<()>,
}

fn config_file(core: &Core) -> PathBuf {
    core.data_dir.join("memory-sync.json")
}

pub fn load_config(core: &Core) -> Value {
    let default_clone = p(&home().join(".ai-project-memory"));
    let raw = read_text(&config_file(core)).and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let d = json!({ "consentAt": null, "backend": null, "repo": DEFAULT_REPO_NAME, "clonePath": default_clone, "repoFull": null, "login": null, "folderPath": null, "autoSync": true, "autoLink": true, "pluginDeclined": null });
    let Some(raw) = raw else { return d };
    let s = |k: &str| raw.get(k).and_then(Value::as_str);
    let abs = |k: &str| s(k).filter(|v| Path::new(v).is_absolute());
    let opt = |v: Option<&str>| v.map(|x| json!(x)).unwrap_or(Value::Null);
    json!({
        "consentAt": raw.get("consentAt").filter(|v| v.is_number()).cloned().unwrap_or(Value::Null),
        "backend": s("backend").filter(|b| BACKENDS.contains(b)).map(|b| json!(b)).unwrap_or(Value::Null),
        "repo": s("repo").map(str::trim).filter(|r| !r.is_empty()).unwrap_or(DEFAULT_REPO_NAME),
        "clonePath": abs("clonePath").unwrap_or(&default_clone),
        "repoFull": opt(s("repoFull")),
        "login": opt(s("login")),
        "folderPath": opt(abs("folderPath")),
        // Older settings called this autoPull
        "autoSync": raw.get("autoSync").or(raw.get("autoPull")) != Some(&json!(false)),
        "autoLink": raw.get("autoLink") != Some(&json!(false)),
        "pluginDeclined": opt(s("pluginDeclined")),
    })
}

fn save_config(core: &Core, c: &Value) -> R<()> {
    let f = config_file(core);
    if let Some(d) = f.parent() {
        fs::create_dir_all(d)?;
    }
    fs::write(f, serde_json::to_string_pretty(c).unwrap())?;
    Ok(())
}

fn memory_root(c: &Value) -> Option<String> {
    match sv(c, "backend") {
        "github" => Some(sv(c, "clonePath").to_string()),
        "onedrive" | "gdrive" => c.get("folderPath").and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
}

fn active_root(c: &Value) -> Option<String> {
    let root = memory_root(c)?;
    (!c["consentAt"].is_null() && root_ready(&root)).then_some(root)
}

fn tools() -> R<Tools> {
    let Some(git) = find_cli("git", true) else { return err("git-missing", "Git is not installed") };
    Ok(Tools { git, gh: find_cli("gh", true), runner: Arc::new(RealRunner) })
}

/// Pasted into a visible terminal for the user to run. gh may be newly installed and not on this process's PATH.
fn commands(gh: Option<&str>) -> Value {
    let win = cfg!(windows);
    let g = match gh {
        Some(x) if win => format!("& \"{x}\""),
        Some(x) => format!("\"{x}\""),
        None => "gh".to_string(),
    };
    json!({
        "shell": if win { "powershell" } else { "bash" },
        "installGh": if win { "winget install --id GitHub.cli -e --source winget" } else { "brew install gh  # or see https://cli.github.com" },
        "loginGh": format!("{g} auth login --hostname github.com --git-protocol https --web; {g} auth setup-git"),
        "installOneDrive": if win { "winget install --id Microsoft.OneDrive -e --source winget" } else { "brew install --cask onedrive" },
        "installGDrive": if win { "winget install --id Google.GoogleDrive -e --source winget" } else { "brew install --cask google-drive" },
    })
}

fn is_git_repo(t: &Tools, dir: &str) -> bool {
    !dir.is_empty() && exists(dir) && git_at(t, dir, &["rev-parse", "--is-inside-work-tree"]).stdout.trim() == "true"
}

fn state(core: &Core, workspace: &str) -> Value {
    let config = load_config(core);
    let (cloud_roots, cloud_apps) = (detect_cloud_roots(), detect_cloud_apps());
    let root = memory_root(&config);
    let ready = root.as_deref().map(root_ready).unwrap_or(false);
    let Ok(t) = tools() else {
        return json!({ "config": config, "cloudRoots": cloud_roots, "cloudApps": cloud_apps, "root": root, "rootReady": ready, "gitMissing": true, "gh": { "installed": false, "loggedIn": false }, "clone": { "exists": false }, "project": null, "commands": commands(None) });
    };
    // gh talks to the network: only in GitHub mode
    let gh = if sv(&config, "backend") == "github" { gh_status(&t) } else { json!({ "installed": t.gh.is_some(), "loggedIn": false }) };
    let clone = clone_state(&t, sv(&config, "clonePath"));
    let project = match &root {
        Some(r) if is_git_repo(&t, workspace) => project_state(&t, workspace, r),
        _ => Value::Null,
    };
    json!({ "config": config, "cloudRoots": cloud_roots, "cloudApps": cloud_apps, "root": root, "rootReady": ready, "gitMissing": false, "gh": gh, "clone": clone, "project": project, "commands": commands(t.gh.as_deref()) })
}

fn bundled_plugin_dir(core: &Core) -> PathBuf {
    let from_resources = core.info.resources.join("plugins").join(PLUGIN);
    if from_resources.exists() {
        return from_resources;
    }
    // Not packaged: the repository itself
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("..").join("plugins").join(PLUGIN)
}

fn marketplace_dir(core: &Core) -> PathBuf {
    core.data_dir.join("plugin-marketplace")
}

fn plugin_state(core: &Core) -> R<Value> {
    let bundled = plugin_version(&bundled_plugin_dir(core))?;
    let settings = core.load_settings();
    let homes = default_homes();
    let agents: Vec<Value> = PLUGIN_AGENTS
        .iter()
        .map(|agent| {
            let installed = installed_plugins(agent, &homes);
            json!({
                "agent": agent,
                "enabled": settings["cliEnabled"][agent] != json!(false),
                "cli": core.find_agent_cli(agent, true).is_some(),
                "installed": installed.iter().map(|x| json!({ "key": x["key"], "version": x["version"], "scope": x["scope"] })).collect::<Vec<_>>(),
                "action": plugin_action(&installed, &bundled),
            })
        })
        .collect();
    Ok(json!({ "bundledVersion": bundled, "declined": load_config(core)["pluginDeclined"], "agents": agents }))
}

fn require_consent(c: &Value) -> R<()> {
    if c["consentAt"].is_null() {
        return err("no-consent", "Memory sync has not been enabled");
    }
    Ok(())
}

fn require_root(c: &Value) -> R<String> {
    require_consent(c)?;
    match active_root(c) {
        Some(r) => Ok(r),
        None => err("no-clone", "Set up the memory storage first"),
    }
}

fn require_github(c: &Value) -> R<()> {
    require_consent(c)?;
    if sv(c, "backend") != "github" {
        return err("bad-backend", sv(c, "backend"));
    }
    Ok(())
}

fn on_project_open(core: &Core, workspace: &str) -> R<Value> {
    let c = load_config(core);
    let Some(root) = active_root(&c) else {
        // Sync off still replaces an old commit-handoff.sh, which would force memory into the project repo. Local files, no network.
        let mem = Path::new(workspace).join(MEMORY_DIR);
        if mem.join("commit-handoff.sh").exists() {
            ensure_memory_scripts(&p(&mem), &memory_scripts())?;
        }
        return Ok(Value::Null);
    };
    let t = tools()?;
    if !is_git_repo(&t, workspace) {
        return Ok(Value::Null);
    }
    let _g = core.features().memsync.lock.lock().unwrap();
    let auto_sync = c["autoSync"] == json!(true);
    let pulled = if sv(&c, "backend") == "github" && !auto_sync { Value::Null } else { sync_now(&t, &root, Some(workspace), true, None)? };
    let st = project_state(&t, workspace, &root);
    let (mut linked, mut needs_review) = (Value::Null, Value::Null);
    let f = |k: &str| st[k] == json!(true);
    let auto = c["autoLink"] == json!(true) && ((f("linkedElsewhere") && f("linkedToOldStore")) || (!f("linked") && !f("linkedElsewhere") && (f("hasLocalMemory") || f("hasRemoteMemory"))));
    if auto {
        // Automatic linking leaves the project repo alone and never overwrites different memory
        needs_review = auto_link_block(&st);
        if needs_review.is_null() {
            linked = link_project(&t, workspace, &root, &LinkOpts { login: c.get("login").and_then(Value::as_str).map(str::to_string), scripts: Some(memory_scripts()), migrate: true, ..Default::default() })?;
        }
    } else if f("linked") {
        // Scripts follow the app; another machine may have written an older version into the shared store
        ensure_memory_scripts(sv(&st, "target"), &memory_scripts())?;
        ensure_local_exclude(&t, workspace)?;
        needs_review = auto_link_block(&st);
    }
    Ok(json!({ "pulled": pulled, "linked": linked, "needsReview": needs_review }))
}

fn set_config(core: &Core, patch: &Value, workspace: &str) -> R<Value> {
    let mut c = load_config(core);
    if let Some(b) = patch.get("consent").and_then(Value::as_bool) {
        c["consentAt"] = if b { json!(now_ms() as u64) } else { Value::Null };
    }
    if let Some(b) = patch.get("backend").filter(|v| !v.is_null()) {
        let name = b.as_str().unwrap_or("");
        if !BACKENDS.contains(&name) {
            return err("bad-backend", name);
        }
        // Projects already linked move over the next time they open
        c["backend"] = json!(name);
    }
    if let Some(r) = patch.get("repo").and_then(Value::as_str) {
        let r = r.trim();
        if !r.is_empty() && !repo_name_re().is_match(r) {
            return err("bad-repo-name", r);
        }
        c["repo"] = json!(if r.is_empty() { DEFAULT_REPO_NAME } else { r });
    }
    for k in ["autoSync", "autoLink"] {
        if let Some(b) = patch.get(k).and_then(Value::as_bool) {
            c[k] = json!(b);
        }
    }
    save_config(core, &c)?;
    Ok(state(core, workspace))
}

fn dispatch(core: &Arc<Core>, window: u32, channel: &str, args: &[Value]) -> R<Value> {
    let ws = core.workspace_for(Some(window));
    let lock = &core.features().memsync.lock;
    match channel {
        "memsync:state" => Ok(state(core, &ws)),
        "memsync:pluginState" => plugin_state(core),
        // Only from the consent dialog. One agent failing does not affect another.
        "memsync:applyPlugin" => {
            let dir = marketplace_dir(core);
            prepare_marketplace(&bundled_plugin_dir(core), &dir)?;
            let wanted: Vec<&str> = args.first().and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
            let homes = default_homes();
            let mut results = Vec::new();
            for agent in PLUGIN_AGENTS.iter().filter(|a| wanted.contains(a)) {
                let Some(cli) = core.find_agent_cli(agent, true) else {
                    results.push(json!({ "agent": agent, "ok": false, "log": [], "error": format!("{agent} CLI not found") }));
                    continue;
                };
                let exec = |a: &[String]| -> RunResult {
                    let spec = RunSpec { command: cli.clone(), args: a.to_vec(), cwd: p(&home()), stdin: String::new(), timeout_ms: 180_000, max_bytes: 8 * 1024 * 1024, env: None };
                    let r = run_process(&spec, &CancelToken::default());
                    RunResult { code: r.code.unwrap_or(1), stdout: r.stdout, stderr: if r.stderr.is_empty() { r.spawn_error.unwrap_or_default() } else { r.stderr } }
                };
                results.push(match apply_bundled_plugin(agent, &exec, &dir, &homes) {
                    Ok(log) => json!({ "agent": agent, "ok": true, "log": log }),
                    Err(e) => json!({ "agent": agent, "ok": false, "log": [], "error": e.message }),
                });
            }
            let mut c = load_config(core);
            c["pluginDeclined"] = Value::Null;
            save_config(core, &c)?;
            Ok(json!({ "results": results, "state": plugin_state(core)? }))
        }
        "memsync:declinePlugin" => {
            let mut c = load_config(core);
            c["pluginDeclined"] = json!(plugin_version(&bundled_plugin_dir(core))?);
            save_config(core, &c)?;
            plugin_state(core)
        }
        "memsync:setConfig" => set_config(core, args.first().unwrap_or(&Value::Null), &ws),
        // A folder under a detected OneDrive or Google Drive only
        "memsync:pickFolder" => {
            let mut c = load_config(core);
            require_consent(&c)?;
            let provider = sv(&c, "backend").to_string();
            if provider != "onedrive" && provider != "gdrive" {
                return err("bad-backend", provider);
            }
            let roots: Vec<String> = detect_cloud_roots()[&provider].as_array().unwrap().iter().filter_map(Value::as_str).map(str::to_string).collect();
            if roots.is_empty() {
                return err(&format!("{provider}-missing"), format!("{provider}-missing"));
            }
            let suggested = p(&Path::new(&roots[0]).join(DEFAULT_REPO_NAME));
            let start = c.get("folderPath").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| if exists(&suggested) { suggested } else { roots[0].clone() });
            let Some(picked) = core.host.pick_folder(window, &start) else { return Ok(json!({ "picked": false, "state": state(core, &ws) })) };
            ensure_folder(&picked, &provider, &roots)?;
            c["folderPath"] = json!(picked);
            save_config(core, &c)?;
            Ok(json!({ "picked": true, "state": state(core, &ws) }))
        }
        "memsync:setupRepo" => {
            let mut c = load_config(core);
            require_github(&c)?;
            let t = tools()?;
            let r = {
                let _g = lock.lock().unwrap();
                ensure_repo(&t, sv(&c, "repo"), sv(&c, "clonePath"))?
            };
            c["repoFull"] = r["repo"].clone();
            c["login"] = gh_status(&t).get("login").cloned().unwrap_or(Value::Null);
            save_config(core, &c)?;
            Ok(json!({ "result": r, "state": state(core, &ws) }))
        }
        "memsync:createRepoViaGit" => {
            let mut c = load_config(core);
            require_github(&c)?;
            let t = tools()?;
            let fallback = sv(&c, "repo").rsplit('/').next().unwrap_or("").to_string();
            let name = Some(arg_str(args, 0)).filter(|n| !n.is_empty()).unwrap_or(&fallback);
            let r = {
                let _g = lock.lock().unwrap();
                create_repo_via_git_login(&t, name, sv(&c, "clonePath"))?
            };
            let (owner, repo) = sv(&r, "repo").split_once('/').unwrap_or(("", ""));
            c["repo"] = json!(repo);
            c["login"] = json!(owner);
            c["repoFull"] = r["repo"].clone();
            save_config(core, &c)?;
            Ok(json!({ "result": r, "state": state(core, &ws) }))
        }
        "memsync:connectRepo" => {
            let mut c = load_config(core);
            require_github(&c)?;
            let t = tools()?;
            let r = {
                let _g = lock.lock().unwrap();
                connect_repo_by_url(&t, arg_str(args, 0), sv(&c, "clonePath"))?
            };
            c["repo"] = r["repo"].clone();
            c["repoFull"] = r["repo"].clone();
            c["login"] = json!(sv(&r, "repo").split('/').next().unwrap_or(""));
            save_config(core, &c)?;
            Ok(json!({ "result": r, "state": state(core, &ws) }))
        }
        // The user confirmed in settings: a "stop tracking" commit is allowed here
        "memsync:linkProject" => {
            let c = load_config(core);
            let root = require_root(&c)?;
            let t = tools()?;
            if !is_git_repo(&t, &ws) {
                return err("not-a-git-repo", ws);
            }
            let prefer = args.first().and_then(Value::as_str).filter(|x| *x == "shared" || *x == "local").map(str::to_string);
            let opts = LinkOpts { login: c.get("login").and_then(Value::as_str).map(str::to_string), scripts: Some(memory_scripts()), migrate: true, allow_commit: true, prefer };
            let r = {
                let _g = lock.lock().unwrap();
                link_project(&t, &ws, &root, &opts)?
            };
            Ok(json!({ "result": r, "state": state(core, &ws) }))
        }
        "memsync:sync" => {
            let c = load_config(core);
            let root = require_root(&c)?;
            let t = tools()?;
            let target = is_git_repo(&t, &ws).then_some(ws.as_str());
            let r = {
                let _g = lock.lock().unwrap();
                sync_now(&t, &root, target, args.first() == Some(&json!(true)), c.get("login").and_then(Value::as_str))?
            };
            Ok(json!({ "result": r, "state": state(core, &ws) }))
        }
        "memsync:onProjectOpen" => on_project_open(core, &ws),
        // Only the detected official program, never a path from the page
        "memsync:launchCloudApp" => {
            let provider = arg_str(args, 0);
            if provider != "onedrive" && provider != "gdrive" {
                return err("bad-backend", provider);
            }
            let apps = detect_cloud_apps();
            let Some(exe) = apps[provider].get("exe").and_then(Value::as_str) else { return err(&format!("{provider}-missing"), format!("{provider}-missing")) };
            core.host.open_path(exe).map_err(|e| MemErr { code: "launch-failed".into(), message: e })?;
            Ok(json!(true))
        }
        _ => err("internal", format!("No handler registered for '{channel}'")),
    }
}

pub fn call(core: &Arc<Core>, window: u32, channel: &str, args: &[Value]) -> Result<Value, String> {
    if channel == "memsync:openClone" {
        if let Some(root) = memory_root(&load_config(core)).filter(|r| exists(r)) {
            let _ = core.host.open_path(&root);
        }
        return Ok(Value::Null);
    }
    Ok(match dispatch(core, window, channel, args) {
        Ok(data) => json!({ "ok": true, "data": data }),
        Err(e) => json!({ "ok": false, "code": e.code, "message": e.message }),
    })
}

/// GitHub mode only: a cloud folder has its own client
pub fn start_background_sync(core: &Arc<Core>) {
    let weak = Arc::downgrade(core);
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(SYNC_INTERVAL_SECS));
        let Some(core) = weak.upgrade() else { return };
        let c = load_config(&core);
        let Some(root) = active_root(&c) else { continue };
        if sv(&c, "backend") != "github" || c["autoSync"] != json!(true) {
            continue;
        }
        let Ok(t) = tools() else { continue };
        let _g = core.features().memsync.lock.lock().unwrap();
        match sync_now(&t, &root, None, false, c.get("login").and_then(Value::as_str)) {
            Ok(r) if r.get("error").is_some() => eprintln!("[memsync] background sync: {}", r["error"]),
            Err(e) => eprintln!("[memsync] background sync failed: {}", e.message),
            _ => {}
        }
    });
}

/// wb-ipc's test entry: the flows of check-memsync.mts with its own Tools
pub fn test_call(name: &str, t: &Tools, a: &Map<String, Value>) -> Result<Value, MemErr> {
    let s = |k: &str| a.get(k).and_then(Value::as_str).unwrap_or("");
    let homes = || match a.get("homes") {
        Some(h) => Homes { claude: PathBuf::from(sv(h, "claude")), codex: PathBuf::from(sv(h, "codex")), antigravity: PathBuf::from(sv(h, "antigravity")) },
        None => default_homes(),
    };
    match name {
        "ensureRepo" => ensure_repo(t, s("name"), s("clonePath")),
        "connectRepoByUrl" => connect_repo_by_url(t, s("input"), s("clonePath")),
        "createRepoViaGitLogin" => create_repo_via_git_login(t, s("name"), s("clonePath")),
        "ghStatus" => Ok(gh_status(t)),
        "projectState" => Ok(project_state(t, s("workspace"), s("clonePath"))),
        "cloneState" => Ok(clone_state(t, s("clonePath"))),
        "syncNow" => sync_now(t, s("clonePath"), a.get("workspace").and_then(Value::as_str), a.get("pullOnly") == Some(&json!(true)), a.get("login").and_then(Value::as_str)),
        "linkProject" => {
            let scripts = a.get("scripts").and_then(Value::as_object).map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect());
            let flag = |k: &str| a.get(k) == Some(&json!(true));
            link_project(t, s("workspace"), s("clonePath"), &LinkOpts { login: a.get("login").and_then(Value::as_str).map(str::to_string), scripts, migrate: flag("migrate"), allow_commit: flag("allowCommit"), prefer: a.get("prefer").and_then(Value::as_str).map(str::to_string) })
        }
        "ensureFolder" => {
            let roots: Vec<String> = a.get("cloudRoots").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            ensure_folder(s("root"), s("provider"), &roots)
        }
        "installedPlugins" => Ok(json!(installed_plugins(s("agent"), &homes()))),
        "pluginVersion" => plugin_version(Path::new(s("dir"))).map(|v| json!(v)),
        "prepareMarketplace" => prepare_marketplace(Path::new(s("bundled")), Path::new(s("dir"))).map(|_| Value::Null),
        "parseRepoInput" => Ok(json!(parse_repo_input(s("input")))),
        "pluginAction" => Ok(json!(plugin_action(a.get("installed").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]), s("version")))),
        "autoLinkBlock" => Ok(auto_link_block(a.get("state").unwrap_or(&Value::Null))),
        _ => err("internal", format!("unknown test call {name}")),
    }
}
