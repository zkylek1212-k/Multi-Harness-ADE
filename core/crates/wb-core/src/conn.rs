// Connection credentials, encrypted with the OS key in a gitignored file. Values only ever reach a spawned CLI's env:
// never an agent config file, never the UI. Old safeStorage values still read; every write uses DPAPI.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Map, Value};

use crate::core::{arg_str, Core};
use crate::manifest::{conn_refs, read_manifest, split_env};
use crate::util::{locale_cmp, read_text};

pub const DPAPI_PREFIX: &str = "dpapi:";

fn store_path(core: &Core) -> PathBuf {
    Path::new(&core.workspace_root()).join(".workbench").join("credentials.enc")
}

fn load(core: &Core) -> Map<String, Value> {
    read_text(&store_path(core)).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(core: &Core, s: &Map<String, Value>) -> Result<(), String> {
    let path = store_path(core);
    if let Some(d) = path.parent() {
        fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    fs::write(path, serde_json::to_string_pretty(s).unwrap()).map_err(|e| e.to_string())
}

pub fn protect_b64(plain: &str) -> Result<String, String> {
    wb_secrets::protect(plain.as_bytes()).map(|b| STANDARD.encode(b)).map_err(|e| e.to_string())
}

pub fn unprotect_b64(sealed: &str) -> Result<String, String> {
    let bytes = STANDARD.decode(sealed).map_err(|e| e.to_string())?;
    let plain = wb_secrets::unprotect(&bytes).map_err(|e| e.to_string())?;
    String::from_utf8(plain).map_err(|e| e.to_string())
}

/// A value Electron's safeStorage wrote, using the key in its Local State file
pub fn legacy_decrypt(core: &Core, sealed_b64: &str) -> Result<String, String> {
    let bytes = STANDARD.decode(sealed_b64).map_err(|e| e.to_string())?;
    let plain = wb_secrets::safe_storage_decrypt(&core.data_dir.join("Local State"), &bytes).map_err(|e| e.to_string())?;
    String::from_utf8(plain).map_err(|e| e.to_string())
}

fn reveal(core: &Core, name: &str) -> Option<String> {
    let store = load(core);
    let v = store.get(name)?.as_str()?;
    if v.is_empty() {
        return None;
    }
    match v.strip_prefix(DPAPI_PREFIX) {
        Some(b64) => unprotect_b64(b64).ok(),
        None => legacy_decrypt(core, v).ok(),
    }
}

/// The manifest's ${conn:x} placeholders as real env for a spawned CLI
pub fn resolve_connection_env(core: &Core) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let m = read_manifest(&core.workspace_root());
    for mcp in m["mcp"].as_array().into_iter().flatten() {
        for (env_key, conn) in split_env(mcp.get("env")).1 {
            if let Some(v) = reveal(core, &conn) {
                if !v.is_empty() {
                    out.insert(env_key, v);
                }
            }
        }
    }
    out
}

pub fn call(core: &Arc<Core>, channel: &str, args: &[Value]) -> Result<Value, String> {
    match channel {
        "conn:list" => {
            let store = load(core);
            let m = read_manifest(&core.workspace_root());
            // What the manifest asks for, plus stored values nothing uses any more
            let mut usage: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for mcp in m["mcp"].as_array().into_iter().flatten() {
                for name in conn_refs(mcp.get("env")) {
                    usage.entry(name).or_default().push(format!("mcp:{}", crate::util::sv(mcp, "id")));
                }
            }
            for name in store.keys() {
                usage.entry(name.clone()).or_default();
            }
            let mut rows: Vec<(String, Vec<String>)> = usage.into_iter().collect();
            rows.sort_by(|a, b| locale_cmp(&a.0, &b.0));
            // Only whether it is set, never the value
            Ok(rows.into_iter().map(|(name, used_by)| json!({ "isSet": store.get(&name).and_then(Value::as_str).map(|v| !v.is_empty()).unwrap_or(false), "name": name, "usedBy": used_by })).collect())
        }
        "conn:set" => {
            let name = arg_str(args, 0).trim();
            if name.is_empty() {
                return Err("Connection name cannot be empty".into());
            }
            let sealed = protect_b64(arg_str(args, 1)).map_err(|_| "OS-backed encryption is unavailable on this system; refusing to store the credential in plain text".to_string())?;
            let mut s = load(core);
            s.insert(name.to_string(), json!(format!("{DPAPI_PREFIX}{sealed}")));
            save(core, &s)?;
            Ok(Value::Null)
        }
        "conn:remove" => {
            let mut s = load(core);
            s.shift_remove(arg_str(args, 0));
            save(core, &s)?;
            Ok(Value::Null)
        }
        _ => Err(format!("No handler registered for '{channel}'")),
    }
}
