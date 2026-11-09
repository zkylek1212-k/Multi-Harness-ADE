//! Port of `src/main/cowork/git.ts`. Same behaviour, checked by running the same test suites against both.
//! Keeps calling the git CLI: behaviour stays identical and the harness's execution-point rules still apply.
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use sha2::{Digest, Sha256};

pub type Result<T> = std::result::Result<T, String>;

fn hide(c: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    let _ = c;
}

fn base(cwd: &Path) -> Command {
    let mut c = Command::new("git");
    // fsmonitor can be set by repo config: the harness's own git commands always turn it off.
    c.args(["-c", "core.fsmonitor=false"]).current_dir(cwd).stdin(Stdio::null());
    hide(&mut c);
    c
}

pub fn git_bytes(cwd: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = base(cwd).args(args).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if stderr.is_empty() { format!("git {} exited with {:?}", args.first().unwrap_or(&""), out.status.code()) } else { stderr })
    }
}

pub fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8_lossy(&git_bytes(cwd, args)?).into_owned())
}

fn sha256(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn resolve(dir: &Path, p: &str) -> PathBuf {
    normalize(&dir.join(p))
}

/// Lexical normalisation, like path.resolve: never touches the disk.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn real(p: &Path) -> Result<PathBuf> {
    let r = fs::canonicalize(p).map_err(|e| e.to_string())?;
    // canonicalize returns \\?\ paths on Windows; the rest of the app and git use plain ones.
    let s = r.to_string_lossy();
    Ok(PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()))
}

fn is_under(base: &Path, p: &Path) -> bool {
    let (b, q) = (lower(base), lower(p));
    q != b && q.starts_with(&format!("{b}/"))
}

fn lower(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.trim_end_matches('/');
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s.to_string()
    }
}

// ── baseline ─────────────────────────────────────────────────────────

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Baseline {
    pub root: String,
    pub common_dir: String,
    pub branch: String,
    pub head: String,
    pub dirty: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn read_baseline(dir: &Path) -> Result<Baseline> {
    let root = git(dir, &["rev-parse", "--show-toplevel"]).map_err(|_| "not-a-git-repo".to_string())?.trim().to_string();
    let rp = Path::new(&root);
    let head = git(rp, &["rev-parse", "--verify", "HEAD"]).map_err(|_| "no-commits".to_string())?.trim().to_string();
    let common = resolve(rp, git(rp, &["rev-parse", "--git-common-dir"])?.trim());
    let git_dir = resolve(rp, git(rp, &["rev-parse", "--git-dir"])?.trim());
    let branch = git(rp, &["rev-parse", "--abbrev-ref", "HEAD"])?.trim().to_string();
    let status = git(rp, &["status", "--porcelain=v1", "-z", "--untracked-files=all"])?;
    let parts: Vec<&str> = status.split('\0').collect();
    let mut dirty = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        let e = parts[i];
        if e.len() >= 4 {
            dirty.push(e[3..].to_string());
            // rename/copy carries the original path as one more field
            let kind = e.as_bytes()[0];
            if kind == b'R' || kind == b'C' {
                i += 1;
            }
        }
        i += 1;
    }
    let mut warnings = Vec::new();
    for (file, label) in [("MERGE_HEAD", "merge-in-progress"), ("rebase-merge", "rebase-in-progress"), ("rebase-apply", "rebase-in-progress"), ("CHERRY_PICK_HEAD", "cherry-pick-in-progress")] {
        if git_dir.join(file).exists() && !warnings.iter().any(|w| w == label) {
            warnings.push(label.to_string());
        }
    }
    Ok(Baseline { root: normalize(rp).to_string_lossy().replace('/', std::path::MAIN_SEPARATOR_STR), common_dir: common.to_string_lossy().into_owned(), branch, head, dirty, warnings })
}

// ── snapshots ────────────────────────────────────────────────────────

fn remove_tree(dir: &Path) -> Result<()> {
    if fs::remove_dir_all(dir).is_ok() || !dir.exists() {
        return Ok(());
    }
    // git marks some files read-only on Windows; clear the flag and retry
    fn clear(p: &Path) {
        if let Ok(rd) = fs::read_dir(p) {
            for e in rd.flatten() {
                let path = e.path();
                let link = fs::symlink_metadata(&path).map(|m| m.file_type().is_symlink()).unwrap_or(true);
                if path.is_dir() && !link {
                    clear(&path);
                } else if let Ok(m) = fs::metadata(&path) {
                    let mut perm = m.permissions();
                    #[allow(clippy::permissions_set_readonly_false)]
                    perm.set_readonly(false);
                    let _ = fs::set_permissions(&path, perm);
                }
            }
        }
    }
    clear(dir);
    fs::remove_dir_all(dir).map_err(|e| e.to_string())
}

