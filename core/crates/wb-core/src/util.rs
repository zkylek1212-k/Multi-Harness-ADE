// Things JavaScript did for free that the on-disk formats and the UI depend on.
use std::fs;
use std::io::Write;
use std::time::Duration;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::UNIX_EPOCH;

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use regex::{Regex, RegexBuilder};
use serde_json::Value;

pub fn home() -> PathBuf {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    PathBuf::from(std::env::var_os(var).unwrap_or_default())
}

pub fn p(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// path.basename: "" for a root
pub fn basename(path: &str) -> String {
    Path::new(path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// path.dirname: a root is its own parent
pub fn dirname(path: &str) -> String {
    Path::new(path).parent().map(p).filter(|s| !s.is_empty()).unwrap_or_else(|| path.to_string())
}

/// path.resolve: absolute, normalized, no trailing separator except on a root
pub fn resolve(path: &str) -> String {
    let abs = std::path::absolute(path).map(|b| p(&b)).unwrap_or_else(|_| path.to_string());
    let trimmed = abs.trim_end_matches(['\\', '/']);
    if trimmed.is_empty() || trimmed.ends_with(':') {
        abs
    } else {
        trimmed.to_string()
    }
}

pub fn exists(path: &str) -> bool {
    Path::new(path).exists()
}

/// fs.statSync(path).mtimeMs, same formula as Node so the existing caches stay valid
pub fn mtime_ms(path: &Path) -> Option<f64> {
    let d = fs::metadata(path).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some(d.as_secs() as f64 * 1000.0 + d.subsec_nanos() as f64 / 1e6)
}

pub fn now_ms() -> f64 {
    Utc::now().timestamp_millis() as f64
}

/// new Date(ms).toISOString()
pub fn iso(ms: f64) -> String {
    Utc.timestamp_millis_opt(ms as i64).single().unwrap_or_default().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// new Date(value).getTime() for the timestamp shapes the agents write. Date-only means UTC, as in JS.
pub fn parse_date(v: &Value) -> Option<DateTime<Utc>> {
    if let Some(n) = v.as_f64() {
        return Utc.timestamp_millis_opt(n as i64).single();
    }
    let s = v.as_str()?;
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Some(d.with_timezone(&Utc));
    }
    if let Ok(d) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f") {
        return Local.from_local_datetime(&d).single().map(|d| d.with_timezone(&Utc));
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?.and_hms_opt(0, 0, 0).map(|d| Utc.from_utc_datetime(&d))
}

pub fn date_ms(s: &str) -> f64 {
    parse_date(&Value::String(s.to_string())).map(|d| d.timestamp_millis() as f64).unwrap_or(f64::NAN)
}

/// String.prototype.slice(0, n): JS counts UTF-16 units. A split pair loses its half, which JS would keep as garbage.
pub fn js_prefix(s: &str, n: usize) -> String {
    let mut used = 0;
    let mut out = String::new();
    for c in s.chars() {
        used += c.len_utf16();
        if used > n {
            break;
        }
        out.push(c);
    }
    out
}

pub fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

pub fn first_line(s: &str) -> &str {
    s.split('\n').next().unwrap_or("").strip_suffix('\r').unwrap_or_else(|| s.split('\n').next().unwrap_or(""))
}

fn lone_surrogate() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // Pairs are consumed first so only a real lone half is replaced
    R.get_or_init(|| Regex::new(r"(?i)(\\\\)|(\\ud[89ab][0-9a-f]{2}\\ud[c-f][0-9a-f]{2})|(\\ud[89a-f][0-9a-f]{2})").unwrap())
}

/// JSON.parse accepts a lone surrogate escape, serde_json does not, and a truncated emoji in an agent log is normal.
pub fn parse_json(line: &str) -> Option<Value> {
    match serde_json::from_str(line) {
        Ok(v) => Some(v),
        Err(_) if line.contains("\\u") || line.contains("\\U") => {
            let fixed = lone_surrogate().replace_all(line, |c: &regex::Captures| if c.get(3).is_some() { "\\ufffd".to_string() } else { c[0].to_string() });
            serde_json::from_str(&fixed).ok()
        }
        Err(_) => None,
    }
}

pub fn read_text(path: &Path) -> Option<String> {
    fs::read(path).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// localeCompare with sensitivity base is close enough for names: case-insensitive, lowercase first on a tie
pub fn locale_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| b.cmp(a))
}

pub fn uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn sv<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

pub fn nv(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(Value::as_u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surrogates() {
        assert_eq!(parse_json(r#"{"a":"x\ud83d"}"#).unwrap()["a"], "x\u{fffd}");
        assert_eq!(parse_json(r#"{"a":"😀"}"#).unwrap()["a"], "\u{1F600}");
        assert_eq!(parse_json(r#"{"a":"\\ud83d"}"#).unwrap()["a"], "\\ud83d");
        assert!(parse_json("{oops").is_none());
    }

    #[test]
    fn js_strings() {
        assert_eq!(js_prefix("ab\u{1F600}c", 3), "ab");
        assert_eq!(js_prefix("ab\u{1F600}c", 4), "ab\u{1F600}");
        assert_eq!(first_line("a\r\nb"), "a");
    }

    #[test]
    fn dates() {
        assert_eq!(iso(0.0), "1970-01-01T00:00:00.000Z");
        assert_eq!(date_ms("2026-01-01T00:00:00.000Z"), 1767225600000.0);
        assert!(date_ms("nope").is_nan());
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

pub fn real_path(p: &str) -> String {
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

fn secret_patterns() -> &'static Vec<Regex> {
    static R: OnceLock<Vec<Regex>> = OnceLock::new();
    R.get_or_init(|| {
        vec![
            Regex::new(r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----").unwrap(),
            Regex::new(r"\b(?:ghp|gho|ghs|ghu|github_pat)_[A-Za-z0-9_]{16,}\b").unwrap(),
            Regex::new(r"\bsk-[A-Za-z0-9_-]{16,}\b").unwrap(),
            Regex::new(r"\bAKIA[0-9A-Z]{16}\b").unwrap(),
            Regex::new(r"\bxox[abprs]-[A-Za-z0-9-]{10,}\b").unwrap(),
            RegexBuilder::new(r#"((?:password|passwd|secret|token|api[_-]?key)["']?\s*[:=]\s*["']?)[^\s"']{6,}"#).case_insensitive(true).build().unwrap(),
        ]
    })
}

/// Masks common credential formats. Best effort: this cannot promise no secrets remain.
pub fn mask_secrets(text: &str) -> String {
    let mut out = text.to_string();
    for re in secret_patterns() {
        out = re.replace_all(&out, |c: &regex::Captures| format!("{}[masked]", c.get(1).map(|m| m.as_str()).unwrap_or(""))).into_owned();
    }
    out
}

#[cfg(test)]
mod storage_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn common_secret_formats_are_masked() {
        let text = "token=abcdef123456 ghp_abcdefghijklmnopqrstuvwxyz sk-abcdefghijklmnopqrst AKIA1234567890123456 xoxb-123456789012 -----BEGIN PRIVATE KEY-----\nkey\n-----END PRIVATE KEY-----";
        let masked = mask_secrets(text);
        assert!(!masked.contains("abcdef"));
        assert!(!masked.contains("123456789012"));
        assert!(!masked.contains("PRIVATE KEY"));
        assert_eq!(masked.matches("[masked]").count(), 6);
        assert_eq!(mask_secrets("ordinary text"), "ordinary text");
    }

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
