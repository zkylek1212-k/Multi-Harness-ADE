// Skills, MCP servers and plugins across agents: the scan, the toggle state and the sync to native config files.
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::core::{arg_str, Core};
use crate::custom_agent as ca;
use crate::manifest::{conn_refs, managed_keys, read_manifest, split_env, write_manifest};
use crate::paths::{agent_paths, AGENTS};
use crate::util::{home, locale_cmp, p, read_text, resolve, sv};

fn read_json(path: &Path) -> Value {
    read_text(path).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}))
}

fn list_dirs(path: &Path) -> Vec<String> {
    fs::read_dir(path).map(|r| r.flatten().filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false)).map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default()
}

fn frontmatter() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^---\r?\n([\s\S]*?)\r?\n---").unwrap())
}

#[derive(Default)]
struct Skill {
    name: Option<String>,
    description: Option<String>,
    version: Option<String>,
}

fn parse_skill_md(file: &Path) -> Skill {
    let Some(raw) = read_text(file) else { return Skill::default() };
    let Some(m) = frontmatter().captures(&raw) else { return Skill::default() };
    let Ok(fm) = serde_norway::from_str::<Value>(&m[1]) else { return Skill::default() };
    let s = |k: &str| fm.get(k).and_then(Value::as_str).map(str::to_string);
    Skill { name: s("name"), description: s("description"), version: fm.pointer("/metadata/version").and_then(Value::as_str).map(str::to_string) }
}

struct Found {
    kind: &'static str,
    id: String,
    name: String,
    description: Option<String>,
    version: Option<String>,
    agent: &'static str,
    detail: Option<String>,
}

fn norm(s: &str) -> String {
    let lower = s.trim().to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut sep = false;
    for c in lower.chars() {
        if c.is_whitespace() || c == '_' {
            if !sep {
                out.push('-');
            }
            sep = true;
        } else {
            sep = false;
            out.push(c);
        }
    }
    out
}

fn found(kind: &'static str, name: &str, description: Option<String>, version: Option<String>, agent: &'static str, detail: Option<String>) -> Found {
    Found { kind, id: norm(name), name: name.to_string(), description, version, agent, detail }
}

fn skill_found(md: &Path, fallback: &str, agent: &'static str, desc_fallback: Option<String>, version_fallback: Option<String>, detail: Option<String>) -> Found {
    let fm = parse_skill_md(md);
    let name = fm.name.unwrap_or_else(|| fallback.to_string());
    found("skill", &name, fm.description.or(desc_fallback), fm.version.or(version_fallback), agent, detail.or_else(|| Some(p(md))))
}

fn cfg_desc(v: &Value) -> Option<String> {
    [sv(v, "command"), sv(v, "url")].into_iter().find(|s| !s.is_empty()).map(str::to_string)
}