pub fn create_snapshot(root: &Path, dir: &Path, commit: &str) -> Result<()> {
    if dir.exists() {
        remove_tree(dir)?;
    }
    let parent = dir.parent().ok_or("no parent")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    git(parent, &["clone", "--quiet", "--shared", "--no-checkout", "--", &root.to_string_lossy(), &dir.to_string_lossy()])?;
    git(dir, &["-c", "advice.detachedHead=false", "checkout", "--quiet", "--detach", commit])?;
    Ok(())
}

pub fn snapshot_changes(dir: &Path, commit: &str) -> Result<Vec<String>> {
    let mut changes = Vec::new();
    let head = git(dir, &["rev-parse", "HEAD"])?.trim().to_string();
    if head != commit {
        changes.push(format!("HEAD moved: {} -> {}", &commit[..commit.len().min(12)], &head[..head.len().min(12)]));
    }
    for line in git(dir, &["status", "--porcelain=v1", "--untracked-files=all", "--ignored"])?.lines() {
        if !line.trim().is_empty() {
            changes.push(line.to_string());
        }
    }
    Ok(changes)
}

// ── worktrees ────────────────────────────────────────────────────────

pub fn add_worktree(repo_root: &Path, dir: &Path, branch: &str, base_commit: &str) -> Result<()> {
    fs::create_dir_all(dir.parent().ok_or("no parent")?).map_err(|e| e.to_string())?;
    git(repo_root, &["worktree", "add", "-b", branch, &dir.to_string_lossy(), base_commit]).map(|_| ())
}

#[cfg(windows)]
fn junction(src: &Path, dst: &Path) -> Result<()> {
    // std has no junction API; mklink is what every Windows install has.
    let mut c = Command::new("cmd");
    c.args(["/d", "/c", "mklink", "/J"]).arg(dst).arg(src).stdout(Stdio::null()).stderr(Stdio::null());
    hide(&mut c);
    let status = c.status().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("could not create a junction at {}", dst.display()))
    }
}

#[cfg(not(windows))]
fn junction(src: &Path, dst: &Path) -> Result<()> {
    std::os::unix::fs::symlink(src, dst).map_err(|e| e.to_string())
}

pub fn link_node_modules(repo_root: &Path, worktree: &Path) -> Result<bool> {
    let src = repo_root.join("node_modules");
    let dst = worktree.join("node_modules");
    if !src.exists() || dst.exists() {
        return Ok(false);
    }
    if git(worktree, &["check-ignore", "-q", "node_modules"]).is_err() {
        return Ok(false); // not ignored: git add -A would pull the link into the commit
    }
    junction(&src, &dst)?;
    Ok(true)
}

fn identity(dir: &Path) -> Vec<String> {
    let has = |k: &str| git(dir, &["config", k]).map(|v| !v.trim().is_empty()).unwrap_or(false);
    if has("user.name") && has("user.email") {
        return vec![];
    }
    ["-c", "user.name=Agent Workbench Cowork", "-c", "user.email=cowork@agent-workbench.invalid"].iter().map(|s| s.to_string()).collect()
}

fn hook_args(hooks_dir: Option<&str>) -> Vec<String> {
    hooks_dir.map(|d| vec!["-c".to_string(), format!("core.hooksPath={}", d.replace('\\', "/"))]).unwrap_or_default()
}

fn strs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

pub fn commit_all(dir: &Path, message: &str, hooks_dir: Option<&str>) -> Result<Option<String>> {
    git(dir, &["add", "-A"])?;
    if git(dir, &["diff", "--cached", "--quiet"]).is_ok() {
        return Ok(None);
    }
    let mut args = hook_args(hooks_dir);
    args.extend(identity(dir));
    args.extend(["commit", "-q", "-m", message].iter().map(|s| s.to_string()));
    git(dir, &strs(&args))?;
    Ok(Some(git(dir, &["rev-parse", "HEAD"])?.trim().to_string()))
}

fn split_nul(s: &str) -> Vec<String> {
    s.split('\0').filter(|x| !x.is_empty()).map(str::to_string).collect()
}

