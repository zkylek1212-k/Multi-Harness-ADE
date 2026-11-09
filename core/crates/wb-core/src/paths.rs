// Where each agent keeps its config and how to find its CLI. Paths probed on real installs (2026-09).
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::core::Core;
use crate::util::{exists, home, p};

pub struct AgentPaths {
    pub id: &'static str,
    pub label: &'static str,
    pub cli: &'static str,
    pub config_home: PathBuf,
    pub skills_dir: Option<PathBuf>,
    pub plugins_dir: PathBuf,
    pub mcp_config: PathBuf,
    pub trust_settings: Option<PathBuf>,
}

pub const AGENTS: [&str; 3] = ["claude", "antigravity", "codex"];

pub fn agent_paths(id: &str) -> AgentPaths {
    let h = home();
    match id {
        "claude" => AgentPaths {
            id: "claude",
            label: "Claude Code",
            cli: "claude",
            config_home: h.join(".claude"),
            skills_dir: Some(h.join(".claude").join("skills")),
            plugins_dir: h.join(".claude").join("plugins"),
            mcp_config: h.join(".claude.json"),
            trust_settings: None,
        },
        "antigravity" => AgentPaths {
            id: "antigravity",
            label: "Antigravity",
            cli: "agy", // the executable is agy, not antigravity
            config_home: h.join(".gemini").join("config"),
            skills_dir: None,
            plugins_dir: h.join(".gemini").join("config").join("plugins"),
            mcp_config: h.join(".gemini").join("config").join("mcp_config.json"),
            trust_settings: Some(h.join(".gemini").join("antigravity-cli").join("settings.json")),
        },
        _ => AgentPaths {
            id: "codex",
            label: "Codex",
            cli: "codex",
            config_home: h.join(".codex"),
            skills_dir: Some(h.join(".codex").join("skills")),
            plugins_dir: h.join(".codex").join("plugins"),
            mcp_config: h.join(".codex").join("config.toml"),
            trust_settings: None,
        },
    }
}

/// A command with no console window
pub fn hidden(program: &str) -> Command {
    let mut c = Command::new(program);
    c.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c
}

/// Prefer something CreateProcess can run
fn pick_finder_result(out: &str) -> Option<String> {
    let lines: Vec<&str> = out.lines().map(str::trim).filter(|l| !l.is_empty() && exists(l)).collect();
    if lines.is_empty() {
        return None;
    }
    if !cfg!(windows) {
        return Some(lines[0].to_string());
    }
    for ext in [".exe", ".cmd", ".bat", ".ps1"] {
        if let Some(hit) = lines.iter().find(|l| l.to_lowercase().ends_with(ext)) {
            return Some(hit.to_string());
        }
    }
    for l in &lines {
        for ext in [".cmd", ".exe", ".bat", ".ps1"] {
            let c = format!("{l}{ext}");
            if exists(&c) {
                return Some(c);
            }
        }
    }
    Some(lines[0].to_string())
}