fn scan_claude(ws: &str) -> Vec<Found> {
    let pa = agent_paths("claude");
    let mut out = Vec::new();
    if let Some(dir) = &pa.skills_dir {
        for d in list_dirs(dir) {
            let md = dir.join(&d).join("SKILL.md");
            if md.exists() {
                out.push(skill_found(&md, &d, "claude", None, None, None));
            }
        }
    }
    let settings = read_json(&pa.config_home.join("settings.json"));
    let inst = pa.plugins_dir.join("installed_plugins.json");
    if inst.exists() {
        let raw = read_json(&inst);
        let map = raw.get("plugins").filter(|v| v.is_object()).unwrap_or(&raw);
        for (key, installs) in map.as_object().into_iter().flatten() {
            if key == "version" {
                continue;
            }
            let first = if installs.is_array() { installs.get(0) } else { Some(installs) };
            let install_path = first.map(|f| sv(f, "installPath")).filter(|s| !s.is_empty());
            let version = first.map(|f| sv(f, "version")).filter(|s| !s.is_empty()).map(str::to_string);
            let enabled = settings.pointer("/enabledPlugins").and_then(|e| e.get(key)) != Some(&json!(false));
            let (plugin_name, marketplace) = match key.split_once('@') {
                Some((a, b)) => (a, b.split('@').next().unwrap_or(b)),
                None => (key.as_str(), ""),
            };
            let description = (!marketplace.is_empty()).then(|| format!("marketplace: {marketplace}{}", if enabled { "" } else { " (disabled)" }));
            out.push(Found { kind: "plugin", id: norm(plugin_name), name: key.clone(), description, version: version.clone(), agent: "claude", detail: install_path.map(str::to_string) });
            let Some(ip) = install_path.filter(|i| Path::new(i).exists()) else { continue };
            let skills = Path::new(ip).join("skills");
            for sub in list_dirs(&skills) {
                let md = skills.join(&sub).join("SKILL.md");
                if md.exists() {
                    out.push(skill_found(&md, &sub, "claude", Some(format!("Provided by plugin {plugin_name}")), version.clone(), Some(format!("plugin: {key}"))));
                }
            }
            for (name, cfg) in read_json(&Path::new(ip).join(".mcp.json")).get("mcpServers").and_then(Value::as_object).into_iter().flatten() {
                out.push(found("mcp", name, cfg_desc(cfg).or_else(|| Some(format!("Plugin MCP: {plugin_name}"))), None, "claude", Some(format!("plugin: {key}"))));
            }
        }
    }
    let mut push_mcp = |servers: Option<&Value>, whr: &str| {
        for (k, v) in servers.and_then(Value::as_object).into_iter().flatten() {
            out.push(found("mcp", k, cfg_desc(v), None, "claude", Some(whr.to_string())));
        }
    };
    let cj = read_json(&pa.mcp_config);
    push_mcp(cj.get("mcpServers"), "~/.claude.json (global)");
    let ws_key = ws.replace('\\', "/").to_lowercase();
    let ws_base = ws.rsplit(['\\', '/']).next().unwrap_or("").to_lowercase();
    for (proj, cfg) in cj.get("projects").and_then(Value::as_object).into_iter().flatten() {
        let key = proj.replace('\\', "/").to_lowercase();
        if key == ws_key || (!ws_base.is_empty() && key.contains(&ws_base)) {
            push_mcp(cfg.get("mcpServers"), "~/.claude.json (this project)");
        }
    }
    push_mcp(read_json(&Path::new(ws).join(".mcp.json")).get("mcpServers"), ".mcp.json");
    out
}

fn scan_antigravity(ws: &str) -> Vec<Found> {
    let pa = agent_paths("antigravity");
    let mut out = Vec::new();
    for d in list_dirs(&pa.plugins_dir) {
        let dir = pa.plugins_dir.join(&d);
        let meta = read_json(&dir.join("plugin.json"));
        let name = Some(sv(&meta, "name")).filter(|s| !s.is_empty()).unwrap_or(&d).to_string();
        let opt = |k: &str| Some(sv(&meta, k)).filter(|s| !s.is_empty()).map(str::to_string);
        out.push(found("plugin", &name, opt("description"), opt("version"), "antigravity", None));
        let skills = dir.join("skills");
        let direct = skills.join("SKILL.md");
        if direct.exists() {
            out.push(skill_found(&direct, &d, "antigravity", None, None, None));
        }
        for sub in list_dirs(&skills) {
            let md = skills.join(&sub).join("SKILL.md");
            if md.exists() {
                out.push(skill_found(&md, &sub, "antigravity", Some(format!("Provided by plugin {name}")), opt("version"), None));
            }
        }
        for (k, v) in read_json(&dir.join("gemini-extension.json")).get("mcpServers").and_then(Value::as_object).into_iter().flatten() {
            out.push(found("mcp", k, Some(sv(v, "command")).filter(|s| !s.is_empty()).map(str::to_string), None, "antigravity", Some(format!("plugin: {d}"))));
        }
    }
    let mut roots = vec![pa.config_home.join("skills")];
    if !ws.is_empty() {
        roots.push(Path::new(ws).join(".agents").join("skills"));
    }
    for root in roots {
        for sub in list_dirs(&root) {
            let md = root.join(&sub).join("SKILL.md");
            if md.exists() {
                out.push(skill_found(&md, &sub, "antigravity", None, None, None));
            }
        }
    }
    for (k, v) in read_json(&pa.mcp_config).get("mcpServers").and_then(Value::as_object).into_iter().flatten() {
        out.push(found("mcp", k, Some(sv(v, "command")).filter(|s| !s.is_empty()).map(str::to_string), None, "antigravity", Some("mcp_config.json".into())));
    }
    out
}

