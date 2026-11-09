// Token usage. Per-file daily buckets, cached by mtime + size so the totals do not change between opens.
// The cache file is shared with installs that ran the previous backend.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Days as ChronoDays, Local, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::util::{home, js_len, mtime_ms, nv, p, parse_date, parse_json, read_text};

#[derive(Clone, Copy, Default, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    pub input: u64,
    pub cache_read: u64,
    pub output: u64,
}

pub type Days = BTreeMap<String, Bucket>;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Agent {
    Claude,
    Codex,
    Antigravity,
}

impl Agent {
    pub const ALL: [Agent; 3] = [Agent::Claude, Agent::Codex, Agent::Antigravity];
    pub fn id(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Antigravity => "antigravity",
        }
    }
}

/// Local-time YYYY-MM-DD: 1d is today, 7d is today and the six days before
pub fn day_key(v: &Value) -> String {
    parse_date(v).map(|d: DateTime<Utc>| d.with_timezone(&Local).format("%Y-%m-%d").to_string()).unwrap_or_default()
}

pub fn add(days: &mut Days, day: String, b: Bucket) {
    if day.is_empty() {
        return;
    }
    let cur = days.entry(day).or_default();
    cur.input += b.input;
    cur.cache_read += b.cache_read;
    cur.output += b.output;
}

pub fn sum_days(days: &Days, since: &str) -> Bucket {
    let mut out = Bucket::default();
    for (day, b) in days {
        if day.as_str() < since {
            continue;
        }
        out.input += b.input;
        out.cache_read += b.cache_read;
        out.output += b.output;
    }
    out
}

fn each_json_line(content: &str, must_include: &str, mut f: impl FnMut(&Value)) {
    for line in content.lines() {
        // Filter first: a Claude log can be 142 MB and most lines have no usage
        if line.is_empty() || !line.contains(must_include) {
            continue;
        }
        if let Some(v) = parse_json(line) {
            f(&v);
        }
    }
}

/// Claude writes real usage per API call. One reply spans several lines with the same usage, so dedupe by message id.
/// input = input + cache writes; cache reads are counted apart.
pub fn claude_days(content: &str) -> Days {
    let mut days = Days::new();
    let mut seen = HashSet::new();
    each_json_line(content, "\"usage\"", |r| {
        let Some(msg) = r.get("message") else { return };
        let Some(u) = msg.get("usage").filter(|u| !u.is_null()) else { return };
        let id = msg.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()).or(r.get("uuid").and_then(Value::as_str).filter(|s| !s.is_empty()));
        if let Some(id) = id {
            if !seen.insert(id.to_string()) {
                return;
            }
        }
        add(
            &mut days,
            day_key(r.get("timestamp").unwrap_or(&Value::Null)),
            Bucket {
                input: nv(u, "input_tokens") + nv(u, "cache_creation_input_tokens"),
                cache_read: nv(u, "cache_read_input_tokens"),
                output: nv(u, "output_tokens"),
            },
        );
    });
    days
}

/// Codex token_count events carry running totals, so take differences. input_tokens already includes the cached part.
pub fn codex_days(content: &str) -> Days {
    let mut days = Days::new();
    let (mut prev_in, mut prev_cached, mut prev_out) = (0u64, 0u64, 0u64);
    each_json_line(content, "total_token_usage", |r| {
        let Some(u) = r.pointer("/payload/info/total_token_usage").filter(|u| !u.is_null()) else { return };
        let (mut input, mut cached, mut output) = (nv(u, "input_tokens"), nv(u, "cached_input_tokens"), nv(u, "output_tokens"));
        // A resumed or compacted session zeroes the detail and keeps total_tokens
        if input == 0 && output == 0 && nv(u, "total_tokens") > 0 {
            input = nv(u, "total_tokens");
            cached = 0;
            output = 0;
        }
        let d_in = input.saturating_sub(prev_in);
        let d_cached = cached.saturating_sub(prev_cached);
        add(
            &mut days,
            day_key(r.get("timestamp").unwrap_or(&Value::Null)),
            Bucket { input: d_in.saturating_sub(d_cached), cache_read: d_cached, output: output.saturating_sub(prev_out) },
        );
        (prev_in, prev_cached, prev_out) = (input, cached, output);
    });
    days
}

