// Things JavaScript did for free that the on-disk formats and the UI depend on.
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::UNIX_EPOCH;

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use regex::Regex;
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