/// Tables with a prefix, for display only. Not a TOML parser: a nested table such as [mcp_servers.x.env] ends the table.
fn toml_tables(raw: &str, prefix: &str) -> Vec<(String, Vec<String>)> {
    let header = Regex::new(&format!(r#"^\[{prefix}\.(?:"([^"]+)"|([A-Za-z0-9_-]+))\]\s*$"#)).unwrap();
    let any = Regex::new(r"^\[.*\]\s*$").unwrap();
    let mut tables: Vec<(String, Vec<String>)> = Vec::new();
    let mut current: Option<usize> = None;
    for line in raw.lines() {
        let line = line.trim();
        if let Some(m) = header.captures(line) {
            let name = m.get(1).or(m.get(2)).unwrap().as_str().to_string();
            tables.retain(|t| t.0 != name);
            tables.push((name, Vec::new()));
            current = Some(tables.len() - 1);
        } else if any.is_match(line) {
            current = None;
        } else if let Some(i) = current {
            tables[i].1.push(line.to_string());
        }
    }
    tables
}

fn scan_codex() -> Vec<Found> {
    let pa = agent_paths("codex");
    let mut out = Vec::new();
    if let Some(dir) = &pa.skills_dir {
        for d in list_dirs(dir) {
            if d.starts_with('.') {
                continue;
            }
            let md = dir.join(&d).join("SKILL.md");
            if md.exists() {
                out.push(skill_found(&md, &d, "codex", None, None, None));
            }
        }
    }
    if let Some(raw) = read_text(&pa.mcp_config) {
        let cmd = Regex::new(r#"^command\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap();
        for (name, lines) in toml_tables(&raw, "mcp_servers") {
            let desc = lines.iter().find_map(|l| cmd.captures(l)).map(|m| m.get(1).or(m.get(2)).unwrap().as_str().to_string());
            out.push(found("mcp", &name, desc, None, "codex", Some("config.toml".into())));
        }
        let en = Regex::new(r"^enabled\s*=\s*(true|false)").unwrap();
        for (key, lines) in toml_tables(&raw, "plugins") {
            if lines.iter().find_map(|l| en.captures(l)).map(|m| &m[1] == "false").unwrap_or(false) {
                continue;
            }
            let (name, market) = match key.split_once('@') {
                Some((a, b)) => (a, b),
                None => (key.as_str(), ""),
            };
            out.push(found("plugin", name, (!market.is_empty()).then(|| format!("marketplace: {market}")), None, "codex", Some("config.toml".into())));
        }
    }
    out
}

fn build_inventory(ws: &str, managed: &HashSet<String>) -> Vec<Value> {
    let mut order: Vec<String> = Vec::new();
    let mut by_key: HashMap<String, Value> = HashMap::new();
    let all = scan_claude(ws).into_iter().chain(scan_antigravity(ws)).chain(scan_codex());
    for f in all {
        let key = format!("{}:{}", f.kind, f.id);
        let item = by_key.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            let mut o = json!({ "id": f.id, "kind": f.kind, "name": f.name });
            if let Some(d) = &f.description {
                o["description"] = json!(d);
            }
            if let Some(v) = &f.version {
                o["version"] = json!(v);
            }
            o["managed"] = json!(managed.contains(&key));
            o["agents"] = json!([]);
            o
        });
        if sv(item, "description").is_empty() {
            if let Some(d) = &f.description {
                item["description"] = json!(d);
            }
        }
        if sv(item, "version").is_empty() {
            if let Some(v) = &f.version {
                item["version"] = json!(v);
            }
        }
        let agents = item["agents"].as_array_mut().unwrap();
        if !agents.iter().any(|a| sv(a, "agent") == f.agent) {
            let mut a = json!({ "agent": f.agent, "state": "installed" });
            if let Some(d) = &f.detail {
                a["detail"] = json!(d);
            }
            agents.push(a);
        }
    }
    let mut items: Vec<Value> = order.into_iter().filter_map(|k| by_key.remove(&k)).collect();
    for item in &mut items {
        let agents = item["agents"].as_array_mut().unwrap();
        for a in AGENTS {
            if !agents.iter().any(|x| sv(x, "agent") == a) {
                agents.push(json!({ "agent": a, "state": "missing" }));
            }
        }
        agents.sort_by_key(|x| AGENTS.iter().position(|a| *a == sv(x, "agent")).unwrap_or(9));
    }
    items.sort_by(|a, b| sv(a, "kind").cmp(sv(b, "kind")).then_with(|| locale_cmp(sv(a, "name"), sv(b, "name"))));
    items
}

/// Everything installed, with no manifest or toggle state
pub fn build_inventory_public(ws: &str) -> Vec<Value> {
    build_inventory(ws, &HashSet::new())
}

fn disabled_keys(core: &Core, ws: &str) -> HashSet<String> {
    let mut keys = HashSet::new();
    let mut files = vec![core.data_dir.join("customized-state.json")];
    if !ws.is_empty() {
        files.push(Path::new(ws).join(".workbench").join("customized-state.json"));
    }
    for f in files {
        if let Some(list) = read_text(&f).and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            for k in list["disabled"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                keys.insert(k.to_string());
            }
        }
    }
    keys
}