pub fn changed_files(dir: &Path) -> Result<Vec<String>> {
    git(dir, &["add", "-A"])?;
    Ok(split_nul(&git(dir, &["diff", "--cached", "--name-only", "--no-renames", "-z"])?))
}

pub fn revert_files(dir: &Path, files: &[String]) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }
    let base = real(dir)?;
    for f in files {
        let abs = normalize(&base.join(f));
        if !is_under(&base, &abs) {
            return Err(format!("refusing to revert a path outside the worktree: {f}"));
        }
        let _ = git(dir, &["reset", "-q", "--", f]);
        if git(dir, &["cat-file", "-e", &format!("HEAD:{f}")]).is_ok() {
            git(dir, &["checkout", "HEAD", "--", f])?;
        } else {
            let _ = fs::remove_file(&abs);
        }
    }
    Ok(())
}

pub fn has_changes(dir: &Path) -> Result<bool> {
    Ok(!git(dir, &["status", "--porcelain", "--untracked-files=all"])?.trim().is_empty())
}

#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub enum PickResult {
    Ok { ok: bool },
    Conflict { ok: bool, commit: String, message: String },
}

fn cut(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

pub fn cherry_pick_all(dir: &Path, commits: &[String], hooks_dir: Option<&str>) -> Result<PickResult> {
    let id = identity(dir);
    for c in commits {
        if git(dir, &["merge-base", "--is-ancestor", c, "HEAD"]).is_ok() {
            continue; // already on this line
        }
        let mut args = hook_args(hooks_dir);
        args.extend(id.iter().cloned());
        args.extend(["cherry-pick", "--allow-empty", "--keep-redundant-commits", c].iter().map(|s| s.to_string()));
        if let Err(e) = git(dir, &strs(&args)) {
            let _ = git(dir, &["cherry-pick", "--abort"]);
            return Ok(PickResult::Conflict { ok: false, commit: c.clone(), message: cut(&e, 500) });
        }
    }
    Ok(PickResult::Ok { ok: true })
}

pub fn diff_stat(dir: &Path, base_commit: &str, head: &str) -> Result<String> {
    Ok(cut(git(dir, &["diff", "--no-ext-diff", "--no-textconv", "--stat", &format!("{base_commit}..{head}")])?.trim(), 8000))
}

#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub enum MergeResult {
    Ok { ok: bool, commit: String },
    Failed { ok: bool, reason: String, message: String },
}

pub fn merge_branch(repo_root: &Path, expect_branch: &str, commit: &str, message: &str, hooks_dir: Option<&str>) -> Result<MergeResult> {
    let current = git(repo_root, &["rev-parse", "--abbrev-ref", "HEAD"])?.trim().to_string();
    if current != expect_branch {
        return Ok(MergeResult::Failed { ok: false, reason: "wrong-branch".into(), message: current });
    }
    let dirty = git(repo_root, &["status", "--porcelain", "--untracked-files=no"])?;
    if !dirty.trim().is_empty() {
        return Ok(MergeResult::Failed { ok: false, reason: "dirty".into(), message: dirty.trim().lines().take(20).collect::<Vec<_>>().join("\n") });
    }
    let mut args = hook_args(hooks_dir);
    args.extend(identity(repo_root));
    args.extend(["merge", "--no-ff", "-m", message, commit].iter().map(|s| s.to_string()));
    if let Err(e) = git(repo_root, &strs(&args)) {
        let _ = git(repo_root, &["merge", "--abort"]);
        return Ok(MergeResult::Failed { ok: false, reason: "conflict".into(), message: cut(&e, 500) });
    }
    Ok(MergeResult::Ok { ok: true, commit: git(repo_root, &["rev-parse", "HEAD"])?.trim().to_string() })
}

fn is_link(p: &Path) -> bool {
    fs::symlink_metadata(p).map(|m| m.file_type().is_symlink() || is_junction(&m)).unwrap_or(false)
}

#[cfg(windows)]
fn is_junction(m: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    m.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT
}

#[cfg(not(windows))]
fn is_junction(_: &fs::Metadata) -> bool {
    false
}

