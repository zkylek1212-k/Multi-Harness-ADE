//! How to start a command. Same rules as `launchPlan` and `resolveCheckCommand` in the TS code.
use std::path::{Component, Path, PathBuf};
use std::process::Command;

/// cmd.exe expands %VAR% even inside quotes and quotes cannot be split reliably: refuse them.
const CMD_UNSAFE: &[char] = &['"', '%', '\r', '\n'];

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LaunchPlan {
    pub file: String,
    pub args: Vec<String>,
    /// The args are one pre-quoted line for cmd.exe, not separate arguments.
    pub verbatim: bool,
}

fn lower_ext(command: &str) -> String {
    Path::new(command).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// npm shims only forward to a real binary or Node entry point. Going around cmd.exe keeps JSON and TOML arguments intact.
fn shim_target(command: &str) -> Option<String> {
    let text = std::fs::read_to_string(command).ok()?;
    let mut found = None;
    for line in text.lines() {
        let line = line.trim_end();
        if !line.ends_with("%*") {
            continue;
        }
        let Some(start) = line.find("\"%dp0%") else { continue };
        let rest = &line[start + 6..];
        let Some(end) = rest.find('"') else { continue };
        let rel = rest[..end].trim_start_matches(['\\', '/']);
        let normal = rel.replace('\\', "/").to_ascii_lowercase();
        if normal.starts_with("node_modules/") && (normal.ends_with(".exe") || normal.ends_with(".js") || normal.ends_with(".cjs") || normal.ends_with(".mjs")) {
            found = Some(rel.to_string());
        }
    }
    found
}

pub fn launch_plan(command: &str, args: &[String], windows: bool) -> Result<LaunchPlan, String> {
    let ext = lower_ext(command);
    if windows && (ext == "cmd" || ext == "bat") {
        if let Some(rel) = shim_target(command) {
            let dir = Path::new(command).parent().unwrap_or(Path::new(""));
            let entry = dir.join(rel.replace('\\', "/"));
            if entry.exists() {
                let entry = entry.to_string_lossy().into_owned();
                if lower_ext(&entry) == "exe" {
                    return Ok(LaunchPlan { file: entry, args: args.to_vec(), verbatim: false });
                }
                let node = dir.join("node.exe");
                let file = if node.exists() { node.to_string_lossy().into_owned() } else { "node.exe".to_string() };
                let mut all = vec![entry];
                all.extend_from_slice(args);
                return Ok(LaunchPlan { file, args: all, verbatim: false });
            }
        }
        let mut all = vec![command.to_string()];
        all.extend_from_slice(args);
        if let Some(bad) = all.iter().find(|a| a.contains(CMD_UNSAFE)) {
            let cut: String = bad.chars().take(80).collect();
            return Err(format!("refusing to pass an argument containing quotes, % or newlines to cmd.exe: {cut}"));
        }
        let line = all.iter().map(|a| format!("\"{a}\"")).collect::<Vec<_>>().join(" ");
        let file = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into());
        return Ok(LaunchPlan { file, args: vec!["/d".into(), "/s".into(), "/c".into(), format!("\"{line}\"")], verbatim: true });
    }
    if windows && ext == "ps1" {
        let mut all: Vec<String> = ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"].iter().map(|s| s.to_string()).collect();
        all.push(command.to_string());
        all.extend_from_slice(args);
        return Ok(LaunchPlan { file: "powershell.exe".into(), args: all, verbatim: false });
    }
    Ok(LaunchPlan { file: command.to_string(), args: args.to_vec(), verbatim: false })
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum Resolved {
    Path(String),
    Error(String),
}

fn escapes(worktree: &Path, name: &str) -> bool {
    let mut depth = 0i32;
    for c in Path::new(name).components() {
        match c {
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return true;
                }
            }
            Component::Normal(_) => depth += 1,
            _ => {}
        }
    }
    let _ = worktree;
    false
}