fn save_disabled(core: &Core, keys: &HashSet<String>, ws: &str) {
    let mut sorted: Vec<&String> = keys.iter().collect();
    sorted.sort();
    let body = serde_json::to_string_pretty(&json!({ "disabled": sorted })).unwrap();
    let _ = fs::create_dir_all(&core.data_dir);
    let _ = fs::write(core.data_dir.join("customized-state.json"), &body);
    if !ws.is_empty() && !core.is_protected_path(ws) {
        let dir = Path::new(ws).join(".workbench");
        let _ = fs::create_dir_all(&dir);
        let _ = fs::write(dir.join("customized-state.json"), &body);
    }
}

fn claude_settings_path() -> PathBuf {
    home().join(".claude").join("settings.json")
}

fn inventory(core: &Core, ws: &str) -> Vec<Value> {
    let m = read_manifest(ws);
    let mut items = build_inventory(ws, &managed_keys(&m));
    let seen: HashSet<String> = items.iter().map(|i| format!("{}:{}", sv(i, "kind"), sv(i, "id"))).collect();
    // Declared in the manifest but installed nowhere yet still shows, so the state is complete
    for mc in m["mcp"].as_array().into_iter().flatten() {
        let id = sv(mc, "id").to_lowercase();
        if seen.contains(&format!("mcp:{id}")) {
            continue;
        }
        items.push(json!({
            "id": id, "kind": "mcp", "name": sv(mc, "id"), "description": sv(mc, "command"), "managed": true,
            "needsConnection": conn_refs(mc.get("env")),
            "agents": AGENTS.iter().map(|a| json!({ "agent": a, "state": "missing", "detail": "Declared, pending sync" })).collect::<Vec<_>>()
        }));
    }
    let plugins = read_json(&claude_settings_path()).get("enabledPlugins").filter(|v| v.is_object()).cloned().unwrap_or(json!({}));
    let plugins = plugins.as_object().unwrap();
    let disabled = disabled_keys(core, ws);
    for it in &mut items {
        let kind = sv(it, "kind").to_string();
        let id_lower = sv(it, "id").to_lowercase();
        let id = sv(it, "id").to_string();
        if kind == "mcp" {
            if let Some(decl) = m["mcp"].as_array().into_iter().flatten().find(|x| sv(x, "id").to_lowercase() == id_lower) {
                it["needsConnection"] = json!(conn_refs(decl.get("env")));
            }
        }
        for a in it["agents"].as_array_mut().unwrap() {
            if sv(a, "state") != "installed" {
                a["enabled"] = json!(false);
                continue;
            }
            let agent = sv(a, "agent").to_string();
            let explicit = disabled.contains(&format!("{agent}:{kind}:{id_lower}")) || (disabled.contains(&format!("{kind}:{id_lower}")) && !disabled.contains(&format!("{agent}:enabled:{kind}:{id_lower}")));
            let mut enabled = !explicit;
            if enabled && agent == "claude" {
                if kind == "plugin" {
                    let hit = plugins.iter().find(|(k, _)| **k == id || k.starts_with(&format!("{id}@")) || k.split('@').next().unwrap_or("").to_lowercase() == id_lower);
                    enabled = hit.map(|(_, v)| v != &json!(false)).unwrap_or(true);
                } else if let Some(rest) = sv(a, "detail").strip_prefix("plugin:") {
                    let name = rest.trim().split('@').next().unwrap_or("").to_lowercase();
                    let hit = plugins.iter().find(|(k, _)| k.to_lowercase() == name || k.split('@').next().unwrap_or("").to_lowercase() == name);
                    enabled = hit.map(|(_, v)| v != &json!(false)).unwrap_or(true);
                }
            }
            a["enabled"] = json!(enabled);
        }
        let any = it["agents"].as_array().unwrap().iter().any(|a| sv(a, "state") == "installed" && a.get("enabled") != Some(&json!(false)));
        it["enabled"] = json!(any);
    }
    items
}