/// Only ever touches paths under `worktree_base` (default <repo>/.cowork).
pub fn remove_worktrees(repo_root: &Path, root: &Path, dirs: &[String], worktree_base: Option<&str>) -> Result<()> {
    let base = worktree_base.map(|b| normalize(Path::new(b))).unwrap_or_else(|| normalize(&repo_root.join(".cowork")));
    for d in dirs {
        let dp = normalize(Path::new(d));
        if !is_under(&base, &dp) {
            continue;
        }
        // Remove the junction first, so worktree removal cannot walk into the main node_modules.
        let nm = dp.join("node_modules");
        if is_link(&nm) {
            if fs::remove_dir(&nm).is_err() {
                let _ = fs::remove_file(&nm);
            }
        }
        let _ = git(repo_root, &["worktree", "remove", "--force", d]);
    }
    let _ = git(repo_root, &["worktree", "prune"]);
    let r = normalize(root);
    if is_under(&base, &r) && r.exists() {
        remove_tree(&r)?;
    }
    let _ = fs::remove_dir(&base);
    Ok(())
}

// ── approvals, hook trust, git config guard ─────────────────────────

pub fn configured_hooks_path(repo_root: &Path) -> Option<String> {
    git(repo_root, &["config", "--get", "core.hooksPath"]).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn write_hook(dest: &Path, name: &str, data: &[u8]) -> Result<()> {
    let base = Path::new(name).file_name().ok_or("bad hook name")?;
    let file = dest.join(base);
    fs::write(&file, data).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&file, fs::Permissions::from_mode(0o755));
    }
    Ok(())
}

/// A baseline copy of the hooks. The harness runs only these: edited tracked hooks and hooks a branch brings in
/// do get executed by plain git (tested with Git 2.53), and turning hooks off would drop real protections.
pub fn export_baseline_hooks(repo_root: &Path, commit: &str, dest: &Path) -> Result<String> {
    if dest.exists() {
        remove_tree(dest)?;
    }
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let configured = configured_hooks_path(repo_root);
    if let Some(c) = configured.as_deref().filter(|c| !Path::new(c).is_absolute()) {
        let rel = c.replace('\\', "/");
        let rel = rel.trim_end_matches('/');
        let names = split_nul(&git(repo_root, &["ls-tree", "--name-only", "-z", commit, "--", &format!("{rel}/")])?);
        for n in &names {
            if let Ok(data) = git_bytes(repo_root, &["cat-file", "blob", &format!("{commit}:{n}")]) {
                write_hook(dest, n, &data)?;
            }
        }
        if !names.is_empty() {
            return Ok(dest.to_string_lossy().into_owned());
        }
    }
    let dir = match configured.as_deref().filter(|c| Path::new(c).is_absolute()) {
        Some(c) => PathBuf::from(c),
        None => resolve(repo_root, git(repo_root, &["rev-parse", "--git-path", "hooks"])?.trim()),
    };
    if let Ok(rd) = fs::read_dir(&dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.file_type().map(|t| t.is_file()).unwrap_or(false) && !name.ends_with(".sample") {
                write_hook(dest, &name, &fs::read(e.path()).map_err(|er| er.to_string())?)?;
            }
        }
    }
    Ok(dest.to_string_lossy().into_owned())
}

fn guard_key(p: &Path) -> String {
    let s = normalize(p).to_string_lossy().replace('\\', "/");
    s.to_lowercase()
}

pub fn git_guard_state(dir: &Path) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    let mut add = |p: &Path| {
        if let Ok(data) = fs::read(p) {
            out.insert(guard_key(p), sha256(&data));
        }
    };
    let top = normalize(Path::new(git(dir, &["rev-parse", "--show-toplevel"])?.trim()));
    if let Ok(list) = git(dir, &["config", "--local", "--list", "--show-origin", "--includes"]) {
        for line in list.lines() {
            if let Some(rest) = line.strip_prefix("file:") {
                if let Some((path, _)) = rest.split_once('\t') {
                    add(&resolve(dir, path));
                }
            }
        }
    }
    add(&resolve(dir, git(dir, &["rev-parse", "--git-path", "config"])?.trim()));
    let wt = resolve(dir, git(dir, &["rev-parse", "--git-path", "config.worktree"])?.trim());
    if wt.exists() {
        add(&wt);
    }
    let hooks = resolve(dir, git(dir, &["rev-parse", "--git-path", "hooks"])?.trim());
    if !is_under(&top, &hooks) && hooks.exists() {
        if let Ok(rd) = fs::read_dir(&hooks) {
            for e in rd.flatten() {
                if e.file_type().map(|t| t.is_file()).unwrap_or(false) && !e.file_name().to_string_lossy().ends_with(".sample") {
                    add(&e.path());
                }
            }
        }
    }
    Ok(out)
}