/// Antigravity logs have no token fields, so estimate 3.5 chars per token. Resent context is invisible: this reads low.
pub fn antigravity_days(content: &str) -> Days {
    let mut days = Days::new();
    let est = |v: Option<&Value>| v.and_then(Value::as_str).map(|s| (js_len(s) as f64 / 3.5).ceil() as u64).unwrap_or(0);
    each_json_line(content, "\"type\"", |r| {
        let day = day_key(r.get("created_at").unwrap_or(&Value::Null));
        let ty = r.get("type").and_then(Value::as_str).unwrap_or("");
        if r.get("source").and_then(Value::as_str) == Some("MODEL") && ty == "PLANNER_RESPONSE" {
            add(&mut days, day, Bucket { output: est(r.get("thinking")) + est(r.get("content")), ..Default::default() });
        } else if ["USER_INPUT", "RUN_COMMAND", "VIEW_FILE", "SYSTEM_MESSAGE"].contains(&ty) {
            add(&mut days, day, Bucket { input: est(r.get("content")), ..Default::default() });
        }
    });
    days
}

fn list_files(dir: &Path, want: &dyn Fn(&str) -> bool, depth: u32) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else { return out };
    for e in entries.flatten() {
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() && depth > 0 {
            out.extend(list_files(&e.path(), want, depth - 1));
        } else if t.is_file() && want(&e.file_name().to_string_lossy()) {
            out.push(e.path());
        }
    }
    out
}

pub const AGY_DIRS: [&str; 3] = ["antigravity-ide", "antigravity-cli", "antigravity"];