fn agent_status(core: &Core, ws: &str, items: &[Value]) -> Vec<Value> {
    AGENTS
        .iter()
        .map(|id| {
            let pa = agent_paths(id);
            let cli = core.find_agent_cli(id, true);
            let mut notes: Vec<String> = Vec::new();
            let mut counts = json!({ "skill": 0, "mcp": 0, "plugin": 0 });
            for it in items {
                let installed = it["agents"].as_array().into_iter().flatten().any(|a| sv(a, "agent") == *id && sv(a, "state") == "installed");
                if installed {
                    let k = sv(it, "kind");
                    counts[k] = json!(counts[k].as_u64().unwrap_or(0) + 1);
                }
            }
            if cli.is_none() {
                notes.push(format!("{} was not found on PATH", pa.cli));
            }
            if !pa.config_home.exists() {
                notes.push(format!("Config directory does not exist: {}", pa.config_home.display()));
            }
            // agy may refuse to work in a folder it has not been told to trust
            if let Some(ts) = pa.trust_settings.as_ref().filter(|t| t.exists()) {
                let s = read_json(ts);
                let wsn = ws.replace('\\', "/").to_lowercase();
                let trusted = s["trustedWorkspaces"].as_array().into_iter().flatten().filter_map(Value::as_str).map(|x| x.replace('\\', "/").to_lowercase()).any(|t| wsn == t || wsn.starts_with(&format!("{t}/")));
                if !trusted {
                    notes.push("This workspace is not in Antigravity's trust list".into());
                }
            }
            if *id == "antigravity" {
                notes.push("MCP config is global only (no per-project scope) — writing affects every project".into());
            }
            let mut o = json!({ "agent": id, "label": pa.label, "cliFound": cli.is_some() });
            if let Some(c) = cli {
                o["cliPath"] = json!(c);
            }
            o["configHome"] = json!(p(&pa.config_home));
            o["supported"] = json!(true);
            o["pending"] = json!(false);
            o["counts"] = counts;
            o["notes"] = json!(notes);
            o
        })
        .collect()
}

// ── Sync: only keys we manage are touched, a user's config is never overwritten wholesale ──

