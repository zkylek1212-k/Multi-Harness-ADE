// Dashboard session scan over ~/.claude, ~/.codex and ~/.gemini. Runs off the UI thread; the cache file carries over.
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use regex::Regex;
use serde_json::{json, Value};

use crate::usage::{sum_days, Agent, UsageCache, AGY_DIRS};
use crate::util::{basename, dirname, exists, first_line, home, iso, js_len, js_prefix, mtime_ms, now_ms, p, parse_json, read_text, sv};

pub fn normalize_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let c = if c == '\\' { '/' } else { c };
        if c == '/' && out.ends_with('/') {
            continue;
        }
        out.push(c);
    }
    let lower = out.to_lowercase();
    lower.trim_end_matches('/').to_string()
}

pub fn is_same_workspace(path1: Option<&str>, name1: Option<&str>, path2: Option<&str>, name2: Option<&str>) -> bool {
    let norm1 = normalize_path(path1.unwrap_or(""));
    let norm2 = normalize_path(path2.unwrap_or(""));
    if !norm1.is_empty() && !norm2.is_empty() && (norm1 == norm2 || norm1.ends_with(&format!("/{norm2}")) || norm2.ends_with(&format!("/{norm1}"))) {
        return true;
    }
    // A side with no path, or a failed comparison: fall back to the folder name
    let pick = |name: Option<&str>, norm: &str| name.filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| if norm.is_empty() { String::new() } else { basename(norm) }).trim().to_lowercase();
    let n1 = pick(name1, &norm1);
    let n2 = pick(name2, &norm2);
    if !n1.is_empty() && !n2.is_empty() {
        if n1 == n2 {
            return true;
        }
        if !norm1.is_empty() && (norm1 == n2 || norm1.ends_with(&format!("/{n2}"))) {
            return true;
        }
        if !norm2.is_empty() && (norm2 == n1 || norm2.ends_with(&format!("/{n1}"))) {
            return true;
        }
    }
    false
}

macro_rules! re {
    ($name:ident, $pat:expr) => {
        fn $name() -> &'static Regex {
            static R: OnceLock<Regex> = OnceLock::new();
            R.get_or_init(|| Regex::new($pat).unwrap())
        }
    };
}

