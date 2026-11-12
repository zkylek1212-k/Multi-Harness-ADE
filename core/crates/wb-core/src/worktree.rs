//! Managed node worktrees. Callers must confirm the node process has exited before mutations.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use wb_git as g;

use crate::util::{mask_secrets, p, read_json_with_fallback, real_path, repo_id_of, sv, write_json_atomic};

type Result<T> = std::result::Result<T, String>;

pub fn worktree_base(repo: &Path, custom: Option<&str>) -> String {
    match custom.map(str::trim).filter(|c| !c.is_empty() && Path::new(c).is_absolute()) {
        Some(c) => crate::util::resolve(c),
        None => p(&repo.join(".cowork")),
    }
}

pub fn cloud_worktree(path: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| RegexBuilder::new("onedrive|dropbox|google ?drive|我的雲端硬碟|icloud").case_insensitive(true).build().unwrap()).is_match(path)
}

fn key(path: &Path) -> String {
    let mut prefix = path;
    let mut suffix = Vec::new();
    while !prefix.exists() {
        let Some(name) = prefix.file_name() else { break };
        suffix.push(name);
        let Some(parent) = prefix.parent() else { break };
        prefix = parent;
    }
    let mut normalized = PathBuf::from(real_path(&p(prefix)));
    for name in suffix.into_iter().rev() { normalized.push(name); }
    let s = p(&normalized).replace('\\', "/");
    let s = s.trim_end_matches('/');
    if cfg!(windows) { s.to_lowercase() } else { s.to_string() }
}

fn common_dir(repo: &Path) -> Result<PathBuf> {
    Ok(repo.join(g::git(repo, &["rev-parse", "--git-common-dir"])?.trim()))
}

fn repo_lock(repo: &Path) -> Result<Arc<Mutex<()>>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    Ok(LOCKS.get_or_init(Default::default).lock().map_err(|_| "repo lock poisoned")?.entry(key(&common_dir(repo)?)).or_default().clone())
}

fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 128 || id.contains("..") || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err("invalid node/channel id".into());
    }
    Ok(())
}

fn is_link(m: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if m.file_attributes() & 0x400 != 0 { return true; }
    }
    m.file_type().is_symlink()
}