fn safe_id(id: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    // id is joined into paths like ~/.claude/skills/<id>/; `../` would write anywhere in the home folder
    R.get_or_init(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$").unwrap()).is_match(id) && id != "." && id != ".."
}

fn inside_workspace(ws: &str, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() {
        return None;
    }
    let root = resolve(ws);
    let abs = resolve(&p(&Path::new(&root).join(rel)));
    (abs.to_lowercase().starts_with(&format!("{}{}", root.to_lowercase(), std::path::MAIN_SEPARATOR)) || abs.to_lowercase() == root.to_lowercase()).then(|| PathBuf::from(abs))
}

fn targets(v: &Value, agent: &str) -> bool {
    v["targets"].as_array().into_iter().flatten().any(|t| t.as_str() == Some(agent))
}

fn merge_mcp_json(existing: &str, manifest: &Value, agent: &str) -> String {
    let mut doc: Map<String, Value> = serde_json::from_str(existing.trim()).unwrap_or_default();
    let mut servers = doc.get("mcpServers").and_then(Value::as_object).cloned().unwrap_or_default();
    let mcp: Vec<&Value> = manifest["mcp"].as_array().into_iter().flatten().filter(|m| targets(m, agent)).collect();
    let now: Vec<&str> = mcp.iter().map(|m| sv(m, "id")).collect();
    for id in doc.get("__workbenchManaged").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        if !now.contains(&id) {
            servers.shift_remove(id);
        }
    }
    let npx = Regex::new(r"(?i)^npx(?:\.cmd)?$").unwrap();
    for m in &mcp {
        if !safe_id(sv(m, "id")) {
            continue;
        }
        // Credentials stay out of files: ${conn:x} is injected at spawn
        let (plain, _) = split_env(m.get("env"));
        let args: Vec<Value> = m["args"].as_array().cloned().unwrap_or_default();
        let windows_npx = cfg!(windows) && agent == "claude" && npx.is_match(sv(m, "command"));
        let mut srv = if windows_npx {
            let mut a = vec![json!("/c"), json!("npx")];
            a.extend(args);
            json!({ "command": "cmd.exe", "args": a })
        } else {
            json!({ "command": sv(m, "command"), "args": args })
        };
        if !plain.is_empty() {
            srv["env"] = Value::Object(plain);
        }
        servers.insert(sv(m, "id").to_string(), srv);
    }
    doc.insert("mcpServers".into(), Value::Object(servers));
    doc.insert("__workbenchManaged".into(), json!(now));
    serde_json::to_string_pretty(&doc).unwrap() + "\n"
}

/// Nothing is written here: most targets sit under ~ with no git to undo them, so the user sees this first
pub fn plan_sync(ws: &str, manifest: &Value) -> Vec<Value> {
    let mut changes = Vec::new();
    let text = |f: &Path| read_text(f).unwrap_or_default();
    let change = |path: &Path, agent: &str, before: &str, after: &str, note: Option<&str>| {
        let mut c = json!({ "path": p(path), "agent": agent, "before": before, "after": after });
        if let Some(n) = note {
            c["note"] = json!(n);
        }
        c
    };
    let any = |a: &str| manifest["mcp"].as_array().into_iter().flatten().any(|m| targets(m, a));
    if any("claude") {
        let f = Path::new(ws).join(".mcp.json");
        let before = text(&f);
        let after = merge_mcp_json(&before, manifest, "claude");
        if before != after {
            changes.push(change(&f, "claude", &before, &after, Some("Project scope — can be committed with the repo")));
        }
    }
    if any("antigravity") {
        let f = agent_paths("antigravity").mcp_config;
        let before = text(&f);
        let after = merge_mcp_json(&before, manifest, "antigravity");
        if before != after {
            changes.push(change(&f, "antigravity", &before, &after, Some("Global config — affects every project")));
        }
    }
    for s in manifest["skills"].as_array().into_iter().flatten() {
        let id = sv(s, "id");
        if !safe_id(id) {
            continue;
        }
        let Some(src) = inside_workspace(ws, sv(s, "path")) else { continue };
        let md = src.join("SKILL.md");
        let content = text(&md);
        if content.is_empty() {
            changes.push(change(&md, "claude", "", "", Some("⚠ Source not found — this skill will be skipped")));
            continue;
        }
        if targets(s, "claude") {
            let f = agent_paths("claude").skills_dir.unwrap().join(id).join("SKILL.md");
            let before = text(&f);
            if before != content {
                changes.push(change(&f, "claude", &before, &content, None));
            }
        }
        if targets(s, "antigravity") {
            let dir = agent_paths("antigravity").plugins_dir.join(id);
            let f = dir.join("skills").join("SKILL.md");
            let before = text(&f);
            if before != content {
                changes.push(change(&f, "antigravity", &before, &content, Some("Antigravity skills must live inside a plugin — plugin.json is generated too")));
            }
            let pj = dir.join("plugin.json");
            if text(&pj).is_empty() {
                let after = serde_json::to_string_pretty(&json!({ "name": id, "version": "0.0.0", "description": format!("Managed by Agent Workbench: {id}"), "author": { "name": "Agent Workbench" } })).unwrap() + "\n";
                changes.push(change(&pj, "antigravity", "", &after, None));
            }
        }
    }
    changes
}