re!(blob_path, r"([a-zA-Z]:(?:\\\\|/)[A-Za-z0-9_.\-\\/ ]+)");
re!(uri_line, r#"([a-zA-Z]:[^\r\n"'>]+?)\s*->\s*[^\r\n]+"#);
re!(cwd_field, r#""[Cc]wd"\s*:\s*(?:"\\?"|")([^"\r\n]+?)(?:\\?"|")"#);
re!(doc_line, r#"(?:Active Document|Other open documents: -)\s*[:\-]?\s*([A-Za-z]:[^\r\n"()]+)"#);
re!(user_request, r"<USER_REQUEST>([\s\S]*?)</USER_REQUEST>");
re!(clean_name, r"^.*?-([A-Za-z0-9_\-\s]+)$");
re!(rollout_id, r"([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\.jsonl$");

pub type Ws = (Option<String>, Option<String>);

fn ws_of(path: String) -> Ws {
    let name = basename(&path);
    (Some(if name.is_empty() { path.clone() } else { name }), Some(path))
}

/// Pull a workspace out of a binary blob or log string. Nothing found stays empty:
/// filling in the focused window's workspace would file the session under the wrong project.
fn workspace_from_blob(buf: &[u8]) -> Ws {
    let text = String::from_utf8_lossy(buf);
    // Anything an agent read ends up in here, so bound the work
    for m in blob_path().find_iter(&text).take(200) {
        if js_len(m.as_str()) > 260 {
            continue; // longer than any real Windows path
        }
        let mut clean = m.as_str().replace("\\\\", "\\").trim().to_string();
        let mut i = 0;
        while i < 40 && js_len(&clean) > 3 && !exists(&clean) {
            clean = dirname(&clean);
            i += 1;
        }
        let lower = clean.to_lowercase();
        if js_len(&clean) > 3 && exists(&clean) && !lower.contains("appdata") && !lower.contains("temp") {
            return ws_of(clean);
        }
    }
    (None, None)
}

pub fn antigravity_workspace(log: &Path, known: &[String]) -> Ws {
    let fallback = (Some("Antigravity Workspace".to_string()), None);
    if !log.exists() {
        return (Some("Antigravity Session".to_string()), None);
    }
    // The head holds the system prompt and environment, the tail the latest Cwd and active document
    let Ok(mut f) = fs::File::open(log) else { return fallback };
    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
    const HEAD: u64 = 524_288;
    let mut head = vec![0u8; HEAD.min(size) as usize];
    if f.read_exact(&mut head).is_err() {
        return fallback;
    }
    let mut content = String::from_utf8_lossy(&head).into_owned();
    if size > HEAD {
        let tail_size = 65_536u64.min(size - HEAD);
        let mut tail = vec![0u8; tail_size as usize];
        if f.seek(SeekFrom::Start(size - tail_size)).is_ok() && f.read_exact(&mut tail).is_ok() {
            content.push('\n');
            content.push_str(&String::from_utf8_lossy(&tail));
        }
    }
    if let Some(m) = uri_line().captures(&content) {
        let target = m[1].replace("\\\\", "\\").trim().to_string();
        if js_len(&target) > 3 && exists(&target) {
            return ws_of(target);
        }
    }
    if let Some(m) = cwd_field().captures(&content) {
        let target = m[1].trim_matches(['\\', '"']).replace("\\\\", "\\").trim().to_string();
        if js_len(&target) > 3 && exists(&target) {
            return ws_of(target);
        }
    }
    if let Some(m) = doc_line().captures(&content) {
        let mut dir = dirname(m[1].replace("\\\\", "\\").trim());
        while js_len(&dir) > 3 && !exists(&dir) {
            dir = dirname(&dir);
        }
        if js_len(&dir) > 3 && exists(&dir) {
            return ws_of(dir);
        }
    }
    // Last resort: the log mentions a known workspace. Longest first, so a parent does not take a child project's session.
    let norm_content = content.replace("\\\\", "/").replace('\\', "/").to_lowercase();
    let mut sorted: Vec<&String> = known.iter().filter(|k| !k.is_empty()).collect();
    sorted.sort_by_key(|k| std::cmp::Reverse(js_len(k)));
    for ws in sorted {
        if norm_content.contains(&normalize_path(ws)) {
            return (Some(basename(ws)), Some(ws.clone()));
        }
    }
    fallback
}

/// Claude turns every non-alphanumeric into '-', one per UTF-16 unit
pub fn normalize_for_claude(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else {
            for _ in 0..c.len_utf16() {
                out.push('-');
            }
        }
    }
    out
}

pub fn claude_workspace(dir_name: &str, cwd: Option<&str>) -> Ws {
    if let Some(c) = cwd.map(str::trim).filter(|c| !c.is_empty()) {
        if exists(c) {
            return ws_of(c.to_string());
        }
    }
    let b = dir_name.as_bytes();
    if b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b'-' && b[2] == b'-' {
        let drive = format!("{}:\\", dir_name[..1].to_uppercase());
        let mut rest = dir_name[3..].to_string();
        // Walk down from the drive root, matching each level by its encoded name
        let mut current = PathBuf::from(&drive);
        while !rest.is_empty() {
            let Ok(rd) = fs::read_dir(&current) else { break };
            let mut names: Vec<String> = rd.flatten().filter(|e| e.file_type().map(|t| t.is_dir() || t.is_symlink()).unwrap_or(false)).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            names.sort_by_key(|n| std::cmp::Reverse(js_len(n)));
            let lower = rest.to_lowercase();
            let mut matched: Option<(String, usize)> = None;
            for name in names {
                let norm = normalize_for_claude(&name).to_lowercase();
                if lower == norm {
                    matched = Some((name, rest.len()));
                    break;
                }
                if lower.starts_with(&format!("{norm}-")) && norm.len() > matched.as_ref().map(|m| m.1).unwrap_or(0) {
                    matched = Some((name, norm.len() + 1));
                }
            }
            let Some((name, len)) = matched else { break };
            current = current.join(name);
            rest = rest[len..].to_string();
        }
        // Only a full match is the project; a halfway parent is not
        if rest.is_empty() && current.exists() {
            return ws_of(p(&current));
        }
        let direct = format!("{drive}{}", dir_name[3..].split("--").collect::<Vec<_>>().join("\\"));
        if exists(&direct) {
            return ws_of(direct);
        }
        let dashed = format!("{drive}{}", dir_name[3..].replace("---", " - ").split("--").collect::<Vec<_>>().join("\\"));
        if exists(&dashed) {
            return ws_of(dashed);
        }
    }
    let last = dir_name.split("--").last().filter(|s| !s.is_empty()).unwrap_or(dir_name);
    let cleaned = clean_name().replace(last, "$1").replace('-', " ");
    let cleaned = if cleaned.is_empty() { last.to_string() } else { cleaned };
    (Some(if cleaned.is_empty() { dir_name.to_string() } else { cleaned }), None)
}

fn status_of(age_ms: f64) -> &'static str {
    if age_ms < 5.0 * 60_000.0 {
        "active"
    } else if age_ms < 30.0 * 60_000.0 {
        "idle"
    } else {
        "completed"
    }
}

fn opt(v: &Option<String>) -> Value {
    v.as_ref().map(|s| json!(s)).unwrap_or(Value::Null)
}

fn pct(part: u64, total: u64, fallback: i64) -> i64 {
    if total > 0 {
        (part as f64 / total as f64 * 100.0).round() as i64
    } else {
        fallback
    }
}

struct Breakdown {
    prompt: u64,
    tool: u64,
    comp: u64,
    labels: [&'static str; 3],
    fallback: (i64, i64),
}

fn session(id: &str, agent: &str, title: &str, status: &str, start_ms: f64, last_ms: f64, model: &str, ws: &Ws, b: Breakdown) -> Value {
    let total = b.prompt + b.tool + b.comp;
    let p1 = pct(b.prompt, total, b.fallback.0);
    let p2 = pct(b.tool, total, b.fallback.1);
    let mut v = json!({
        "id": id, "agent": agent, "title": title, "status": status,
        "startTime": iso(start_ms), "lastActiveTime": iso(last_ms),
        "totalTokens": total, "model": model,
    });
    let o = v.as_object_mut().unwrap();
    if ws.0.is_some() {
        o.insert("workspace".into(), opt(&ws.0));
    }
    if ws.1.is_some() {
        o.insert("workspacePath".into(), opt(&ws.1));
    }
    o.insert(
        "tokenBreakdown".into(),
        json!({
            "promptTokens": b.prompt, "toolReadTokens": b.tool, "completionTokens": b.comp,
            "details": [
                { "category": b.labels[0], "tokens": b.prompt, "percentage": p1 },
                { "category": b.labels[1], "tokens": b.tool, "percentage": p2 },
                { "category": b.labels[2], "tokens": b.comp, "percentage": (100 - p1 - p2).max(0) },
            ]
        }),
    );
    v
}

#[derive(Clone)]
struct Cached {
    mtime: f64,
    session: Value,
}

/// `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`, id from the name, newest first
pub fn find_codex_rollouts(max: usize) -> Vec<(String, PathBuf, f64)> {
    fn walk(dir: &Path, depth: u32, out: &mut Vec<(String, PathBuf, f64)>) {
        if depth > 4 {
            return;
        }
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let Ok(t) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            if t.is_dir() {
                walk(&e.path(), depth + 1, out);
            } else if t.is_file() && name.starts_with("rollout-") && name.ends_with(".jsonl") {
                if let (Some(m), Some(mt)) = (rollout_id().captures(&name), mtime_ms(&e.path())) {
                    out.push((m[1].to_string(), e.path(), mt));
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(&home().join(".codex").join("sessions"), 0, &mut out);
    out.sort_by(|a, b| b.2.total_cmp(&a.2));
    out.truncate(max);
    out
}

pub struct Scanner {
    dir: PathBuf,
    pub usage: UsageCache,
    /// Serializes scans; the cache sits behind it
    cache: Mutex<Option<HashMap<String, Cached>>>,
    dirty: Mutex<bool>,
}

fn max_mtime(paths: impl IntoIterator<Item = PathBuf>) -> f64 {
    paths.into_iter().filter_map(|x| mtime_ms(&x)).fold(0.0, f64::max)
}

/// Everything that moves while an Antigravity session is working: logs, SQLite WAL, the presence lock, tasks and messages
fn agy_session_mtime(brain_path: &Path, id: &str, brain_dir: &Path) -> f64 {
    let logs = brain_path.join(".system_generated").join("logs");
    let root = brain_dir.parent().unwrap_or(brain_dir);
    let conv = root.join("conversations");
    let mut paths = vec![
        logs.join("transcript.jsonl"),
        logs.join("transcript_full.jsonl"),
        conv.join(format!("{id}.db")),
        conv.join(format!("{id}.db-wal")),
        root.join("presence").join(format!("{id}.lock")),
    ];
    for sub in ["tasks", "messages"] {
        let dir = brain_path.join(".system_generated").join(sub);
        if let Ok(rd) = fs::read_dir(&dir) {
            paths.push(dir);
            let mut names: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            names.sort();
            let skip = names.len().saturating_sub(5);
            paths.extend(names.into_iter().skip(skip));
        }
    }
    max_mtime(paths)
}

impl Scanner {
    pub fn new(dir: PathBuf) -> Self {
        Self { usage: UsageCache::new(dir.clone()), dir, cache: Mutex::new(None), dirty: Mutex::new(false) }
    }

    fn load<'a>(&self, slot: &'a mut Option<HashMap<String, Cached>>) -> &'a mut HashMap<String, Cached> {
        slot.get_or_insert_with(|| {
            let mut m = HashMap::new();
            if let Some(data) = read_text(&self.dir.join("dashboard-cache.json")).and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
                if data.get("version").and_then(Value::as_u64) == Some(3) {
                    if let Some(obj) = data.get("sessions").and_then(Value::as_object) {
                        for (k, v) in obj {
                            if let (Some(mtime), Some(session)) = (v.get("mtime").and_then(Value::as_f64), v.get("session")) {
                                m.insert(k.clone(), Cached { mtime, session: session.clone() });
                            }
                        }
                    }
                }
            }
            m
        })
    }

    pub fn invalidate(&self) {
        *self.cache.lock().unwrap() = None;
    }

    fn cached_get(&self, key: &str, mtime: f64) -> Option<Value> {
        let mut g = self.cache.lock().unwrap();
        self.load(&mut g).get(key).filter(|c| c.mtime == mtime).map(|c| c.session.clone())
    }

    fn cached_put(&self, key: String, mtime: f64, session: &Value) {
        let mut g = self.cache.lock().unwrap();
        self.load(&mut g).insert(key, Cached { mtime, session: session.clone() });
        *self.dirty.lock().unwrap() = true;
    }

    fn save(&self) {
        let mut dirty = self.dirty.lock().unwrap();
        if !*dirty {
            return;
        }
        let g = self.cache.lock().unwrap();
        let Some(c) = g.as_ref() else { return };
        let mut sessions = serde_json::Map::new();
        for (k, v) in c {
            sessions.insert(k.clone(), json!({ "mtime": v.mtime, "session": v.session }));
        }
        if fs::write(self.dir.join("dashboard-cache.json"), json!({ "version": 3, "sessions": sessions }).to_string()).is_ok() {
            *dirty = false;
        }
    }

    /// (workspacePath, mtime) of every cached session, newest first: used to suggest recent projects
    pub fn cached_entries(&self) -> Vec<(f64, Value)> {
        let mut g = self.cache.lock().unwrap();
        let mut v: Vec<(f64, Value)> = self.load(&mut g).values().map(|c| (c.mtime, c.session.clone())).collect();
        v.sort_by(|a, b| b.0.total_cmp(&a.0));
        v
    }

    /// Everything in the cache, or None right after it was cleared
    pub fn cached_sessions(&self) -> Option<Vec<Value>> {
        self.cache.lock().unwrap().as_ref().map(|c| c.values().map(|e| e.session.clone()).collect())
    }

    fn scan_antigravity(&self, max: usize, known: &[String]) -> Vec<Value> {
        let h = home();
        let mut all: Vec<(String, PathBuf, f64)> = Vec::new();
        for base in AGY_DIRS {
            let brain = h.join(".gemini").join(base).join("brain");
            let Ok(rd) = fs::read_dir(&brain) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) || name.starts_with('.') || name == "tempmediaStorage" {
                    continue;
                }
                let mtime = agy_session_mtime(&e.path(), &name, &brain);
                all.push((name, e.path(), mtime));
            }
        }
        all.sort_by(|a, b| b.2.total_cmp(&a.2));
        all.truncate(max);
        let mut list = Vec::new();
        for (name, path, dir_mtime) in all {
            let log = path.join(".system_generated").join("logs").join("transcript.jsonl");
            let Some(log_mtime) = mtime_ms(&log) else { continue };
            let last = if dir_mtime != 0.0 { dir_mtime } else { log_mtime };
            let key = p(&log);
            if let Some(mut s) = self.cached_get(&key, last) {
                s["status"] = json!(status_of(now_ms() - last));
                s["lastActiveTime"] = json!(iso(last));
                list.push(s);
                continue;
            }
            let mut title = "Antigravity Session".to_string();
            let (mut prompt, mut comp) = (0, 0);
            let ws = antigravity_workspace(&log, known);
            if let Some(content) = read_text(&log) {
                for line in content.lines() {
                    let Some(row) = parse_json(line) else { continue };
                    if sv(&row, "type") == "USER_INPUT" && !sv(&row, "content").is_empty() {
                        // Only the first line is used, so 64 KB of it is plenty
                        if let Some(m) = user_request().captures(&js_prefix(sv(&row, "content"), 65_536)) {
                            title = js_prefix(first_line(m[1].trim()), 45);
                        }
                    }
                }
                let b = sum_days(&self.usage.file_days(Agent::Antigravity, &log, Some(&content)), "");
                prompt = b.input;
                comp = b.output;
            }
            let s = session(
                &name,
                "antigravity",
                &title,
                status_of(now_ms() - last),
                (last - 180_000.0).max(0.0),
                last,
                "Gemini 3.8 Flash",
                &ws,
                Breakdown { prompt, tool: 0, comp, labels: ["Input & Tool Output (estimated)", "Cache Read", "Model Output (estimated)"], fallback: (70, 18) },
            );
            self.cached_put(key, last, &s);
            list.push(s);
        }
        list
    }

    /// The CLI's own store: `agy --conversation <id>` understands these ids, which never overlap the IDE panel's brain ids.
    /// The .db is a protobuf blob with no token fields, so tokens come from brain transcripts only.
    fn scan_antigravity_cli(&self, max: usize) -> Vec<Value> {
        let h = home();
        let mut seen = HashSet::new();
        let mut files: Vec<(String, PathBuf, f64)> = Vec::new();
        for base in ["antigravity-cli", "antigravity-ide", "antigravity"] {
            let dir = h.join(".gemini").join(base).join("conversations");
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            let mut batch: Vec<(String, PathBuf, f64)> = Vec::new();
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if !e.file_type().map(|t| t.is_file()).unwrap_or(false) || !name.ends_with(".db") {
                    continue;
                }
                let id = name[..name.len() - 3].to_string();
                let path = e.path();
                let wal = PathBuf::from(format!("{}-wal", path.display()));
                let lock = dir.parent().unwrap_or(&dir).join("presence").join(format!("{id}.lock"));
                let mtime = max_mtime([path.clone(), wal, lock]);
                batch.push((id, path, mtime));
            }
            for f in batch {
                if seen.insert(f.0.clone()) {
                    files.push(f);
                }
            }
        }
        files.sort_by(|a, b| b.2.total_cmp(&a.2));
        files.truncate(max);
        let mut list = Vec::new();
        for (id, path, mtime) in files {
            let last = if mtime != 0.0 { mtime } else { now_ms() };
            let status = status_of(now_ms() - mtime);
            let key = p(&path);
            if let Some(mut s) = self.cached_get(&key, mtime) {
                s["status"] = json!(status);
                s["lastActiveTime"] = json!(iso(last));
                list.push(s);
                continue;
            }
            let ws = fs::read(&path).map(|b| workspace_from_blob(&b)).unwrap_or((None, None));
            let title = match &ws.0 {
                Some(n) => format!("Antigravity Session ({n})"),
                None => format!("Antigravity Session ({})", js_prefix(&id, 8)),
            };
            let mut v = json!({
                "id": id, "agent": "antigravity", "title": title, "status": status,
                "startTime": iso((mtime - 180_000.0).max(0.0)), "lastActiveTime": iso(last),
                "totalTokens": 0, "model": "Gemini 3.8 Flash",
            });
            let o = v.as_object_mut().unwrap();
            if ws.0.is_some() {
                o.insert("workspace".into(), opt(&ws.0));
            }
            if ws.1.is_some() {
                o.insert("workspacePath".into(), opt(&ws.1));
            }
            o.insert(
                "tokenBreakdown".into(),
                json!({ "promptTokens": 0, "toolReadTokens": 0, "completionTokens": 0, "details": [{ "category": "Unavailable (no transcript)", "tokens": 0, "percentage": 0 }] }),
            );
            self.cached_put(key, mtime, &v);
            list.push(v);
        }
        list
    }

    fn scan_claude(&self, max: usize) -> Vec<Value> {
        let projects = home().join(".claude").join("projects");
        let Ok(rd) = fs::read_dir(&projects) else { return Vec::new() };
        let mut list = Vec::new();
        // Every project: a folder's mtime does not follow an append on Windows, so sorting folders by time misses live sessions
        for td in rd.flatten() {
            if !td.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let dir_name = td.file_name().to_string_lossy().into_owned();
            let Ok(files) = fs::read_dir(td.path()) else { continue };
            let mut files: Vec<(String, PathBuf, f64)> = files
                .flatten()
                .filter(|f| f.file_type().map(|t| t.is_file()).unwrap_or(false) && f.file_name().to_string_lossy().ends_with(".jsonl"))
                .filter_map(|f| mtime_ms(&f.path()).map(|m| (f.file_name().to_string_lossy().into_owned(), f.path(), m)))
                .collect();
            files.sort_by(|a, b| b.2.total_cmp(&a.2));
            files.truncate(max);
            for (name, path, mtime) in files {
                let key = p(&path);
                if let Some(mut s) = self.cached_get(&key, mtime) {
                    let wp = s.get("workspacePath").and_then(Value::as_str).map(str::to_string);
                    if !wp.as_deref().map(exists).unwrap_or(false) {
                        let (name2, path2) = claude_workspace(&dir_name, wp.as_deref());
                        if let Some(path2) = path2 {
                            s["workspacePath"] = json!(path2);
                            s["workspace"] = opt(&name2);
                            self.cached_put(key.clone(), mtime, &s);
                        }
                    }
                    s["status"] = json!(status_of(now_ms() - mtime));
                    s["lastActiveTime"] = json!(iso(mtime));
                    list.push(s);
                    continue;
                }
                let mut title = "Claude Session".to_string();
                let (mut prompt, mut tool, mut comp) = (0, 0, 0);
                let mut model = "Claude 3.7 Sonnet".to_string();
                let mut cwd: Option<String> = None;
                if let Some(content) = read_text(&path) {
                    for line in content.lines() {
                        let Some(row) = parse_json(line) else { continue };
                        if cwd.is_none() {
                            if let Some(c) = row.get("cwd").and_then(Value::as_str).filter(|c| !c.is_empty()) {
                                cwd = Some(c.to_string());
                            }
                        }
                        if sv(&row, "type") == "user" && title == "Claude Session" {
                            let text = match row.pointer("/message/content") {
                                Some(Value::String(s)) => s.clone(),
                                Some(Value::Array(a)) => a.iter().map(|c| sv(c, "text")).collect::<Vec<_>>().join(" "),
                                _ => String::new(),
                            };
                            if !text.is_empty() {
                                title = js_prefix(first_line(&text), 45);
                            }
                        }
                        if row.pointer("/attachment/type").and_then(Value::as_str) == Some("model") {
                            if let Some(m) = row.pointer("/attachment/identity/marketingName").and_then(Value::as_str).filter(|m| !m.is_empty()) {
                                model = m.to_string();
                            }
                        }
                    }
                    let b = sum_days(&self.usage.file_days(Agent::Claude, &path, Some(&content)), "");
                    (prompt, tool, comp) = (b.input, b.cache_read, b.output);
                }
                let ws = claude_workspace(&dir_name, cwd.as_deref());
                let s = session(
                    name.trim_end_matches(".jsonl"),
                    "claude",
                    &title,
                    status_of(now_ms() - mtime),
                    mtime - 300_000.0,
                    mtime,
                    &model,
                    &ws,
                    Breakdown { prompt, tool, comp, labels: ["Input & Cache Write", "Cache Read", "Output"], fallback: (75, 15) },
                );
                self.cached_put(key, mtime, &s);
                list.push(s);
            }
        }
        list
    }

    /// Codex titles live in an append-only index: the last row for an id is the newest title
    fn codex_titles() -> HashMap<String, String> {
        let mut map = HashMap::new();
        if let Some(content) = read_text(&home().join(".codex").join("session_index.jsonl")) {
            for line in content.lines() {
                if let Some(row) = parse_json(line) {
                    if !sv(&row, "id").is_empty() && !sv(&row, "thread_name").is_empty() {
                        map.insert(sv(&row, "id").to_string(), sv(&row, "thread_name").to_string());
                    }
                }
            }
        }
        map
    }

    fn scan_codex(&self, max: usize) -> Vec<Value> {
        if !home().join(".codex").join("sessions").exists() {
            return Vec::new();
        }
        let titles = Self::codex_titles();
        let mut list = Vec::new();
        for (id, path, mtime) in find_codex_rollouts(max) {
            let key = p(&path);
            let index_title = titles.get(&id);
            if let Some(mut s) = self.cached_get(&key, mtime) {
                if let Some(t) = index_title {
                    s["title"] = json!(t);
                }
                s["status"] = json!(status_of(now_ms() - mtime));
                s["lastActiveTime"] = json!(iso(mtime));
                list.push(s);
                continue;
            }
            let mut title = index_title.cloned().unwrap_or_else(|| "Codex Session".to_string());
            let (mut prompt, mut cached, mut comp) = (0, 0, 0);
            let mut model = "Codex CLI".to_string();
            let mut ws: Ws = (None, None);
            if let Some(content) = read_text(&path) {
                for line in content.lines() {
                    let Some(row) = parse_json(line) else { continue };
                    if sv(&row, "type") == "session_meta" {
                        if let Some(cwd) = row.pointer("/payload/cwd").and_then(Value::as_str).filter(|c| !c.is_empty()) {
                            ws = ws_of(cwd.to_string());
                        }
                    }
                    if index_title.is_none() && sv(&row, "type") == "response_item" && row.pointer("/payload/role").and_then(Value::as_str) == Some("user") {
                        if let Some(parts) = row.pointer("/payload/content").and_then(Value::as_array) {
                            let found = parts.iter().find(|c| sv(c, "type") == "input_text" && !sv(c, "text").is_empty() && !sv(c, "text").trim_start().starts_with('<'));
                            if let Some(f) = found {
                                title = js_prefix(first_line(sv(f, "text").trim()), 40);
                            }
                        }
                    }
                    if let Some(m) = row.pointer("/payload/base_instructions/provenance/model").and_then(Value::as_str).filter(|m| !m.is_empty()) {
                        model = m.to_string();
                    }
                }
                let b = sum_days(&self.usage.file_days(Agent::Codex, &path, Some(&content)), "");
                (prompt, cached, comp) = (b.input, b.cache_read, b.output);
            }
            let s = session(
                &id,
                "codex",
                &title,
                status_of(now_ms() - mtime),
                mtime - 300_000.0,
                mtime,
                &model,
                &ws,
                Breakdown { prompt, tool: cached, comp, labels: ["Input (non-cached)", "Cache Read", "Output"], fallback: (0, 0) },
            );
            self.cached_put(key, mtime, &s);
            list.push(s);
        }
        list
    }

    /// All the file work behind dashboard:data. A disabled CLI's sessions are not listed.
    pub fn scan_dashboard(&self, claude: bool, codex: bool, known: &[String]) -> Value {
        let agy_brain = self.scan_antigravity(30, known);
        let agy_cli = self.scan_antigravity_cli(30);
        let claude_list = if claude { self.scan_claude(20) } else { Vec::new() };
        let codex_list = if codex { self.scan_codex(20) } else { Vec::new() };
        let usage = self.usage.scan_all();
        self.save();
        json!({ "agyBrain": agy_brain, "agyCli": agy_cli, "claude": claude_list, "codex": codex_list, "usage": usage })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_workspace() {
        assert!(is_same_workspace(Some(r"C:\a\b"), None, Some("c:/A/b/"), None));
        assert!(is_same_workspace(None, Some("B"), Some(r"C:\a\b"), None));
        assert!(!is_same_workspace(Some(r"C:\a\b"), None, Some(r"C:\a\c"), None));
    }

    #[test]
    fn encoded_names() {
        assert_eq!(normalize_for_claude("Claude Agent - Personal"), "Claude-Agent---Personal");
        assert_eq!(normalize_for_claude("a\u{1F600}"), "a--");
    }

    #[test]
    fn fallback_workspace_name() {
        assert_eq!(claude_workspace("no-such-thing", None).0.unwrap(), "such thing");
    }
}