fn no_links(path: &Path) -> Result<()> {
    let mut prefix = PathBuf::new();
    for part in path.components() {
        if matches!(part, Component::ParentDir | Component::CurDir) { return Err("non-normal path".into()); }
        prefix.push(part.as_os_str());
        match fs::symlink_metadata(&prefix) {
            Ok(m) if is_link(&m) => return Err(format!("refusing linked path: {}", prefix.display())),
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

/// Check both lexical containment and existing prefixes; canonicalizing only the leaf misses dangling links.
pub fn must_be_under(root: &Path, path: &Path) -> Result<()> {
    if !root.is_absolute() || !path.is_absolute() { return Err("path outside managed root".into()); }
    no_links(root)?;
    no_links(path)?;
    let (r, p) = (key(root), key(path));
    if p == r || !p.starts_with(&format!("{r}/")) { return Err("path outside managed root".into()); }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeWorktree {
    pub path: String,
    pub branch: String,
    pub base_commit: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ownership {
    schema_version: u32,
    node: NodeWorktree,
    root: String,
    common_dir: String,
    channel: String,
    id: String,
    nonce: String,
    ready: bool,
    guard: BTreeMap<String, String>,
    hooks_hash: String,
}

fn record_path(repo: &Path, node: &NodeWorktree) -> Result<PathBuf> {
    Ok(common_dir(repo)?.join("cowork/node-worktrees").join(format!("{}.json", repo_id_of(&node.path))))
}

fn hooks_dir(repo: &Path, node: &NodeWorktree) -> Result<PathBuf> {
    Ok(common_dir(repo)?.join("cowork/node-hooks").join(repo_id_of(&node.path)))
}

fn git_dir(path: &Path) -> Result<PathBuf> {
    Ok(path.join(g::git(path, &["rev-parse", "--git-dir"])?.trim()))
}

#[derive(Debug)]
struct Registered {
    path: String,
    branch: String,
}

fn registered(repo: &Path) -> Result<Vec<Registered>> {
    let bytes = g::git_bytes(repo, &["worktree", "list", "--porcelain", "-z"])?;
    let text = String::from_utf8(bytes).map_err(|_| "non-UTF8 worktree path")?;
    let mut out: Vec<Registered> = Vec::new();
    for part in text.split('\0') {
        if let Some(path) = part.strip_prefix("worktree ") { out.push(Registered { path: path.into(), branch: String::new() }); }
        if let Some(branch) = part.strip_prefix("branch refs/heads/") {
            if let Some(w) = out.last_mut() { w.branch = branch.into(); }
        }
    }
    Ok(out)
}

fn owned(repo: &Path, node: &NodeWorktree) -> Result<Ownership> {
    let file = record_path(repo, node)?;
    no_links(&file)?;
    let owner: Ownership = serde_json::from_slice(&fs::read(file).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    validate_id(&owner.channel)?;
    validate_id(&owner.id)?;
    let expected = Path::new(&owner.root).join(&owner.channel).join(&owner.id);
    must_be_under(Path::new(&owner.root), Path::new(&node.path))?;
    if owner.schema_version != 1 || !owner.ready || owner.node != *node || key(&expected) != key(Path::new(&node.path))
        || node.branch != format!("cowork/{}/{}", owner.channel, owner.id)
        || key(Path::new(&owner.common_dir)) != key(&common_dir(repo)?)
        || key(Path::new(&node.path)) == key(Path::new(g::git(repo, &["rev-parse", "--show-toplevel"])?.trim()))
        || key(&common_dir(Path::new(&node.path))?) != key(&common_dir(repo)?) {
        return Err("worktree ownership mismatch".into());
    }
    if !registered(repo)?.iter().any(|w| key(Path::new(&w.path)) == key(Path::new(&node.path)) && w.branch == node.branch) {
        return Err("worktree is not registered to this repo/branch".into());
    }
    let marker = git_dir(Path::new(&node.path))?.join("wb-node-owner");
    no_links(&marker)?;
    if fs::read_to_string(marker).map_err(|e| e.to_string())? != owner.nonce { return Err("ownership marker mismatch".into()); }
    Ok(owner)
}

fn check_guard(repo: &Path, node: &NodeWorktree, owner: &Ownership) -> Result<()> {
    if git_guard(repo)? != owner.guard || git_guard(Path::new(&node.path))? != owner.guard
        || fingerprint(&hooks_dir(repo, node)?)? != owner.hooks_hash {
        return Err("git-config-or-hooks-changed".into());
    }
    Ok(())
}

fn git_guard(dir: &Path) -> Result<BTreeMap<String, String>> {
    Ok(g::git_guard_state(Path::new(&real_path(&p(dir))))?.into_iter().map(|(path, hash)| (key(Path::new(&path)), hash)).collect())
}

/// Leaves a creation record on failure; startup/launch failures must not force-delete user data.
pub fn create_node(repo: &Path, channel: &str, id: &str, custom_root: Option<&str>) -> Result<NodeWorktree> {
    validate_id(channel)?;
    validate_id(id)?;
    let lock = repo_lock(repo)?;
    let _lock = lock.lock().map_err(|_| "repo lock poisoned")?;
    let baseline = g::read_baseline(repo)?;
    let root = worktree_base(Path::new(&baseline.root), custom_root);
    let path = Path::new(&root).join(channel).join(id);
    must_be_under(Path::new(&root), &path)?;
    let node = NodeWorktree { path: p(&path), branch: format!("cowork/{channel}/{id}"), base_commit: baseline.head };
    let file = record_path(repo, &node)?;
    no_links(&file)?;
    if path.exists() || file.exists() || registered(repo)?.iter().any(|w| key(Path::new(&w.path)) == key(&path)) {
        return Err("worktree/creation record already exists".into());
    }
    exclude_cowork_dir(&baseline.common_dir);
    if !fs::read_to_string(Path::new(&baseline.common_dir).join("info/exclude")).map_err(|e| e.to_string())?.lines().any(|l| l.trim() == ".cowork/") {
        return Err("could not exclude .cowork/".into());
    }
    let mut owner = Ownership { schema_version: 1, node: node.clone(), root, common_dir: baseline.common_dir, channel: channel.into(), id: id.into(), nonce: crate::util::uuid(), ready: false, guard: git_guard(repo)?, hooks_hash: String::new() };
    write_json_atomic(&file, &serde_json::to_value(&owner).map_err(|e| e.to_string())?)?;
    let hooks = hooks_dir(repo, &node)?;
    no_links(&hooks)?;
    g::export_baseline_hooks(repo, &node.base_commit, &hooks)?;
    owner.hooks_hash = fingerprint(&hooks)?;
    g::add_worktree(repo, &path, &node.branch, &node.base_commit)?;
    fs::write(git_dir(&path)?.join("wb-node-owner"), &owner.nonce).map_err(|e| e.to_string())?;
    owner.ready = true;
    write_json_atomic(&file, &serde_json::to_value(&owner).map_err(|e| e.to_string())?)?;
    Ok(node)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    pub status: String,
    pub text: String,
    pub binary: bool,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangesSummary {
    pub commits: Vec<String>,
    pub committed: Vec<FileChange>,
    pub staged: Vec<FileChange>,
    pub unstaged: Vec<FileChange>,
    pub untracked: Vec<FileChange>,
}

fn nul_paths(bytes: Vec<u8>) -> Result<Vec<String>> {
    Ok(String::from_utf8(bytes).map_err(|_| "non-UTF8 git path")?.split('\0').filter(|s| !s.is_empty()).map(str::to_string).collect())
}

fn paths(dir: &Path, args: &[&str]) -> Result<Vec<String>> { nul_paths(g::git_bytes(dir, args)?) }

fn content(path: String, status: String, bytes: Vec<u8>, limit: usize, binary: bool) -> FileChange {
    let binary = binary || bytes.contains(&0) || std::str::from_utf8(&bytes).is_err();
    let truncated = binary || bytes.len() > limit;
    let text = if truncated { String::new() } else { mask_secrets(&String::from_utf8_lossy(&bytes)) };
    FileChange { path, status, text, binary, truncated }
}

fn diff_files(dir: &Path, revisions: &[&str], limit: usize) -> Result<Vec<FileChange>> {
    let mut args = vec!["--literal-pathspecs", "diff", "--no-ext-diff", "--no-textconv", "--no-renames"];
    args.extend_from_slice(revisions);
    let mut names = args.clone();
    names.extend(["--name-status", "-z"]);
    let fields = nul_paths(g::git_bytes(dir, &names)?)?;
    let mut out = Vec::new();
    for pair in fields.chunks(2) {
        if pair.len() != 2 { return Err("invalid git diff status".into()); }
        let mut diff = args.clone();
        diff.extend(["--", &pair[1]]);
        let bytes = g::git_bytes(dir, &diff)?;
        let binary = bytes.windows(13).any(|b| b == b"Binary files ");
        out.push(content(pair[1].clone(), pair[0].clone(), bytes, limit, binary));
    }
    Ok(out)
}

pub fn changes_summary(repo: &Path, node: &NodeWorktree, max_bytes: usize) -> Result<ChangesSummary> {
    owned(repo, node)?;
    let dir = Path::new(&node.path);
    let range = format!("{}..HEAD", node.base_commit);
    let commits = g::git(dir, &["log", "--format=%H %s", &range])?.lines().map(str::to_string).collect();
    let committed = diff_files(dir, &[&node.base_commit, "HEAD"], max_bytes)?;
    let staged = diff_files(dir, &["--cached"], max_bytes)?;
    let mut unstaged = diff_files(dir, &[], max_bytes)?;
    for path in hidden_paths(dir)? {
        if !unstaged.iter().any(|c| c.path == path) {
            unstaged.push(FileChange { path, status: "index-flags".into(), text: String::new(), binary: false, truncated: true });
        }
    }
    let mut untracked = Vec::new();
    for path in paths(dir, &["ls-files", "--others", "--exclude-standard", "-z"])? {
        let file = dir.join(&path);
        // Do not follow an untracked link into secrets outside the worktree.
        let bytes = if no_links(&file).is_err() || file.is_dir() { Vec::new() } else {
            let f = fs::File::open(&file).map_err(|e| e.to_string())?;
            let mut bytes = Vec::new();
            f.take(max_bytes.saturating_add(1) as u64).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            bytes
        };
        let mut change = content(path, "?".into(), bytes, max_bytes, false);
        change.truncated |= no_links(&file).is_err() || file.is_dir();
        untracked.push(change);
    }
    Ok(ChangesSummary { commits, committed, staged, unstaged, untracked })
}

fn changed_paths(dir: &Path) -> Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    for args in [vec!["diff", "--no-ext-diff", "--no-textconv", "--name-only", "--no-renames", "-z", "HEAD"], vec!["diff", "--no-ext-diff", "--no-textconv", "--cached", "--name-only", "--no-renames", "-z"], vec!["ls-files", "--others", "--exclude-standard", "-z"]] {
        out.extend(paths(dir, &args)?);
    }
    Ok(out)
}

fn hidden_paths(dir: &Path) -> Result<Vec<String>> {
    Ok(paths(dir, &["ls-files", "-v", "-z"])?.into_iter().filter(|s| s.as_bytes().first().map(|c| c.is_ascii_lowercase() || *c == b'S').unwrap_or(false)).map(|s| s[2..].to_string()).collect())
}

fn idle(dir: &Path) -> Result<()> {
    if !g::read_baseline(dir)?.warnings.is_empty() { return Err("git operation in progress".into()); }
    Ok(())
}

fn clean(dir: &Path) -> Result<()> {
    idle(dir)?;
    // These index flags can hide modified tracked files from git status.
    if !hidden_paths(dir)?.is_empty() { return Err("hidden index flags require review".into()); }
    if g::has_changes(dir)? { return Err("dirty".into()); }
    Ok(())
}

/// The explicit list must exactly match the current changes. Never stage an unlisted file.
pub fn commit_all(repo: &Path, node: &NodeWorktree, files: &[String], message: &str) -> Result<String> {
    let lock = repo_lock(repo)?;
    let _lock = lock.lock().map_err(|_| "repo lock poisoned")?;
    let owner = owned(repo, node)?;
    check_guard(repo, node, &owner)?;
    let dir = Path::new(&node.path);
    idle(dir)?;
    if !hidden_paths(dir)?.is_empty() { return Err("hidden index flags require review".into()); }
    let listed: BTreeSet<_> = files.iter().cloned().collect();
    if message.trim().is_empty() || files.is_empty() || listed.len() != files.len() || listed != changed_paths(dir)? {
        return Err("stale file list or empty commit message".into());
    }
    for file in files {
        if file.contains('\\') || file.contains(':') || file.split('/').any(|x| x.is_empty() || x == "." || x == ".." || x.eq_ignore_ascii_case(".git")) { return Err("invalid file path".into()); }
        must_be_under(dir, &dir.join(file))?;
    }
    let hooks = hooks_dir(repo, node)?;
    let mut add = vec!["--literal-pathspecs", "add", "-A", "--"];
    add.extend(files.iter().map(String::as_str));
    g::git(dir, &add)?;
    if changed_paths(dir)? != listed || paths(dir, &["diff", "--no-ext-diff", "--no-textconv", "--cached", "--name-only", "--no-renames", "-z"])?.into_iter().collect::<BTreeSet<_>>() != listed {
        return Err("changes moved while staging; review again".into());
    }
    let before = g::rev_parse(dir, "HEAD")?;
    let head = g::commit_staged(dir, message, Some(&p(&hooks)))?;
    let committed: BTreeSet<_> = paths(dir, &["diff", "--no-ext-diff", "--no-textconv", "--name-only", "--no-renames", "-z", &before, &head])?.into_iter().collect();
    if committed != listed || g::git(dir, &["show", "-s", "--format=%P", &head])?.trim() != before {
        // Rewind only our commit, leaving every staged/worktree byte for the user to review.
        g::git(dir, &["update-ref", &format!("refs/heads/{}", node.branch), &before, &head])?;
        return Err("commit hook changed the listed files; commit rolled back, review staged changes".into());
    }
    Ok(head)
}

#[derive(Debug)]
pub struct MergeToken {
    node: NodeWorktree,
    source: String,
    target_branch: String,
    target_head: String,
}

fn target_head(repo: &Path, branch: &str) -> Result<String> {
    g::git(repo, &["check-ref-format", &format!("refs/heads/{branch}")])?;
    if g::git(repo, &["symbolic-ref", "--short", "HEAD"])?.trim() != branch { return Err("wrong-branch".into()); }
    g::rev_parse(repo, &format!("refs/heads/{branch}"))
}

fn source_head(node: &NodeWorktree) -> Result<String> {
    let dir = Path::new(&node.path);
    clean(dir)?;
    if g::git(dir, &["symbolic-ref", "--short", "HEAD"])?.trim() != node.branch { return Err("wrong-source-branch".into()); }
    g::rev_parse(dir, "HEAD")
}

pub fn merge_preview(repo: &Path, node: &NodeWorktree, target_branch: &str) -> Result<MergeToken> {
    let lock = repo_lock(repo)?;
    let _lock = lock.lock().map_err(|_| "repo lock poisoned")?;
    let owner = owned(repo, node)?;
    check_guard(repo, node, &owner)?;
    clean(repo)?;
    Ok(MergeToken { node: node.clone(), source: source_head(node)?, target_branch: target_branch.into(), target_head: target_head(repo, target_branch)? })
}

pub fn merge(repo: &Path, token: &MergeToken) -> Result<String> {
    let lock = repo_lock(repo)?;
    let _lock = lock.lock().map_err(|_| "repo lock poisoned")?;
    let owner = owned(repo, &token.node)?;
    check_guard(repo, &token.node, &owner)?;
    clean(repo)?;
    if source_head(&token.node)? != token.source { return Err("stale source".into()); }
    let head = target_head(repo, &token.target_branch)?;
    if head != token.target_head {
        // Recover a completed merge even if the response was lost, without accepting unrelated target changes.
        let parents = g::git(repo, &["show", "-s", "--format=%P", &head])?;
        if parents.trim() == format!("{} {}", token.target_head, token.source) { return Ok(head); }
        return Err("stale target".into());
    }
    if g::git(repo, &["merge-base", "--is-ancestor", &token.source, &head]).is_ok() { return Ok(head); }
    let tree = match g::merge_tree(repo, &head, &token.source) {
        g::MergeTree::Ok { tree, .. } => tree,
        g::MergeTree::Failed { message, .. } => return Err(format!("conflict: {message}")),
    };
    let incoming = paths(repo, &["diff", "--no-ext-diff", "--no-textconv", "--name-only", "--no-renames", "-z", &head, &tree])?;
    for local in ignored(repo)? {
        let normalize = |s: &str| if cfg!(windows) { s.trim_end_matches('/').to_lowercase() } else { s.trim_end_matches('/').to_string() };
        let local = normalize(&local);
        if incoming.iter().map(|s| normalize(s)).any(|p| p == local || p.starts_with(&format!("{local}/")) || local.starts_with(&format!("{p}/"))) {
            return Err(format!("ignored target data would be overwritten: {local}"));
        }
    }
    let hooks = p(&hooks_dir(repo, &token.node)?);
    match g::merge_branch(repo, &token.target_branch, &token.source, &format!("Merge {}", token.node.branch), Some(&hooks))? {
        g::MergeResult::Ok { commit, .. } => Ok(commit),
        g::MergeResult::Failed { reason, message, .. } => Err(format!("{reason}: {message}")),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscardPreview {
    pub unmerged_commits: Vec<String>,
    pub changes: ChangesSummary,
    pub ignored: Vec<String>,
    #[serde(skip)]
    pub token: DiscardToken,
}

#[derive(Debug)]
pub struct DiscardToken {
    node: NodeWorktree,
    target_branch: String,
    target_head: String,
    fingerprint: String,
}

fn ignored(dir: &Path) -> Result<Vec<String>> { paths(dir, &["ls-files", "--others", "--ignored", "--exclude-standard", "-z"]) }

fn hash_tree(path: &Path, hash: &mut Sha256, root: bool) -> Result<()> {
    let m = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if is_link(&m) { return Err(format!("linked data requires manual cleanup: {}", path.display())); }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        hash.update(m.permissions().mode().to_le_bytes());
    }
    hash.update([u8::from(m.permissions().readonly())]);
    if m.is_dir() {
        hash.update(b"directory\0");
        let mut entries = fs::read_dir(path).map_err(|e| e.to_string())?.collect::<std::io::Result<Vec<_>>>().map_err(|e| e.to_string())?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if root && entry.file_name() == ".git" { continue; }
            let name = entry.file_name();
            let name = name.to_str().ok_or("non-UTF8 path")?;
            hash.update((name.len() as u64).to_le_bytes());
            hash.update(name.as_bytes());
            hash_tree(&entry.path(), hash, false)?;
        }
        hash.update(b"end\0");
    } else if m.is_file() {
        hash.update(b"file\0");
        hash.update(m.len().to_le_bytes());
        let mut f = fs::File::open(path).map_err(|e| e.to_string())?;
        let mut buf = [0u8; 65536];
        loop {
            let n = f.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 { break; }
            hash.update(&buf[..n]);
        }
    } else { return Err("special file requires manual cleanup".into()); }
    Ok(())
}

fn fingerprint(path: &Path) -> Result<String> {
    let mut hash = Sha256::new();
    hash_tree(path, &mut hash, true)?;
    Ok(format!("{:x}", hash.finalize()))
}

fn discard_state(node: &NodeWorktree) -> Result<String> {
    let dir = Path::new(&node.path);
    let mut hash = Sha256::new();
    hash.update(fingerprint(dir)?);
    hash.update(g::rev_parse(dir, "HEAD")?);
    hash.update(g::git_bytes(dir, &["ls-files", "--stage", "-z"])?);
    hash.update(g::git_bytes(dir, &["status", "--porcelain=v1", "-z", "--untracked-files=all", "--ignored"])?);
    Ok(format!("{:x}", hash.finalize()))
}

pub fn discard_preview(repo: &Path, node: &NodeWorktree, target_branch: &str) -> Result<DiscardPreview> {
    let lock = repo_lock(repo)?;
    let _lock = lock.lock().map_err(|_| "repo lock poisoned")?;
    owned(repo, node)?;
    idle(Path::new(&node.path))?;
    let head = target_head(repo, target_branch)?;
    let before = discard_state(node)?;
    let range = format!("{head}..{}", node.branch);
    let unmerged_commits = g::git(repo, &["log", "--format=%H %s", &range])?.lines().map(str::to_string).collect();
    let changes = changes_summary(repo, node, 64 * 1024)?;
    let ignored = ignored(Path::new(&node.path))?;
    if discard_state(node)? != before { return Err("changes moved during preview".into()); }
    Ok(DiscardPreview { unmerged_commits, changes, ignored, token: DiscardToken { node: node.clone(), target_branch: target_branch.into(), target_head: head, fingerprint: before } })
}

fn remove_owned(repo: &Path, node: &NodeWorktree) -> Result<()> {
    owned(repo, node)?;
    // No recursive filesystem fallback: a locked/failed git removal must preserve the directory for retry.
    g::git(repo, &["worktree", "remove", "--force", &node.path])?;
    let record = record_path(repo, node)?;
    fs::remove_file(&record).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(crate::util::prev_path(&record));
    Ok(())
}

pub fn discard(repo: &Path, token: &DiscardToken) -> Result<()> {
    let lock = repo_lock(repo)?;
    let _lock = lock.lock().map_err(|_| "repo lock poisoned")?;
    owned(repo, &token.node)?;
    idle(Path::new(&token.node.path))?;
    if target_head(repo, &token.target_branch)? != token.target_head || discard_state(&token.node)? != token.fingerprint { return Err("stale discard preview".into()); }
    remove_owned(repo, &token.node)
}

pub fn cleanup(repo: &Path, node: &NodeWorktree, target_branch: &str) -> Result<()> {
    let lock = repo_lock(repo)?;
    let _lock = lock.lock().map_err(|_| "repo lock poisoned")?;
    owned(repo, node)?;
    let source = source_head(node)?;
    let target = target_head(repo, target_branch)?;
    if g::git(repo, &["merge-base", "--is-ancestor", &source, &target]).is_err() { return Err("unmerged commits".into()); }
    if !ignored(Path::new(&node.path))?.is_empty() { return Err("ignored data requires discard preview".into()); }
    fingerprint(Path::new(&node.path))?;
    remove_owned(repo, node)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyWorktree {
    pub path: String,
    pub branch: String,
    pub base_commit: Option<String>,
    pub ownership_provable: bool,
    pub dirty: Vec<String>,
    pub untracked: Vec<String>,
    pub ignored: Vec<String>,
    pub error: Option<String>,
}

/// `data_dir` is the old <app data>/cowork directory; settings is the unmodified settings JSON.
/// Inventory never grants deletion rights: only new nodes with a matching ownership marker can be removed.
pub fn legacy_inventory(repo: &Path, data_dir: &Path, settings: &Value) -> Result<Vec<LegacyWorktree>> {
    let common = common_dir(repo)?;
    let repo_key = repo_id_of(&p(&common));
    let manifests = data_dir.join(&repo_key);
    let mut runs = Vec::new();
    if let Ok(entries) = fs::read_dir(manifests) {
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.file_type().map_err(|e| e.to_string())?.is_dir() { continue; }
            if let Some((run, _)) = read_json_with_fallback(&entry.path().join("run.json"), &|v| v["schemaVersion"] == 1 && v["execution"].is_object()) {
                if repo_id_of(sv(&run["repo"], "commonDir")) == repo_key { runs.push(run); }
            }
        }
    }
    let root = worktree_base(Path::new(g::git(repo, &["rev-parse", "--show-toplevel"])?.trim()), settings["cowork"]["worktreeRoot"].as_str());
    let main = g::git(repo, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let mut out = Vec::new();
    for wt in registered(repo)? {
        let path = Path::new(&wt.path);
        // The primary checkout has .git as its git dir; linked worktrees have their own admin directory.
        if git_dir(path).map(|d| key(&d) == key(Path::new(main.trim()))).unwrap_or(false) { continue; }
        let mut base_commit = None;
        let mut ownership_provable = false;
        let mut mentioned = false;
        for run in &runs {
            let ex = &run["execution"];
            for (id, w) in ex["worktrees"].as_object().into_iter().flatten() {
                if key(Path::new(sv(w, "path"))) != key(path) { continue; }
                mentioned = true;
                base_commit = run["repo"]["baseCommit"].as_str().map(str::to_string);
                let base = ex["worktreeBase"].as_str().unwrap_or(&root);
                let expected = Path::new(base).join(sv(run, "id")).join(id);
                let branch = format!("cowork/{}/{id}", sv(run, "id"));
                ownership_provable |= validate_id(sv(run, "id")).is_ok() && validate_id(id).is_ok()
                    && must_be_under(Path::new(base), path).is_ok() && key(&expected) == key(path)
                    && wt.branch == branch && sv(w, "branch") == branch
                    && common_dir(path).map(|c| key(&c) == key(&common)).unwrap_or(false);
            }
        }
        if !mentioned && !wt.branch.starts_with("cowork/") && must_be_under(Path::new(&root), path).is_err() { continue; }
        // New nodes have their own records and are not legacy runs.
        let candidate = NodeWorktree { path: wt.path.clone(), branch: wt.branch.clone(), base_commit: base_commit.clone().unwrap_or_default() };
        if record_path(repo, &candidate)?.exists() { continue; }
        let state = (|| -> Result<_> {
            let untracked = paths(path, &["ls-files", "--others", "--exclude-standard", "-z"])?;
            let dirty = changed_paths(path)?.into_iter().filter(|p| !untracked.contains(p)).collect();
            Ok((dirty, untracked, ignored(path)?))
        })();
        let (dirty, untracked, ignored, error) = match state {
            Ok((dirty, untracked, ignored)) => (dirty, untracked, ignored, None),
            Err(e) => (Vec::new(), Vec::new(), Vec::new(), Some(e)),
        };
        out.push(LegacyWorktree { path: wt.path, branch: wt.branch, base_commit, ownership_provable, dirty, untracked, ignored, error });
    }
    Ok(out)
}

/// Git marks some files read-only on Windows, and a plain delete then fails
fn remove_tree(dir: &Path) -> std::io::Result<()> {
    if fs::remove_dir_all(dir).is_ok() || !dir.exists() {
        return Ok(());
    }
    fn walk(p: &Path) {
        for e in fs::read_dir(p).into_iter().flatten().flatten() {
            let t = fs::symlink_metadata(e.path());
            if t.as_ref().map(|m| is_link(m)).unwrap_or(true) { continue; }
            if t.as_ref().map(|t| t.is_dir()).unwrap_or(false) {
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
pub fn remove_snapshot(dir: &str, under: &str) -> Result<()> {
    if !Path::new(dir).exists() {
        return Ok(());
    }
    let (real, base) = (real_path(dir), real_path(under));
    no_links(Path::new(dir)).and_then(|_| must_be_under(Path::new(&base), Path::new(&real))).map_err(|e| format!("refusing to delete {real}: {e}"))?;
    let b = key(Path::new(&base));
    remove_tree(Path::new(&real)).map_err(|e| e.to_string())?;
    // Empty run and cowork folders go too
    let mut p = crate::util::dirname(&real);
    while key(Path::new(&p)) != b && key(Path::new(&p)).starts_with(&format!("{b}/")) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Repo { temp: PathBuf, repo: PathBuf }

    impl Repo {
        fn new() -> Self {
            let temp = std::env::temp_dir().join(format!("wb-node-{}", crate::util::uuid()));
            let repo = temp.join("repo");
            fs::create_dir_all(&repo).unwrap();
            g::git(&repo, &["init", "-q", "-b", "main"]).unwrap();
            for (k, v) in [("user.name", "Node test"), ("user.email", "node@test.invalid"), ("core.autocrlf", "false")] {
                g::git(&repo, &["config", k, v]).unwrap();
            }
            fs::write(repo.join("a.txt"), "base\n").unwrap();
            fs::write(repo.join(".gitignore"), "ignored.txt\nignored/\n").unwrap();
            g::commit_all(&repo, "initial", None).unwrap();
            Self { temp, repo }
        }
        fn node(&self) -> NodeWorktree { create_node(&self.repo, "channel", "node", None).unwrap() }
        fn commit(&self, node: &NodeWorktree, file: &str, text: &str) -> String {
            fs::write(Path::new(&node.path).join(file), text).unwrap();
            commit_all(&self.repo, node, &[file.into()], "node change").unwrap()
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) { let _ = remove_tree(&self.temp); }
    }

    #[test]
    fn creation_validates_ids_and_keeps_ownership_outside_the_worktree() {
        let r = Repo::new();
        for bad in ["", "..", "a..b", "a/b", "a\\b", "C:", "n."] {
            assert!(create_node(&r.repo, bad, "node", None).is_err());
            assert!(create_node(&r.repo, "channel", bad, None).is_err());
        }
        let node = r.node();
        assert_eq!(key(Path::new(&node.path)), key(&r.repo.join(".cowork/channel/node")));
        assert_eq!(node.branch, "cowork/channel/node");
        assert_eq!(node.base_commit, g::rev_parse(&r.repo, "HEAD").unwrap());
        assert!(owned(&r.repo, &node).is_ok());
        assert!(!g::has_changes(&r.repo).unwrap());
        assert!(create_node(&r.repo, "channel", "node", None).is_err());
        let custom = r.temp.join("custom");
        let other = create_node(&r.repo, "channel", "other", Some(&p(&custom))).unwrap();
        assert_eq!(key(Path::new(&other.path)), key(&custom.join("channel/other")));
        assert!(cloud_worktree("C:/OneDrive/project/.cowork"));
        assert!(!cloud_worktree("C:/local/project/.cowork"));
    }

    #[test]
    fn summary_keeps_every_layer_binary_and_oversized_files() {
        let r = Repo::new();
        let node = r.node();
        r.commit(&node, "committed.txt", "committed\n");
        let dir = Path::new(&node.path);
        fs::write(dir.join("a.txt"), "staged\n").unwrap();
        g::git(dir, &["add", "a.txt"]).unwrap();
        fs::write(dir.join("a.txt"), "unstaged\n").unwrap();
        fs::write(dir.join("binary.dat"), [0, 1, 2]).unwrap();
        fs::write(dir.join("large.txt"), "x".repeat(1025)).unwrap();
        fs::write(dir.join("secret.txt"), "token=abcdefghijklmnop").unwrap();
        let summary = changes_summary(&r.repo, &node, 1024).unwrap();
        assert_eq!(summary.commits.len(), 1);
        assert_eq!(summary.committed[0].path, "committed.txt");
        assert_eq!(summary.staged[0].path, "a.txt");
        assert_eq!(summary.unstaged[0].path, "a.txt");
        assert!(summary.untracked.iter().any(|c| c.path == "binary.dat" && c.binary && c.truncated));
        assert!(summary.untracked.iter().any(|c| c.path == "large.txt" && c.truncated));
        assert!(summary.untracked.iter().any(|c| c.path == "secret.txt" && c.text.contains("[masked]")));
        assert!(g::git(dir, &["diff", "--cached"]).unwrap().contains("staged"), "summary must not stage anything");
        g::git(dir, &["add", "binary.dat"]).unwrap();
        assert!(changes_summary(&r.repo, &node, 1024).unwrap().staged.iter().any(|c| c.path == "binary.dat" && c.binary && c.truncated));
    }

    #[test]
    fn commit_requires_the_exact_explicit_list_including_deletions_and_untracked() {
        let r = Repo::new();
        let node = r.node();
        let dir = Path::new(&node.path);
        fs::remove_file(dir.join("a.txt")).unwrap();
        fs::write(dir.join("file[1].txt"), "new\n").unwrap();
        assert!(commit_all(&r.repo, &node, &["a.txt".into()], "incomplete").is_err());
        assert!(g::git(dir, &["diff", "--cached"]).unwrap().is_empty());
        assert!(commit_all(&r.repo, &node, &["a.txt".into(), "file[1].txt".into()], " ").is_err());
        commit_all(&r.repo, &node, &["a.txt".into(), "file[1].txt".into()], "delete and add").unwrap();
        assert!(!g::has_changes(dir).unwrap());
        assert_eq!(g::git(dir, &["show", "HEAD:file[1].txt"]).unwrap(), "new\n");
    }

    #[test]
    fn git_config_and_exported_hooks_cannot_change_after_creation() {
        let r = Repo::new();
        let node = r.node();
        fs::write(Path::new(&node.path).join("new.txt"), "new").unwrap();
        g::git(&r.repo, &["config", "filter.changed.clean", "false"]).unwrap();
        assert!(commit_all(&r.repo, &node, &["new.txt".into()], "guarded").unwrap_err().contains("git-config"));
        g::git(&r.repo, &["config", "--unset", "filter.changed.clean"]).unwrap();
        fs::write(hooks_dir(&r.repo, &node).unwrap().join("pre-commit"), "modified").unwrap();
        assert!(commit_all(&r.repo, &node, &["new.txt".into()], "guarded").unwrap_err().contains("hooks"));
    }

    #[test]
    fn commit_hooks_cannot_leave_unlisted_files_committed() {
        let r = Repo::new();
        fs::create_dir(r.repo.join(".githooks")).unwrap();
        #[cfg(not(windows))]
        fs::write(r.repo.join(".githooks/pre-commit"), "#!/bin/sh\nprintf extra > extra.txt\ngit add extra.txt\n").unwrap();
        #[cfg(windows)]
        {
            // A native hook avoids Git Bash's signal-pipe dependency inside the Windows sandbox.
            let source = r.temp.join("hook.rs");
            fs::write(&source, r#"fn main() {
                std::fs::write("extra.txt", "extra").unwrap();
                assert!(std::process::Command::new("git").args(["add", "extra.txt"]).status().unwrap().success());
            }"#).unwrap();
            let out = std::process::Command::new("rustc").arg(&source).arg("-o").arg(r.repo.join(".githooks/pre-commit.exe")).output().unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        }
        g::commit_all(&r.repo, "baseline hook", None).unwrap();
        g::git(&r.repo, &["config", "core.hooksPath", ".githooks"]).unwrap();
        let node = r.node();
        let before = g::rev_parse(Path::new(&node.path), "HEAD").unwrap();
        fs::write(Path::new(&node.path).join("new.txt"), "new").unwrap();
        let error = commit_all(&r.repo, &node, &["new.txt".into()], "explicit").unwrap_err();
        assert!(error.contains("rolled back"), "{error}");
        assert_eq!(g::rev_parse(Path::new(&node.path), "HEAD").unwrap(), before);
        assert!(Path::new(&node.path).join("extra.txt").exists());
        assert!(Path::new(&node.path).join("new.txt").exists());
    }

    #[test]
    fn hidden_index_flags_cannot_make_changed_files_look_safe_to_clean_or_merge() {
        let r = Repo::new();
        let node = r.node();
        let dir = Path::new(&node.path);
        g::git(dir, &["update-index", "--assume-unchanged", "a.txt"]).unwrap();
        fs::write(dir.join("a.txt"), "hidden user data").unwrap();
        assert!(cleanup(&r.repo, &node, "main").is_err());
        assert!(merge_preview(&r.repo, &node, "main").is_err());
        assert!(changes_summary(&r.repo, &node, 1024).unwrap().unstaged.iter().any(|c| c.path == "a.txt" && c.truncated));
    }

    #[test]
    fn merging_is_serialized_idempotent_and_cleanup_requires_no_local_data() {
        let r = Repo::new();
        let node = r.node();
        let source = r.commit(&node, "new.txt", "node\n");
        let token = Arc::new(merge_preview(&r.repo, &node, "main").unwrap());
        let joins: Vec<_> = (0..2).map(|_| {
            let repo = r.repo.clone();
            let token = token.clone();
            std::thread::spawn(move || merge(&repo, &token).unwrap())
        }).collect();
        let heads: Vec<_> = joins.into_iter().map(|j| j.join().unwrap()).collect();
        assert_eq!(heads[0], heads[1]);
        assert_eq!(merge(&r.repo, &token).unwrap(), heads[0]);
        assert!(g::git(&r.repo, &["merge-base", "--is-ancestor", &source, "HEAD"]).is_ok());
        assert_eq!(g::git(&r.repo, &["rev-list", "--count", "--merges", "HEAD"]).unwrap().trim(), "1");
        let dir = Path::new(&node.path);
        fs::write(dir.join("untracked.txt"), "keep").unwrap();
        assert!(cleanup(&r.repo, &node, "main").is_err());
        fs::remove_file(dir.join("untracked.txt")).unwrap();
        fs::write(dir.join("ignored.txt"), "private data").unwrap();
        assert!(cleanup(&r.repo, &node, "main").unwrap_err().contains("ignored"));
        fs::remove_file(dir.join("ignored.txt")).unwrap();
        cleanup(&r.repo, &node, "main").unwrap();
        assert!(!dir.exists());
        assert!(!record_path(&r.repo, &node).unwrap().exists());
    }

    #[test]
    fn merge_rejects_stale_source_target_branch_and_dirty_workspaces() {
        let r = Repo::new();
        let node = r.node();
        r.commit(&node, "new.txt", "node\n");
        let source_token = merge_preview(&r.repo, &node, "main").unwrap();
        r.commit(&node, "new.txt", "second\n");
        assert!(merge(&r.repo, &source_token).unwrap_err().contains("source"));
        let target_token = merge_preview(&r.repo, &node, "main").unwrap();
        fs::write(r.repo.join("target.txt"), "target\n").unwrap();
        assert!(merge(&r.repo, &target_token).unwrap_err().contains("dirty"));
        g::commit_all(&r.repo, "target", None).unwrap();
        assert!(merge(&r.repo, &target_token).unwrap_err().contains("target"));
        let token = merge_preview(&r.repo, &node, "main").unwrap();
        g::git(&r.repo, &["checkout", "-q", "-b", "other"]).unwrap();
        assert!(merge(&r.repo, &token).unwrap_err().contains("branch"));
        g::git(&r.repo, &["checkout", "-q", "main"]).unwrap();
        fs::write(Path::new(&node.path).join("new.txt"), "dirty").unwrap();
        assert!(merge(&r.repo, &token).unwrap_err().contains("dirty"));
    }

    #[test]
    fn merge_conflict_preserves_both_sides_and_does_not_leave_a_merge() {
        let r = Repo::new();
        let node = r.node();
        let source = r.commit(&node, "a.txt", "source\n");
        fs::write(r.repo.join("a.txt"), "target\n").unwrap();
        let target = g::commit_all(&r.repo, "target", None).unwrap().unwrap();
        let token = merge_preview(&r.repo, &node, "main").unwrap();
        assert!(merge(&r.repo, &token).unwrap_err().contains("conflict"));
        assert_eq!(g::rev_parse(&r.repo, "HEAD").unwrap(), target);
        assert_eq!(g::rev_parse(Path::new(&node.path), "HEAD").unwrap(), source);
        assert_eq!(fs::read_to_string(r.repo.join("a.txt")).unwrap(), "target\n");
        assert!(g::read_baseline(&r.repo).unwrap().warnings.is_empty());
        assert!(cleanup(&r.repo, &node, "main").is_err());
    }

    #[test]
    fn merge_never_overwrites_ignored_target_data() {
        let r = Repo::new();
        let node = r.node();
        let dir = Path::new(&node.path);
        fs::write(dir.join("ignored.txt"), "source\n").unwrap();
        g::git(dir, &["add", "-f", "ignored.txt"]).unwrap();
        g::git(dir, &["commit", "-qm", "tracked on source"]).unwrap();
        fs::write(r.repo.join("ignored.txt"), "target local data\n").unwrap();
        let head = g::rev_parse(&r.repo, "HEAD").unwrap();
        let token = merge_preview(&r.repo, &node, "main").unwrap();
        assert!(merge(&r.repo, &token).is_err());
        assert_eq!(fs::read_to_string(r.repo.join("ignored.txt")).unwrap(), "target local data\n");
        assert_eq!(g::rev_parse(&r.repo, "HEAD").unwrap(), head);
        assert!(g::read_baseline(&r.repo).unwrap().warnings.is_empty());
    }

    #[test]
    fn discard_lists_all_data_and_rechecks_content_not_just_names() {
        let r = Repo::new();
        let node = r.node();
        r.commit(&node, "new.txt", "unmerged\n");
        let dir = Path::new(&node.path);
        fs::write(dir.join("a.txt"), "tracked\n").unwrap();
        fs::write(dir.join("local.txt"), "untracked\n").unwrap();
        fs::write(dir.join("ignored.txt"), "secret\n").unwrap();
        let preview = discard_preview(&r.repo, &node, "main").unwrap();
        assert_eq!(preview.unmerged_commits.len(), 1);
        assert_eq!(preview.changes.unstaged[0].path, "a.txt");
        assert_eq!(preview.changes.untracked[0].path, "local.txt");
        assert_eq!(preview.ignored, ["ignored.txt"]);
        fs::write(dir.join("ignored.txt"), "edited\n").unwrap();
        assert!(discard(&r.repo, &preview.token).unwrap_err().contains("stale"));
        let preview = discard_preview(&r.repo, &node, "main").unwrap();
        g::git(dir, &["add", "a.txt"]).unwrap();
        assert!(discard(&r.repo, &preview.token).is_err(), "staging alone invalidates approval");
        let preview = discard_preview(&r.repo, &node, "main").unwrap();
        discard(&r.repo, &preview.token).unwrap();
        assert!(!dir.exists());
        assert!(r.repo.join("a.txt").exists());
    }

    #[test]
    fn deletion_requires_registration_repo_ownership_and_marker_never_a_root_node() {
        let r = Repo::new();
        let node = r.node();
        let mut forged = node.clone();
        forged.path = p(&r.repo);
        assert!(cleanup(&r.repo, &forged, "main").is_err());
        assert!(discard_preview(&r.repo, &forged, "main").is_err());
        let other = Repo::new();
        assert!(cleanup(&other.repo, &node, "main").is_err());
        let marker = git_dir(Path::new(&node.path)).unwrap().join("wb-node-owner");
        fs::write(&marker, "forged").unwrap();
        assert!(cleanup(&r.repo, &node, "main").is_err());
        assert!(Path::new(&node.path).exists());
        let external = r.repo.join(".cowork/external/node");
        g::add_worktree(&r.repo, &external, "cowork/external/node", "HEAD").unwrap();
        let external = NodeWorktree { path: p(&external), branch: "cowork/external/node".into(), base_commit: node.base_commit.clone() };
        assert!(cleanup(&r.repo, &external, "main").is_err());
    }

    #[test]
    fn discard_target_and_worktree_registration_are_rechecked() {
        let r = Repo::new();
        let node = r.node();
        let preview = discard_preview(&r.repo, &node, "main").unwrap();
        fs::write(r.repo.join("target.txt"), "target").unwrap();
        g::commit_all(&r.repo, "target", None).unwrap();
        assert!(discard(&r.repo, &preview.token).is_err());
        let preview = discard_preview(&r.repo, &node, "main").unwrap();
        g::git(Path::new(&node.path), &["checkout", "-q", "--detach"]).unwrap();
        assert!(discard(&r.repo, &preview.token).is_err());
        assert!(Path::new(&node.path).exists());
    }

    #[test]
    fn junctions_and_path_traversal_are_rejected_before_creation_or_deletion() {
        let r = Repo::new();
        let outside = r.temp.join("outside");
        fs::create_dir(&outside).unwrap();
        assert!(must_be_under(&r.repo, &r.repo.join("../outside")).is_err());
        let link = r.repo.join(".cowork");
        #[cfg(windows)]
        junction::create(&outside, &link).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert!(create_node(&r.repo, "channel", "node", None).is_err());
        #[cfg(windows)]
        junction::delete(&link).unwrap();
        #[cfg(unix)]
        fs::remove_file(&link).unwrap();
        let node = r.node();
        fs::write(outside.join("keep.txt"), "keep").unwrap();
        let link = Path::new(&node.path).join("ignored");
        #[cfg(windows)]
        junction::create(&outside, &link).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert!(cleanup(&r.repo, &node, "main").is_err());
        assert!(discard_preview(&r.repo, &node, "main").is_err());
        assert_eq!(fs::read_to_string(outside.join("keep.txt")).unwrap(), "keep");
        #[cfg(windows)]
        junction::delete(&link).unwrap();
        #[cfg(unix)]
        fs::remove_file(&link).unwrap();
    }

    #[test]
    fn legacy_inventory_cross_checks_external_roots_missing_manifests_and_local_data() {
        let r = Repo::new();
        let custom = r.temp.join("old-custom");
        let external = custom.join("run1/main");
        let missing = custom.join("run2/main");
        let unrelated = r.temp.join("external-feature");
        let base = g::rev_parse(&r.repo, "HEAD").unwrap();
        for (path, branch) in [(&external, "cowork/run1/main"), (&missing, "cowork/run2/main"), (&unrelated, "feature")] {
            g::add_worktree(&r.repo, path, branch, &base).unwrap();
        }
        fs::write(external.join("a.txt"), "uncommitted\n").unwrap();
        fs::write(external.join("local.txt"), "untracked\n").unwrap();
        fs::write(external.join("ignored.txt"), "ignored\n").unwrap();
        let data = r.temp.join("data/cowork");
        let common = common_dir(&r.repo).unwrap();
        let manifest = data.join(repo_id_of(&p(&common))).join("run1/run.json");
        let run = json!({ "schemaVersion": 1, "id": "run1", "repo": { "root": p(&r.repo), "commonDir": p(&common), "baseCommit": base }, "execution": { "worktreeBase": p(&custom), "root": p(&custom.join("run1")), "worktrees": { "main": { "path": p(&external), "branch": "cowork/run1/main" } } } });
        write_json_atomic(&manifest, &run).unwrap();
        let settings = json!({"cowork": {"worktreeRoot": p(&custom)}});
        let before = discard_state(&NodeWorktree { path: p(&external), branch: "cowork/run1/main".into(), base_commit: base.clone() }).unwrap();
        let inventory = legacy_inventory(&r.repo, &data, &settings).unwrap();
        assert_eq!(inventory.len(), 2);
        let found = inventory.iter().find(|w| w.branch == "cowork/run1/main").unwrap();
        assert!(found.ownership_provable);
        assert_eq!(found.base_commit.as_deref(), Some(base.as_str()));
        assert_eq!(found.dirty, ["a.txt"]);
        assert_eq!(found.untracked, ["local.txt"]);
        assert_eq!(found.ignored, ["ignored.txt"]);
        assert!(!inventory.iter().find(|w| w.branch == "cowork/run2/main").unwrap().ownership_provable);
        assert_eq!(discard_state(&NodeWorktree { path: p(&external), branch: "cowork/run1/main".into(), base_commit: base }).unwrap(), before);
        fs::remove_file(&manifest).unwrap();
        assert!(legacy_inventory(&r.repo, &data, &settings).unwrap().iter().all(|w| !w.ownership_provable));
    }

    #[test]
    fn snapshot_deletion_and_exclusion_keep_the_existing_safety_contract() {
        let r = Repo::new();
        let base = r.temp.join("snapshots");
        let snapshot = base.join("run/snapshot");
        fs::create_dir_all(&snapshot).unwrap();
        fs::write(snapshot.join("data"), "snapshot").unwrap();
        assert!(remove_snapshot(&p(&r.repo), &p(&base)).is_err());
        assert!(remove_snapshot(&p(&base), &p(&base)).is_err());
        remove_snapshot(&p(&snapshot), &p(&base)).unwrap();
        assert!(!snapshot.exists());
        let common = common_dir(&r.repo).unwrap();
        exclude_cowork_dir(&p(&common));
        exclude_cowork_dir(&p(&common));
        assert_eq!(fs::read_to_string(common.join("info/exclude")).unwrap().lines().filter(|l| *l == ".cowork/").count(), 1);
    }
}
