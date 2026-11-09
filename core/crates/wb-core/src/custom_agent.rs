// A user-configured agent, plus the guarded editor for its extension files.
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{json, Value};

use crate::util::{basename, dirname, resolve};

pub const CUSTOM_AGENT_ID: &str = "custom-agent";

pub fn sanitize_custom_agent(raw: Option<&Value>) -> Value {
    let empty = json!({});
    let c = raw.filter(|v| v.is_object()).unwrap_or(&empty);
    let text = |v: Option<&Value>| v.and_then(Value::as_str).map(|s| s.trim().to_string()).unwrap_or_default();
    let name = text(c.get("name"));
    let args: Vec<&str> = c.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).filter(|s| !s.is_empty()).collect()).unwrap_or_default();
    let res = c.get("resources");
    let r = |k: &str| text(res.and_then(|r| r.get(k)));
    json!({
        "enabled": c.get("enabled") == Some(&json!(true)),
        "name": if name.is_empty() { "Custom Agent".to_string() } else { name },
        "command": text(c.get("command")),
        "args": args,
        "resources": { "skill": r("skill"), "mcp": r("mcp"), "plugin": r("plugin"), "hook": r("hook") }
    })
}

const MAX_BYTES: u64 = 1024 * 1024;

fn resource(config: &Value, kind: &str) -> Result<(String, bool), String> {
    if config["enabled"] != json!(true) {
        return Err("Custom Agent is disabled".into());
    }
    if !["skill", "mcp", "plugin", "hook"].contains(&kind) {
        return Err("Unknown extension kind".into());
    }
    let root = config["resources"][kind].as_str().unwrap_or("");
    if root.is_empty() || !Path::new(root).is_absolute() {
        return Err("Set an absolute extension path in Settings → Agents first".into());
    }
    Ok((resolve(root), kind == "skill" || kind == "plugin"))
}

fn no_symlinks(target: &str, root: &str) -> Result<(), String> {
    let mut current = target.to_string();
    loop {
        if let Ok(m) = fs::symlink_metadata(&current) {
            if m.file_type().is_symlink() {
                return Err("Symbolic links are not editable here".into());
            }
        }
        if current == root {
            return Ok(());
        }
        let next = dirname(&current);
        if next == current {
            return Ok(());
        }
        current = next;
    }
}