pub fn scope_content_hash(dir: &Path, files: &[String]) -> Result<String> {
    let mut sorted = files.to_vec();
    sorted.sort();
    let mut parts = Vec::new();
    for f in &sorted {
        let line = git(dir, &["ls-files", "-s", "--", f])?;
        let line = line.trim();
        parts.push(format!("{f}\0{}", if line.is_empty() { "deleted" } else { line }));
    }
    Ok(sha256(parts.join("\n").as_bytes()))
}

pub fn patch_id(dir: &Path, base_commit: &str, head: &str) -> Result<String> {
    let diff = git_bytes(dir, &["diff", "--no-ext-diff", "--no-textconv", &format!("{base_commit}..{head}")])?;
    if diff.is_empty() {
        return Ok(String::new());
    }
    let mut c = base(dir);
    c.args(["patch-id", "--stable"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = c.spawn().map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&diff);
    });
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let _ = writer.join();
    Ok(String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap_or("").to_string())
}

#[derive(Debug, serde::Serialize)]
pub struct DiffText {
    pub text: String,
    pub truncated: bool,
}

pub fn diff_text(dir: &Path, base_commit: &str, head: &str, max_chars: usize) -> Result<DiffText> {
    let text = git(dir, &["diff", "--no-ext-diff", "--no-textconv", &format!("{base_commit}..{head}")])?;
    if text.chars().count() > max_chars {
        return Ok(DiffText { text: cut(&text, max_chars), truncated: true });
    }
    Ok(DiffText { text, truncated: false })
}

pub fn changed_paths(dir: &Path, base_commit: &str, head: &str) -> Result<Vec<String>> {
    Ok(split_nul(&git(dir, &["diff", "--no-ext-diff", "--name-only", "--no-renames", "-z", &format!("{base_commit}..{head}")])?))
}

pub fn tree_of(dir: &Path) -> Result<String> {
    git(dir, &["add", "-A"])?;
    Ok(git(dir, &["write-tree"])?.trim().to_string())
}

#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub enum MergeTree {
    Ok { ok: bool, tree: String },
    Failed { ok: bool, message: String },
}

pub fn merge_tree(repo_root: &Path, target: &str, commit: &str) -> MergeTree {
    match git(repo_root, &["merge-tree", "--write-tree", target, commit]) {
        Ok(out) => MergeTree::Ok { ok: true, tree: out.trim().lines().next().unwrap_or("").to_string() },
        Err(e) => MergeTree::Failed { ok: false, message: cut(&e, 800) },
    }
}

pub fn add_tree_worktree(repo_root: &Path, dir: &Path, base_commit: &str, tree: &str) -> Result<()> {
    fs::create_dir_all(dir.parent().ok_or("no parent")?).map_err(|e| e.to_string())?;
    git(repo_root, &["worktree", "add", "--detach", &dir.to_string_lossy(), base_commit])?;
    git(dir, &["read-tree", "-u", "--reset", tree]).map(|_| ())
}

pub fn reset_worktree(dir: &Path) -> Result<()> {
    git(dir, &["reset", "-q", "--hard"])?;
    git(dir, &["clean", "-q", "-fd"]).map(|_| ())
}