/// Standard install locations, for when this process's PATH predates the install
fn candidate_paths(cmd: &str) -> Vec<PathBuf> {
    let h = home();
    if !cfg!(windows) {
        return vec![h.join(".local").join("bin").join(cmd), h.join(".npm-global").join("bin").join(cmd), Path::new("/usr/local/bin").join(cmd), Path::new("/opt/homebrew/bin").join(cmd)];
    }
    let var = |k: &str, d: PathBuf| std::env::var_os(k).map(PathBuf::from).unwrap_or(d);
    let local = var("LOCALAPPDATA", h.join("AppData").join("Local"));
    let roaming = var("APPDATA", h.join("AppData").join("Roaming"));
    let f = |dir: &Path, ext: &str| dir.join(format!("{cmd}.{ext}"));
    let npm = roaming.join("npm");
    let pnpm = local.join("pnpm");
    let scoop = h.join("scoop").join("shims");
    let dotlocal = h.join(".local").join("bin");
    let mut out = Vec::new();
    if cmd == "git" || cmd == "gh" {
        let pf = var("ProgramFiles", PathBuf::from("C:\\Program Files"));
        if cmd == "git" {
            out.extend([pf.join("Git").join("cmd").join("git.exe"), local.join("Programs").join("Git").join("cmd").join("git.exe")]);
        } else {
            out.extend([pf.join("GitHub CLI").join("gh.exe"), local.join("Programs").join("GitHub CLI").join("gh.exe")]);
        }
    }
    if cmd == "codex" {
        out.extend([local.join("Programs").join("OpenAI").join("Codex").join("bin").join("codex.exe"), h.join(".codex").join("packages").join("standalone").join("current").join("bin").join("codex.exe"), npm.join("codex.cmd"), npm.join("codex.ps1"), pnpm.join("codex.cmd")]);
    }
    if cmd == "claude" {
        out.extend([dotlocal.join("claude.exe"), npm.join("claude.cmd"), npm.join("claude.ps1"), local.join("Programs").join("Claude").join("claude.exe")]);
    }
    if cmd == "agy" || cmd == "antigravity" {
        out.extend([local.join("agy").join("bin").join("agy.exe"), npm.join("agy.cmd"), h.join(".gemini").join("antigravity-cli").join("bin").join("agy.exe")]);
    }
    if let Some(nvm) = std::env::var_os("NVM_SYMLINK").map(PathBuf::from) {
        out.extend([f(&nvm, "cmd"), f(&nvm, "exe")]);
    }
    out.extend([
        f(&npm, "cmd"),
        f(&npm, "exe"),
        f(&npm, "ps1"),
        f(&pnpm, "cmd"),
        f(&pnpm, "exe"),
        f(&pnpm, "ps1"),
        f(&scoop, "exe"),
        f(&scoop, "cmd"),
        f(&scoop, "ps1"),
        f(&local.join("Yarn").join("bin"), "cmd"),
        f(&local.join("Microsoft").join("WinGet").join("Links"), "exe"),
        local.join(cmd).join("bin").join(format!("{cmd}.exe")),
        local.join("Programs").join(cmd).join(format!("{cmd}.exe")),
        f(&dotlocal, "exe"),
        f(&dotlocal, "cmd"),
        f(Path::new("C:\\Program Files\\nodejs"), "cmd"),
        f(Path::new("C:\\Program Files\\nodejs"), "exe"),
        f(Path::new("C:\\Program Files (x86)\\nodejs"), "cmd"),
        f(Path::new("C:\\Program Files (x86)\\nodejs"), "exe"),
    ]);
    out
}

const FOUND_TTL: Duration = Duration::from_secs(300);

fn found_cache() -> &'static Mutex<HashMap<String, (String, Instant)>> {
    static C: OnceLock<Mutex<HashMap<String, (String, Instant)>>> = OnceLock::new();
    C.get_or_init(Mutex::default)
}

/// PATH, then the standard locations. `fresh` skips the 5-minute memory; only "found" is remembered,
/// so a just-installed CLI is never hidden by an old miss.
pub fn find_cli(cmd: &str, fresh: bool) -> Option<String> {
    if !fresh {
        if let Some((path, at)) = found_cache().lock().unwrap().get(cmd) {
            if at.elapsed() < FOUND_TTL && exists(path) {
                return Some(path.clone());
            }
        }
    }
    let finder = if cfg!(windows) { "where" } else { "which" };
    let mut found = hidden(finder).arg(cmd).stderr(Stdio::null()).output().ok().and_then(|o| pick_finder_result(&String::from_utf8_lossy(&o.stdout)));
    if found.is_none() {
        found = candidate_paths(cmd).into_iter().find(|c| c.exists()).map(|c| p(&c));
    }
    let mut cache = found_cache().lock().unwrap();
    match &found {
        Some(f) => {
            cache.insert(cmd.to_string(), (f.clone(), Instant::now()));
        }
        None => {
            cache.remove(cmd);
        }
    }
    found
}

impl Core {
    pub fn find_agent_cli(&self, agent: &str, fresh: bool) -> Option<String> {
        if let Some(custom) = self.custom_cli_path(agent).filter(|c| exists(c)) {
            return Some(custom);
        }
        find_cli(agent_paths(agent).cli, fresh)
    }
}