fn target_path(config: &Value, kind: &str, file: &str) -> Result<String, String> {
    let (root, directory) = resource(config, kind)?;
    let bad_chars = Regex::new(r#"[<>:"|?*\x00-\x1f]"#).unwrap();
    let reserved = Regex::new(r"(?i)^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)").unwrap();
    let bad = file.is_empty()
        || Path::new(file).is_absolute()
        || file.split(['\\', '/']).any(|s| s.is_empty() || s == "." || s == ".." || bad_chars.is_match(s) || s.ends_with('.') || s.ends_with(' ') || reserved.is_match(s));
    if bad {
        return Err("Use a relative file name without special characters or parent traversal".into());
    }
    if file.to_lowercase().ends_with(".workbench-backup") {
        return Err("Backup files are read-only".into());
    }
    let target = if directory { crate::util::p(&PathBuf::from(&root).join(file)) } else { root.clone() };
    let target = if directory { resolve(&target) } else { target };
    if !directory && file != basename(&root) {
        return Err("Only the configured file can be edited".into());
    }
    let base = if directory { root.clone() } else { dirname(&root) };
    if target == base || !target.to_lowercase().starts_with(&format!("{}\\", base.to_lowercase()).replace("\\\\", "\\")) && !target.to_lowercase().starts_with(&format!("{}/", base.to_lowercase())) {
        return Err("File is outside the configured extension path".into());
    }
    no_symlinks(&target, &base)?;
    Ok(target)
}

pub fn list_files(config: &Value, kind: &str) -> Result<Vec<String>, String> {
    let (root, directory) = resource(config, kind)?;
    no_symlinks(&root, &root)?;
    if !directory {
        return Ok(vec![basename(&root)]);
    }
    if !Path::new(&root).exists() {
        return Ok(Vec::new());
    }
    fn walk(root: &Path, dir: &Path, depth: u32, out: &mut Vec<String>) -> Result<(), String> {
        for e in fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(t) = e.file_type() else { continue };
            if t.is_symlink() || name == "node_modules" || name == ".git" || name.to_lowercase().ends_with(".workbench-backup") {
                continue;
            }
            if t.is_dir() {
                if depth < 6 {
                    walk(root, &e.path(), depth + 1, out)?;
                }
            } else if t.is_file() {
                out.push(e.path().strip_prefix(root).unwrap_or(&e.path()).to_string_lossy().replace('\\', "/"));
                if out.len() > 500 {
                    return Err("More than 500 extension files; choose a smaller directory".into());
                }
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(Path::new(&root), Path::new(&root), 0, &mut out)?;
    out.sort();
    Ok(out)
}

pub fn read_file(config: &Value, kind: &str, file: &str) -> Result<Option<String>, String> {
    let target = target_path(config, kind, file)?;
    let meta = match fs::metadata(&target) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    if meta.len() > MAX_BYTES {
        return Err("Only text files up to 1 MB can be edited".into());
    }
    let raw = fs::read(&target).map_err(|e| e.to_string())?;
    match String::from_utf8(raw) {
        Ok(t) if !t.contains('\0') => Ok(Some(t)),
        _ => Err("Only UTF-8 text files can be edited".into()),
    }
}

#[cfg(windows)]
fn link_count(path: &Path) -> u32 {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
    let Ok(f) = fs::File::open(path) else { return 0 };
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(f.as_raw_handle() as _, &mut info) } == 0 {
        return 0;
    }
    info.nNumberOfLinks
}

#[cfg(not(windows))]
fn link_count(path: &Path) -> u32 {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).map(|m| m.nlink() as u32).unwrap_or(0)
}

pub fn write_file(config: &Value, kind: &str, file: &str, content: &str, before: Option<&str>) -> Result<(), String> {
    let target = target_path(config, kind, file)?;
    if content.len() as u64 > MAX_BYTES || content.contains('\0') {
        return Err("Only text files up to 1 MB can be saved".into());
    }
    if target.to_lowercase().ends_with(".json") {
        serde_json::from_str::<Value>(content).map_err(|e| e.to_string())?;
    }
    if read_file(config, kind, file)?.as_deref() != before {
        return Err("File changed on disk. Reload before saving".into());
    }
    let dir = dirname(&target);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let temp = PathBuf::from(&dir).join(format!(".workbench-{}.tmp", crate::util::uuid()));
    let result = (|| -> Result<(), String> {
        use std::io::Write;
        let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&temp).map_err(|e| e.to_string())?;
        f.write_all(content.as_bytes()).map_err(|e| e.to_string())?;
        drop(f);
        if before.is_some() {
            let backup = format!("{target}.workbench-backup");
            no_symlinks(&backup, &dir)?;
            // Copying over a hard link would write into whatever else it points to
            if link_count(Path::new(&backup)) > 1 {
                return Err("Backup destination cannot be a hard link".into());
            }
            fs::copy(&target, &backup).map_err(|e| e.to_string())?;
            fs::rename(&temp, &target).map_err(|e| e.to_string())
        } else {
            // Linking fails if another process created the destination while this form was open
            fs::hard_link(&temp, &target).map_err(|e| e.to_string())
        }
    })();
    let _ = fs::remove_file(&temp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_and_reserved_names() {
        let dir = std::env::temp_dir().join(format!("wb-ca-{}", crate::util::uuid()));
        fs::create_dir_all(&dir).unwrap();
        let cfg = json!({ "enabled": true, "resources": { "skill": dir.to_string_lossy() } });
        for bad in ["../x", "a/../../x", "con", "x:y", "a.", "x.workbench-backup", ""] {
            assert!(write_file(&cfg, "skill", bad, "hi", None).is_err(), "{bad}");
        }
        write_file(&cfg, "skill", "a/b.md", "one", None).unwrap();
        assert_eq!(read_file(&cfg, "skill", "a/b.md").unwrap().as_deref(), Some("one"));
        assert!(write_file(&cfg, "skill", "a/b.md", "two", Some("stale")).is_err());
        write_file(&cfg, "skill", "a/b.md", "two", Some("one")).unwrap();
        assert_eq!(list_files(&cfg, "skill").unwrap(), vec!["a/b.md"]);
        assert_eq!(fs::read_to_string(dir.join("a").join("b.md.workbench-backup")).unwrap(), "one");
        let _ = fs::remove_dir_all(dir);
    }
}
