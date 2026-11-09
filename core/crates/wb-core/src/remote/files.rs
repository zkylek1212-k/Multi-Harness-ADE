// Read-only file access for the phone. Both the requested path and where symlinks lead must stay inside the workspace.
use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::util::{locale_cmp, now_ms};

const IGNORED: [&str; 4] = [".git", "node_modules", "out", ".deps"];

fn check(real_root: &Path, target: &Path) -> Result<(), String> {
    if target.starts_with(real_root) {
        Ok(())
    } else {
        Err("path outside workspace".into())
    }
}

/// Stricter than the path rules need: no drive-relative or rooted forms at all, and `..` resolves before the filesystem sees it
pub fn workspace_path(root: &str, relative: &str) -> Result<PathBuf, String> {
    if relative.len() > 4096 || relative.contains('\0') {
        return Err("invalid path".into());
    }
    let real_root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let mut candidate = real_root.clone();
    // Windows also treats a forward slash as a separator, so this sees "a/../../b" and "a\..\..\b" alike
    for part in Path::new(relative).components() {
        match part {
            Component::Normal(p) => candidate.push(p),
            Component::CurDir => {}
            Component::ParentDir => {
                candidate.pop();
            }
            Component::RootDir | Component::Prefix(_) => return Err("invalid path".into()),
        }
    }
    check(&real_root, &candidate)?;
    let real = fs::canonicalize(&candidate).map_err(|e| e.to_string())?;
    check(&real_root, &real)?;
    Ok(real)
}

fn rel_slashes(root: &str, file: &Path) -> Result<String, String> {
    let real_root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let rel = file.strip_prefix(&real_root).map_err(|_| "path outside workspace".to_string())?;
    Ok(rel.to_string_lossy().replace('\\', "/"))
}

pub fn list_workspace_files(root: &str, relative: &str) -> Result<Vec<Value>, String> {
    let dir = workspace_path(root, relative)?;
    let mut rows: Vec<(String, bool, PathBuf)> = Vec::new();
    for e in fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(t) = e.file_type() else { continue };
        // A junction counts as a symlink here too
        if IGNORED.contains(&name.as_str()) || t.is_symlink() {
            continue;
        }
        rows.push((name, t.is_dir(), e.path()));
    }
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| locale_cmp(&a.0, &b.0)));
    rows.truncate(2000);
    rows.into_iter().map(|(name, is_dir, path)| Ok(json!({ "name": name, "path": rel_slashes(root, &path)?, "isDir": is_dir }))).collect()
}

pub fn read_workspace_text(root: &str, relative: &str) -> Result<String, String> {
    let file = workspace_path(root, relative)?;
    let m = fs::metadata(&file).map_err(|e| e.to_string())?;
    if !m.is_file() || m.len() > 1024 * 1024 {
        return Err("text preview limit is 1 MB".into());
    }
    let data = fs::read(&file).map_err(|e| e.to_string())?;
    if data.contains(&0) {
        return Err("binary file cannot be previewed as text".into());
    }
    Ok(String::from_utf8_lossy(&data).into_owned())
}

pub fn mime_of(file: &Path) -> &'static str {
    match file.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("pdf") => "application/pdf",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        // Never lets a browser guess: an unknown type shows as text
        _ => "text/plain; charset=utf-8",
    }
}

/// HTML and SVG get an opaque sandbox origin, even when opened directly
pub fn csp_of(file: &Path) -> String {
    let active = matches!(file.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref(), Some("html" | "htm" | "svg"));
    format!("{}default-src 'none'; script-src https: 'unsafe-inline'; style-src https: 'unsafe-inline'; img-src https: data:; font-src https: data:; connect-src 'none'; form-action 'none'; base-uri 'none'", if active { "sandbox allow-scripts; " } else { "" })
}

pub enum RangeOut {
    Whole,
    Part(u64, u64),
    Unsatisfiable,
}

/// None is a malformed header
pub fn parse_range(header: &str, size: u64) -> Option<RangeOut> {
    let spec = header.strip_prefix("bytes=")?;
    let (a, b) = spec.split_once('-')?;
    let digits = |s: &str| s.bytes().all(|c| c.is_ascii_digit());
    if (a.is_empty() && b.is_empty()) || !digits(a) || !digits(b) {
        return None;
    }
    let (start, end) = if a.is_empty() {
        (size.saturating_sub(b.parse().ok()?), size as i128 - 1)
    } else {
        let start: u64 = a.parse().ok()?;
        let last = size as i128 - 1;
        (start, if b.is_empty() { last } else { last.min(b.parse::<u64>().ok()? as i128) })
    };
    if start as i128 > end || start >= size {
        return Some(RangeOut::Unsatisfiable);
    }
    Some(RangeOut::Part(start, end as u64))
}

#[derive(Clone)]
pub struct Grant {
    pub root: String,
    pub device_id: String,
    expires_at: f64,
}

/// Scoped, expiring preview URLs
#[derive(Default)]
pub struct RemoteFiles {
    grants: Mutex<Vec<(String, Grant)>>,
}

impl RemoteFiles {
    pub fn clear(&self) {
        self.grants.lock().unwrap().clear();
    }

    pub fn issue_url(&self, host: &str, root: &str, relative: &str, device_id: &str) -> Result<String, String> {
        let file = workspace_path(root, relative)?;
        if !fs::metadata(&file).map_err(|e| e.to_string())?.is_file() {
            return Err("not a file".into());
        }
        let key = super::random_hex(24);
        let now = now_ms();
        let mut g = self.grants.lock().unwrap();
        g.retain(|(_, x)| x.expires_at >= now);
        // ponytail: at most 100 grants, oldest goes; reopen a file if its preview expired
        if g.len() >= 100 {
            g.remove(0);
        }
        g.push((key.clone(), Grant { root: root.to_string(), device_id: device_id.to_string(), expires_at: now + 30.0 * 60_000.0 }));
        drop(g);
        let rel: Vec<String> = rel_slashes(root, &file)?.split('/').map(|s| percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()).collect();
        // ponytail: relative HTML assets work as files; root-relative URLs need the dev-server Preview
        Ok(format!("https://{host}/_files/{key}/{}", rel.join("/")))
    }

    pub fn grant(&self, key: &str) -> Option<Grant> {
        let now = now_ms();
        self.grants.lock().unwrap().iter().find(|(k, g)| super::store::ct_eq(k.as_bytes(), key.as_bytes()) && g.expires_at >= now).map(|(_, g)| g.clone())
    }
}

pub type DeviceWorkspaces = HashMap<String, Vec<String>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_rules() {
        let part = |h: &str, size| match parse_range(h, size) {
            Some(RangeOut::Part(a, b)) => Some((a, b)),
            _ => None,
        };
        assert_eq!(part("bytes=0-3", 10), Some((0, 3)));
        assert_eq!(part("bytes=5-", 10), Some((5, 9)));
        assert_eq!(part("bytes=-4", 10), Some((6, 9)));
        assert_eq!(part("bytes=-40", 10), Some((0, 9)));
        assert_eq!(part("bytes=8-400", 10), Some((8, 9)));
        assert!(matches!(parse_range("bytes=20-", 10), Some(RangeOut::Unsatisfiable)));
        assert!(matches!(parse_range("bytes=5-2", 10), Some(RangeOut::Unsatisfiable)));
        for bad in ["bytes=-", "items=0-1", "bytes=a-b", "bytes=0-1,3-4", "bytes=99999999999999999999999-"] {
            assert!(parse_range(bad, 10).is_none(), "{bad}");
        }
    }
}