fn apply_sync(changes: &[Value]) -> Result<Vec<String>, String> {
    let mut written = Vec::new();
    for c in changes {
        if sv(c, "note").starts_with('⚠') {
            continue;
        }
        let f = Path::new(sv(c, "path"));
        if let Some(d) = f.parent() {
            fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        fs::write(f, sv(c, "after")).map_err(|e| e.to_string())?;
        written.push(sv(c, "path").to_string());
    }
    Ok(written)
}

fn toggle(core: &Core, ws: &str, kind: &str, id: &str, enabled: bool, agent: Option<&str>) {
    let mut disabled = disabled_keys(core, ws);
    let id_lower = id.to_lowercase();
    let mut one = |a: &str| {
        let key = format!("{a}:{kind}:{id_lower}");
        if enabled {
            disabled.remove(&key);
            disabled.remove(&format!("{kind}:{id_lower}"));
        } else {
            disabled.insert(key);
        }
        if a == "claude" && kind == "plugin" {
            let f = claude_settings_path();
            if let Some(mut cfg) = read_text(&f).and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
                if !cfg["enabledPlugins"].is_object() {
                    cfg["enabledPlugins"] = json!({});
                }
                let map = cfg["enabledPlugins"].as_object_mut().unwrap();
                map.insert(id.to_string(), json!(enabled));
                for (k, v) in map.iter_mut() {
                    if k == id || k.starts_with(&format!("{id}@")) || k.split('@').next().unwrap_or("").to_lowercase() == id_lower {
                        *v = json!(enabled);
                    }
                }
                let _ = fs::write(&f, serde_json::to_string_pretty(&cfg).unwrap());
            }
        }
    };
    match agent {
        Some(a) => one(a),
        None => {
            for a in AGENTS {
                one(a);
            }
            if enabled {
                disabled.remove(&format!("{kind}:{id_lower}"));
            } else {
                disabled.insert(format!("{kind}:{id_lower}"));
            }
        }
    }
    save_disabled(core, &disabled, ws);
}

/// Visible install commands, pasted into a terminal for the user to run. Never run in the background:
/// antivirus flags download-and-execute, and the user could not see what is happening.
fn ps_with_npm(url: &str, pkg: &str) -> String {
    format!("try {{ irm {url} -ErrorAction Stop | iex }} catch {{ Write-Warning \"Official installer failed: $($_.Exception.Message) - falling back to: npm install -g {pkg}\"; npm install -g {pkg} }}")
}

/// Official script, then winget, then npm. Tested 2026-10-24: the official script URLs return 404, and only winget needs no Node.
fn ps_codex() -> String {
    "try { irm https://chatgpt.com/codex/install.ps1 -ErrorAction Stop | iex } catch { Write-Warning \"Official installer failed: $($_.Exception.Message)\"; $ok = $false; if (Get-Command winget -ErrorAction SilentlyContinue) { Write-Host \"Trying: winget install --id OpenAI.Codex -e\"; winget install --id OpenAI.Codex -e --source winget; $ok = $LASTEXITCODE -eq 0 }; if (-not $ok) { if (Get-Command npm -ErrorAction SilentlyContinue) { Write-Host \"Trying: npm install -g @openai/codex\"; npm install -g @openai/codex } else { Write-Warning \"winget and npm are not available. Install Node.js from https://nodejs.org, then run: npm install -g @openai/codex\" } } }".to_string()
}