fn usage_files(agent: Agent) -> Vec<PathBuf> {
    let h = home();
    match agent {
        Agent::Claude => list_files(&h.join(".claude").join("projects"), &|n| n.ends_with(".jsonl"), 1),
        Agent::Codex => list_files(&h.join(".codex").join("sessions"), &|n| n.starts_with("rollout-") && n.ends_with(".jsonl"), 4),
        Agent::Antigravity => {
            let mut out = Vec::new();
            for base in AGY_DIRS {
                let Ok(ids) = fs::read_dir(h.join(".gemini").join(base).join("brain")) else { continue };
                for id in ids.flatten() {
                    let t = id.path().join(".system_generated").join("logs").join("transcript.jsonl");
                    if t.exists() {
                        out.push(t);
                    }
                }
            }
            out
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    mtime: f64,
    size: u64,
    days: Days,
}

const CACHE_VERSION: u64 = 1;
/// A larger log is skipped and counted as zero, as it was before: the risk was memory, and it is cached so it is not reread.
const MAX_LOG: u64 = 64 * 1024 * 1024;

#[derive(Default)]
struct CacheState {
    files: Option<HashMap<String, Entry>>,
    dirty: bool,
}

pub struct UsageCache {
    dir: PathBuf,
    state: Mutex<CacheState>,
}

impl UsageCache {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, state: Mutex::new(CacheState::default()) }
    }

    fn with<R>(&self, f: impl FnOnce(&mut HashMap<String, Entry>, &mut bool) -> R) -> R {
        let mut guard = self.state.lock().unwrap();
        let s = &mut *guard;
        let files = s.files.get_or_insert_with(|| {
            let mut m = HashMap::new();
            if let Some(data) = read_text(&self.dir.join("usage-cache.json")).and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
                if data.get("version").and_then(Value::as_u64) == Some(CACHE_VERSION) {
                    if let Some(obj) = data.get("files").and_then(Value::as_object) {
                        for (k, v) in obj {
                            if let Ok(e) = serde_json::from_value::<Entry>(v.clone()) {
                                m.insert(k.clone(), e);
                            }
                        }
                    }
                }
            }
            m
        });
        f(files, &mut s.dirty)
    }

    fn save(&self) {
        let mut guard = self.state.lock().unwrap();
        let s = &mut *guard;
        if !s.dirty {
            return;
        }
        let Some(files) = &s.files else { return };
        let body = json!({ "version": CACHE_VERSION, "files": files });
        if fs::write(self.dir.join("usage-cache.json"), body.to_string()).is_ok() {
            s.dirty = false;
        }
    }

    /// One file's buckets. The session cards use this too, so their numbers add up to the totals.
    pub fn file_days(&self, agent: Agent, path: &Path, content: Option<&str>) -> Days {
        let Ok(meta) = fs::metadata(path) else { return Days::new() };
        let (Some(mtime), size) = (mtime_ms(path), meta.len()) else { return Days::new() };
        let key = p(path);
        if let Some(hit) = self.with(|c, _| c.get(&key).filter(|e| e.mtime == mtime && e.size == size).map(|e| e.days.clone())) {
            return hit;
        }
        let days = if size > MAX_LOG {
            Days::new()
        } else {
            let owned;
            let text = match content {
                Some(t) => Some(t),
                None => {
                    owned = read_text(path);
                    owned.as_deref()
                }
            };
            match (agent, text) {
                (Agent::Claude, Some(t)) => claude_days(t),
                (Agent::Codex, Some(t)) => codex_days(t),
                (Agent::Antigravity, Some(t)) => antigravity_days(t),
                (_, None) => Days::new(),
            }
        };
        self.with(|c, dirty| {
            c.insert(key, Entry { mtime, size, days: days.clone() });
            *dirty = true;
        });
        days
    }

    /// {claude: {all, 30d, 7d, 1d}, codex: ..., antigravity: ...}: full history, cut by daily buckets
    pub fn scan_all(&self) -> Value {
        let today = Local::now().date_naive();
        let since = |n: u64| (today - ChronoDays::new(n - 1)).format("%Y-%m-%d").to_string();
        let mut out = serde_json::Map::new();
        for agent in Agent::ALL {
            let mut merged = Days::new();
            for f in usage_files(agent) {
                for (day, b) in self.file_days(agent, &f, None) {
                    add(&mut merged, day, b);
                }
            }
            out.insert(
                agent.id().to_string(),
                json!({
                    "all": sum_days(&merged, ""),
                    "30d": sum_days(&merged, &since(30)),
                    "7d": sum_days(&merged, &since(7)),
                    "1d": sum_days(&merged, &since(1)),
                }),
            );
        }
        self.save();
        Value::Object(out)
    }

    pub fn flush(&self) {
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_dedupes_by_message_id() {
        let log = r#"{"timestamp":"2026-03-01T12:00:00Z","message":{"id":"a","usage":{"input_tokens":1,"cache_creation_input_tokens":2,"cache_read_input_tokens":3,"output_tokens":4}}}
{"timestamp":"2026-03-01T12:00:01Z","message":{"id":"a","usage":{"input_tokens":1,"cache_creation_input_tokens":2,"cache_read_input_tokens":3,"output_tokens":4}}}
not json "usage"
"#;
        let t = sum_days(&claude_days(log), "");
        assert_eq!(t, Bucket { input: 3, cache_read: 3, output: 4 });
    }

    #[test]
    fn codex_takes_differences() {
        let log = r#"{"timestamp":"2026-03-01T12:00:00Z","payload":{"info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":4,"output_tokens":2}}}}
{"timestamp":"2026-03-01T12:00:01Z","payload":{"info":{"total_token_usage":{"input_tokens":15,"cached_input_tokens":4,"output_tokens":3}}}}
"#;
        assert_eq!(sum_days(&codex_days(log), ""), Bucket { input: 11, cache_read: 4, output: 3 });
    }

    #[test]
    fn antigravity_counts_utf16_units() {
        let log = r#"{"type":"USER_INPUT","created_at":"2026-03-01T12:00:00Z","content":"abcdefg"}
{"type":"PLANNER_RESPONSE","source":"MODEL","created_at":"2026-03-01T12:00:00Z","content":"abc"}
"#;
        assert_eq!(sum_days(&antigravity_days(log), ""), Bucket { input: 2, cache_read: 0, output: 1 });
    }

    #[test]
    fn since_cuts_old_days() {
        let mut d = Days::new();
        add(&mut d, "2026-01-01".into(), Bucket { input: 1, ..Default::default() });
        add(&mut d, "2026-02-01".into(), Bucket { input: 2, ..Default::default() });
        assert_eq!(sum_days(&d, "2026-01-15").input, 2);
    }
}
