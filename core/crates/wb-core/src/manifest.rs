// .workbench/extensions.yaml: one source of truth, generating each agent's native config.
// It travels with the repo, so ids and paths are untrusted (see adapters in ext.rs).
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Value};

use crate::util::{read_text, sv};

fn manifest_path(ws: &str) -> std::path::PathBuf {
    Path::new(ws).join(".workbench").join("extensions.yaml")
}

pub fn read_manifest(ws: &str) -> Value {
    let empty = json!({ "version": 1, "skills": [], "mcp": [], "plugins": [] });
    let Some(raw) = read_text(&manifest_path(ws)) else { return empty };
    let Ok(m) = serde_norway::from_str::<Value>(&raw) else { return empty };
    if m.is_null() {
        return empty;
    }
    let or = |k: &str, d: Value| m.get(k).filter(|v| !v.is_null()).cloned().unwrap_or(d);
    json!({ "version": or("version", json!(1)), "skills": or("skills", json!([])), "mcp": or("mcp", json!([])), "plugins": or("plugins", json!([])) })
}

pub fn write_manifest(ws: &str, m: &Value) -> Result<(), String> {
    let path = manifest_path(ws);
    if let Some(d) = path.parent() {
        fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let body = serde_norway::to_string(m).map_err(|e| e.to_string())?;
    fs::write(path, format!("# Workbench 擴充清單：唯一真相，由 Customized 面板產生各家 agent 的原生設定。\n# 憑證不放這裡 —— env 值寫 ${{conn:名稱}}，實際值存 OS 加密儲存，spawn 時才注入。\n{body}")).map_err(|e| e.to_string())
}

pub fn managed_keys(m: &Value) -> std::collections::HashSet<String> {
    let mut s = std::collections::HashSet::new();
    for (list, kind) in [("skills", "skill"), ("mcp", "mcp"), ("plugins", "plugin")] {
        for x in m[list].as_array().into_iter().flatten() {
            s.insert(format!("{kind}:{}", sv(x, "id").to_lowercase()));
        }
    }
    s
}

fn conn_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^\$\{conn:([A-Za-z0-9_.\-]+)\}$").unwrap())
}

pub fn conn_refs(env: Option<&Value>) -> Vec<String> {
    env.and_then(Value::as_object).into_iter().flatten().filter_map(|(_, v)| v.as_str().and_then(|s| conn_re().captures(s)).map(|m| m[1].to_string())).collect()
}

/// Env that may be written to a config file, and env (key -> connection name) injected at spawn
pub fn split_env(env: Option<&Value>) -> (serde_json::Map<String, Value>, BTreeMap<String, String>) {
    let (mut plain, mut secret) = (serde_json::Map::new(), BTreeMap::new());
    for (k, v) in env.and_then(Value::as_object).into_iter().flatten() {
        match v.as_str().and_then(|s| conn_re().captures(s)) {
            Some(m) => {
                secret.insert(k.clone(), m[1].to_string());
            }
            None => {
                plain.insert(k.clone(), v.clone());
            }
        }
    }
    (plain, secret)
}