pub fn install_info(id: &str) -> Value {
    let win = cfg!(windows);
    let h = home();
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| h.join("AppData").join("Local"));
    let shell = if win { "powershell" } else { "bash" };
    let bin = |name: &str| p(&h.join(".local").join("bin").join(name));
    match id {
        "claude" => json!({
            "id": "claude", "name": "Claude Code", "shell": shell,
            "command": if win { ps_with_npm("https://claude.ai/install.ps1", "@anthropic-ai/claude-code") } else { "curl -fsSL https://claude.ai/install.sh | bash".to_string() },
            "targetPath": if win { bin("claude.exe") } else { bin("claude") }
        }),
        "antigravity" => json!({
            "id": "antigravity", "name": "Antigravity (AGY)", "shell": shell,
            "command": if win { "irm https://antigravity.google/cli/install.ps1 | iex" } else { "curl -fsSL https://antigravity.google/cli/install.sh | bash" },
            "targetPath": if win { p(&local.join("agy").join("bin").join("agy.exe")) } else { bin("agy") }
        }),
        _ => json!({
            "id": "codex", "name": "Codex CLI", "shell": shell,
            "command": if win { ps_codex() } else { "script=\"$(curl -fsSL https://chatgpt.com/codex/install.sh)\" && printf '%s\\n' \"$script\" | sh || { echo \"Official installer failed - falling back to: npm install -g @openai/codex\"; npm install -g @openai/codex; }".to_string() },
            "targetPath": if win { p(&local.join("Programs").join("OpenAI").join("Codex").join("bin").join("codex.exe")) } else { bin("codex") }
        }),
    }
}

pub fn call(core: &Arc<Core>, window: u32, channel: &str, args: &[Value]) -> Result<Value, String> {
    let ws = core.workspace_for(Some(window));
    let custom = || ca::sanitize_custom_agent(core.load_settings().get("customAgent"));
    match channel {
        "ext:customList" => Ok(json!(ca::list_files(&custom(), arg_str(args, 0))?)),
        "ext:customRead" => Ok(json!(ca::read_file(&custom(), arg_str(args, 0), arg_str(args, 1))?)),
        "ext:customWrite" => {
            ca::write_file(&custom(), arg_str(args, 0), arg_str(args, 1), arg_str(args, 2), args.get(3).and_then(Value::as_str))?;
            Ok(Value::Null)
        }
        "ext:inventory" => Ok(json!(inventory(core, &ws))),
        "ext:toggleItem" => {
            toggle(core, &ws, arg_str(args, 0), arg_str(args, 1), args.get(2).and_then(Value::as_bool).unwrap_or(false), args.get(3).and_then(Value::as_str));
            Ok(json!(true))
        }
        "ext:agents" => Ok(json!(agent_status(core, &ws, &inventory(core, &ws)))),
        "ext:manifest" => Ok(read_manifest(&ws)),
        "ext:saveManifest" => {
            write_manifest(&ws, args.first().unwrap_or(&Value::Null))?;
            Ok(Value::Null)
        }
        "ext:planSync" => Ok(json!(plan_sync(&ws, &read_manifest(&ws)))),
        "ext:applySync" => Ok(json!({ "written": apply_sync(&plan_sync(&ws, &read_manifest(&ws)))? })),
        // Only from the user's button, never silently
        "ext:trustWorkspace" => {
            let Some(f) = agent_paths("antigravity").trust_settings else { return Ok(json!(false)) };
            let mut doc = read_json(&f);
            let key = ws.replace('\\', "/").to_lowercase();
            let mut list = doc["trustedWorkspaces"].as_array().cloned().unwrap_or_default();
            if !list.iter().filter_map(Value::as_str).any(|t| t.replace('\\', "/").to_lowercase() == key) {
                list.push(json!(ws));
            }
            doc["trustedWorkspaces"] = json!(list);
            if let Some(d) = f.parent() {
                fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            fs::write(&f, serde_json::to_string_pretty(&doc).unwrap()).map_err(|e| e.to_string())?;
            Ok(json!(true))
        }
        "ext:getAgentInstallInfo" => Ok(install_info(arg_str(args, 0))),
        _ => Err(format!("No handler registered for '{channel}'")),
    }
}