/// Resolve a command to a full path.
/// `where` searches the current directory first, so it never runs from the worktree: a planted npm.cmd must not replace the real one.
pub fn resolve_command(cmd: &str, worktree: &Path) -> Resolved {
    let name = cmd.trim();
    if name.is_empty() {
        return Resolved::Error("empty command".into());
    }
    let p = Path::new(name);
    if p.is_absolute() {
        return if p.exists() { Resolved::Path(name.into()) } else { Resolved::Error(format!("not found: {name}")) };
    }
    if name.contains(['/', '\\']) {
        if escapes(worktree, name) {
            return Resolved::Error(format!("path leaves the worktree: {name}"));
        }
        let full: PathBuf = worktree.join(name);
        return Resolved::Path(full.to_string_lossy().into_owned());
    }
    let windows = cfg!(windows);
    let finder = if windows {
        PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())).join("System32").join("where.exe")
    } else {
        PathBuf::from("which")
    };
    let mut c = Command::new(finder);
    if windows {
        c.arg(name);
    } else {
        c.args(["-a", name]);
    }
    let found: Vec<String> = c
        .current_dir(std::env::temp_dir())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default();
    let pick = if windows {
        // `where` also lists the extensionless sh script (npm) next to npm.cmd: only the latter starts on Windows.
        found.into_iter().find(|f| matches!(lower_ext(f).as_str(), "exe" | "cmd" | "bat" | "com" | "ps1"))
    } else {
        found.into_iter().next()
    };
    match pick {
        Some(f) => Resolved::Path(f),
        None => Resolved::Error(format!("command not found: {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_commands_pass_through() {
        let p = launch_plan("git", &["status".into()], true).unwrap();
        assert_eq!(p, LaunchPlan { file: "git".into(), args: vec!["status".into()], verbatim: false });
    }

    #[test]
    fn ps1_goes_through_powershell() {
        let p = launch_plan("C:/x/run.PS1", &["a".into()], true).unwrap();
        assert_eq!(p.file, "powershell.exe");
        assert_eq!(p.args.last().unwrap(), "a");
        assert!(p.args.contains(&"C:/x/run.PS1".to_string()));
    }

    #[test]
    fn cmd_refuses_unsafe_arguments() {
        for bad in ["a\"b", "100%", "x\ny"] {
            let e = launch_plan("C:/missing/tool.cmd", &[bad.to_string()], true).unwrap_err();
            assert!(e.contains("refusing"), "{e}");
        }
    }

    #[test]
    fn cmd_without_a_shim_is_quoted_for_cmd_exe() {
        let p = launch_plan("C:/missing/tool.bat", &["a b".into()], true).unwrap();
        assert!(p.verbatim);
        assert_eq!(p.args[3], "\"\"C:/missing/tool.bat\" \"a b\"\"");
    }

    #[test]
    fn npm_shim_goes_around_cmd_exe() {
        let dir = std::env::temp_dir().join(format!("wb-shim-{}", std::process::id()));
        let target = dir.join("node_modules/pkg/cli.js");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "").unwrap();
        let shim = dir.join("tool.cmd");
        std::fs::write(&shim, "@ECHO off\r\n\"%dp0%\\node_modules\\pkg\\cli.js\" %*\r\n").unwrap();
        let p = launch_plan(shim.to_str().unwrap(), &["{\"json\":\"%x\"}".into()], true).unwrap();
        assert!(!p.verbatim);
        assert!(p.args[0].replace('\\', "/").ends_with("node_modules/pkg/cli.js"), "{:?}", p.args);
        assert_eq!(p.args[1], "{\"json\":\"%x\"}", "arguments arrive unchanged");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn resolve_rules() {
        let wt = std::env::temp_dir().join(format!("wb-res-{}", std::process::id()));
        std::fs::create_dir_all(&wt).unwrap();
        assert!(matches!(resolve_command("", &wt), Resolved::Error(_)));
        assert!(matches!(resolve_command("../outside.exe", &wt), Resolved::Error(_)));
        assert!(matches!(resolve_command("definitely-not-a-command-xyz", &wt), Resolved::Error(_)));
        assert!(matches!(resolve_command("./scripts/x", &wt), Resolved::Path(_)));
        // A planted npm.cmd in the worktree does not win.
        std::fs::write(wt.join("npm.cmd"), "@echo HIJACKED\r\n").unwrap();
        match resolve_command("npm", &wt) {
            Resolved::Path(p) => {
                assert_ne!(Path::new(&p).parent().unwrap(), wt.as_path(), "hijacked: {p}");
                if cfg!(windows) {
                    assert!(matches!(lower_ext(&p).as_str(), "cmd" | "exe"), "{p}");
                }
            }
            Resolved::Error(e) => panic!("{e}"),
        }
        let _ = std::fs::remove_dir_all(wt);
    }
}