pub fn rev_parse(cwd: &Path, reference: &str) -> Result<String> {
    Ok(git(cwd, &["rev-parse", "--verify", reference])?.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wb-git-{name}-{}", std::process::id()));
        let _ = remove_tree(&d);
        fs::create_dir_all(&d).unwrap();
        git(&d, &["init", "-q", "-b", "main"]).unwrap();
        git(&d, &["config", "user.email", "t@t"]).unwrap();
        git(&d, &["config", "user.name", "t"]).unwrap();
        git(&d, &["config", "core.autocrlf", "false"]).unwrap();
        fs::write(d.join("a.txt"), "a\n").unwrap();
        git(&d, &["add", "-A"]).unwrap();
        git(&d, &["commit", "-qm", "init"]).unwrap();
        d
    }

    #[test]
    fn commit_changed_and_patch_id() {
        let d = repo("commit");
        let base = rev_parse(&d, "HEAD").unwrap();
        assert_eq!(commit_all(&d, "none", None).unwrap(), None);
        fs::write(d.join("b.txt"), "中文\n").unwrap();
        assert_eq!(changed_files(&d).unwrap(), vec!["b.txt"]);
        let hash = scope_content_hash(&d, &["b.txt".into()]).unwrap();
        fs::write(d.join("b.txt"), "changed\n").unwrap();
        git(&d, &["add", "-A"]).unwrap();
        assert_ne!(scope_content_hash(&d, &["b.txt".into()]).unwrap(), hash, "same name, new content");
        let head = commit_all(&d, "add b", None).unwrap().unwrap();
        assert_eq!(rev_parse(&d, "HEAD").unwrap(), head);
        assert!(!patch_id(&d, &base, &head).unwrap().is_empty());
        assert_eq!(patch_id(&d, &base, &base).unwrap(), "");
        assert!(diff_stat(&d, &base, &head).unwrap().contains("b.txt"));
        remove_tree(&d).unwrap();
    }

    #[test]
    fn baseline_hooks_come_from_the_commit() {
        let d = repo("hooks");
        git(&d, &["config", "core.hooksPath", ".githooks"]).unwrap();
        fs::create_dir(d.join(".githooks")).unwrap();
        fs::write(d.join(".githooks/pre-commit"), "v1\n").unwrap();
        git(&d, &["add", "-A"]).unwrap();
        git(&d, &["-c", "core.hooksPath=/dev/null", "commit", "-qm", "hook"]).unwrap();
        let c = rev_parse(&d, "HEAD").unwrap();
        fs::write(d.join(".githooks/pre-commit"), "working copy\n").unwrap();
        let dest = d.with_extension("hooks");
        export_baseline_hooks(&d, &c, &dest).unwrap();
        assert_eq!(fs::read_to_string(dest.join("pre-commit")).unwrap(), "v1\n");
        remove_tree(&dest).unwrap();
        remove_tree(&d).unwrap();
    }

    #[test]
    fn guard_sees_config_changes() {
        let d = repo("guard");
        let before = git_guard_state(&d).unwrap();
        assert!(!before.is_empty());
        assert_eq!(git_guard_state(&d).unwrap(), before);
        git(&d, &["config", "filter.evil.clean", "evil"]).unwrap();
        assert_ne!(git_guard_state(&d).unwrap(), before);
        remove_tree(&d).unwrap();
    }

    #[test]
    fn merge_tree_reports_conflicts_without_touching_the_tree() {
        let d = repo("mt");
        git(&d, &["checkout", "-q", "-b", "f"]).unwrap();
        fs::write(d.join("a.txt"), "f\n").unwrap();
        let f = commit_all(&d, "f", None).unwrap().unwrap();
        git(&d, &["checkout", "-q", "main"]).unwrap();
        assert!(matches!(merge_tree(&d, "main", &f), MergeTree::Ok { .. }));
        fs::write(d.join("a.txt"), "main\n").unwrap();
        commit_all(&d, "m", None).unwrap();
        assert!(matches!(merge_tree(&d, "main", &f), MergeTree::Failed { .. }));
        assert!(!has_changes(&d).unwrap());
        remove_tree(&d).unwrap();
    }

    #[test]
    fn remove_worktrees_stays_inside_its_base() {
        let d = repo("rm");
        let keep = d.with_extension("keep");
        fs::create_dir_all(&keep).unwrap();
        let base = d.join(".cowork");
        let wt = base.join("r1").join("main");
        add_worktree(&d, &wt, "cowork/r1/main", "HEAD").unwrap();
        remove_worktrees(&d, &base.join("r1"), &[wt.to_string_lossy().into_owned(), keep.to_string_lossy().into_owned()], None).unwrap();
        assert!(!wt.exists());
        assert!(keep.exists(), "a path outside the base is never touched");
        remove_tree(&keep).unwrap();
        remove_tree(&d).unwrap();
    }

    #[test]
    fn revert_refuses_paths_outside_the_worktree() {
        let d = repo("revert");
        assert!(revert_files(&d, &["../x".into()]).is_err());
        fs::write(d.join("new.txt"), "n").unwrap();
        fs::write(d.join("a.txt"), "edited").unwrap();
        revert_files(&d, &["new.txt".into(), "a.txt".into()]).unwrap();
        assert!(!d.join("new.txt").exists());
        assert_eq!(fs::read_to_string(d.join("a.txt")).unwrap(), "a\n");
        remove_tree(&d).unwrap();
    }
}
