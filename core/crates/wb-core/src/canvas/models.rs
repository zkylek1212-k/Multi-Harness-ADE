//! The model and effort a Bot runs with (P7), per node. Unset = the CLI's own default from its config.
//!   Claude   --model <alias or name> --effort <level>, chat and terminal alike
//!   Codex    -c model=<slug> -c model_reasoning_effort=<level>; the list comes from `codex debug models`
//!   agy      ids carry the effort (gemini-4-neon-high): `--model` in its terminal, ACP session/set_model in chat
//! A custom agent follows its chat protocol's CLI.
use std::collections::HashMap;

use super::*;

const EFFORTS: [&str; 7] = ["minimal", "low", "medium", "high", "xhigh", "max", "ultra"];
const CACHE_MS: f64 = 10.0 * 60_000.0;

/// A model name goes on a command line: letters, digits and . _ : / - [ ], never a leading dash
pub fn model_ok(m: &str) -> bool {
    !m.is_empty() && m.len() <= 100 && !m.starts_with('-') && m.chars().all(|c| c.is_ascii_alphanumeric() || "._:/-[]".contains(c))
}

pub fn effort_ok(e: &str) -> bool {
    EFFORTS.contains(&e)
}

/// The model and effort in a request; empty means the CLI's default
pub(super) fn pick(req: &Value) -> R<(Option<String>, Option<String>)> {
    let m = s(req, "model").trim();
    let e = s(req, "effort").trim();
    if !m.is_empty() && !model_ok(m) {
        return Err(err("invalid", "Not a model name"));
    }
    if !e.is_empty() && !effort_ok(e) {
        return Err(err("invalid", "Effort is low, medium, high, xhigh or max"));
    }
    Ok((Some(m.to_string()).filter(|x| !x.is_empty()), Some(e.to_string()).filter(|x| !x.is_empty())))
}

/// Which CLI's flags a node takes: claude, codex or agy; None for a shell or a terminal-only custom agent
pub(super) fn cli_of(core: &Core, agent: &str) -> Option<&'static str> {
    match agent {
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        "antigravity" => Some("agy"),
        crate::custom_agent::CUSTOM_AGENT_ID => match chat::protocol(core, agent) {
            Some("claude-stream-json") => Some("claude"),
            Some("codex-app-server") => Some("codex"),
            Some("acp") => Some("agy"),
            _ => None,
        },
        _ => None,
    }
}

/// Launch flags. agy's chat (ACP) sets its model over the protocol instead, so it gets none here.
pub fn args(cli: &str, chat: bool, model: Option<&str>, effort: Option<&str>) -> Vec<String> {
    let mut a = Vec::new();
    match cli {
        "claude" => {
            if let Some(m) = model {
                a.extend(["--model".to_string(), m.to_string()]);
            }
            if let Some(e) = effort {
                a.extend(["--effort".to_string(), e.to_string()]);
            }
        }
        // Raw TOML strings: codex.cmd runs through cmd, which mangles quotes, and a bare word parses as a string
        "codex" => {
            if let Some(m) = model {
                a.extend(["-c".to_string(), format!("model={m}")]);
            }
            if let Some(e) = effort {
                a.extend(["-c".to_string(), format!("model_reasoning_effort={e}")]);
            }
        }
        "agy" if !chat => {
            if let Some(m) = model {
                a.extend(["--model".to_string(), m.to_string()]);
            }
        }
        _ => {}
    }
    a
}

static CACHE: Mutex<Option<HashMap<String, (f64, Value)>>> = Mutex::new(None);

fn run(core: &Core, agent: &str, extra: &[&str]) -> Option<String> {
    let (command, mut pre, env) = core.agent_process(agent).ok()?;
    pre.extend(extra.iter().map(|s| s.to_string()));
    let r = wb_proc::run_process(&wb_proc::RunSpec { command, args: pre, cwd: p(&core.data_dir), stdin: String::new(), timeout_ms: 60_000, max_bytes: 8 << 20, env: Some(env) }, &wb_proc::CancelToken::default());
    (r.code == Some(0)).then_some(r.stdout)
}

/// `codex debug models`: the listed ones, each with the efforts it supports
fn codex_models(raw: &str) -> Vec<Value> {
    let start = raw.find('{').unwrap_or(0);
    let Ok(v) = serde_json::Deserializer::from_str(&raw[start..]).into_iter::<Value>().next().unwrap_or(Ok(Value::Null)) else { return vec![] };
    v["models"].as_array().into_iter().flatten().filter(|m| m["visibility"] == "list").filter_map(|m| {
        let id = m["slug"].as_str().filter(|s| model_ok(s))?;
        let efforts: Vec<&str> = m["supported_reasoning_levels"].as_array().into_iter().flatten().filter_map(|l| l["effort"].as_str().or(l.as_str())).filter(|e| effort_ok(e)).collect();
        Some(json!({ "id": id, "name": m["display_name"].as_str().unwrap_or(id), "efforts": efforts, "defaultEffort": m["default_reasoning_level"] }))
    }).collect()
}

/// `agy models`: "<id>\t<name>" per line
fn agy_models(raw: &str) -> Vec<Value> {
    raw.lines().filter_map(|l| {
        let (id, name) = l.split_once('\t')?;
        let id = id.trim();
        model_ok(id).then(|| json!({ "id": id, "name": name.trim() }))
    }).collect()
}

/// For the pickers: the models a Bot can run, the efforts it takes (empty: the model carries it), whether any name may be typed
pub(super) fn list(core: &Core, agent: &str) -> R<Value> {
    let cli = cli_of(core, agent).ok_or_else(|| err("invalid", format!("{agent} has no model choice")))?;
    let key = format!("{agent}|{}", core.agent_process(agent).map(|(c, _, _)| c).unwrap_or_default());
    if let Some((at, v)) = CACHE.lock().unwrap().get_or_insert_with(HashMap::new).get(&key) {
        if now_ms() - at < CACHE_MS {
            return Ok(v.clone());
        }
    }
    let v = match cli {
        "claude" => json!({
            "models": [{ "id": "fable", "name": "Fable (latest)" }, { "id": "opus", "name": "Opus (latest)" }, { "id": "sonnet", "name": "Sonnet (latest)" }, { "id": "haiku", "name": "Haiku (latest)" }],
            "efforts": ["low", "medium", "high", "xhigh", "max"],
        }),
        "codex" => json!({ "models": run(core, agent, &["debug", "models"]).map(|o| codex_models(&o)).unwrap_or_default(), "efforts": ["low", "medium", "high", "xhigh", "max"] }),
        _ => json!({ "models": run(core, agent, &["models"]).map(|o| agy_models(&o)).unwrap_or_default(), "efforts": [] }),
    };
    CACHE.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, (now_ms(), v.clone()));
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_per_cli() {
        assert_eq!(args("claude", true, Some("haiku"), Some("low")), ["--model", "haiku", "--effort", "low"]);
        assert_eq!(args("codex", false, Some("gpt-6.1-terra"), Some("high")), ["-c", "model=gpt-6.1-terra", "-c", "model_reasoning_effort=high"]);
        assert_eq!(args("agy", false, Some("gemini-4-neon-high"), Some("low")), ["--model", "gemini-4-neon-high"], "agy's id carries the effort");
        assert!(args("agy", true, Some("gemini-4-neon-high"), None).is_empty(), "agy chat sets it over ACP");
        assert!(args("claude", true, None, None).is_empty(), "unset: the CLI's own default");
    }

    #[test]
    fn names_are_checked() {
        assert!(model_ok("claude-sonnet-5-5") && model_ok("sonnet[1m]") && model_ok("gpt-6.1-terra"));
        assert!(!model_ok("--dangerously-skip-permissions") && !model_ok("a b") && !model_ok("x;rm") && !model_ok(""));
        assert!(effort_ok("xhigh") && !effort_ok("turbo"));
        assert!(pick(&json!({ "model": "-x" })).is_err() && pick(&json!({ "effort": "loud" })).is_err());
        assert_eq!(pick(&json!({ "model": " opus ", "effort": "" })).unwrap(), (Some("opus".into()), None));
    }

    #[test]
    fn catalogs_parse() {
        let codex = r#"noise {"models":[{"slug":"gpt-6.1-terra","display_name":"GPT-6.1 Terra","visibility":"list","supported_reasoning_levels":[{"effort":"low"},{"effort":"ultra"}],"default_reasoning_level":"low"},{"slug":"hidden","visibility":"hide"}]}"#;
        let m = codex_models(codex);
        assert_eq!(m.len(), 1);
        assert_eq!((m[0]["id"].as_str(), m[0]["efforts"].as_array().unwrap().len()), (Some("gpt-6.1-terra"), 2));
        let agy = "Fetching available models...\ngemini-4-neon-high\tGemini 4 Neon (High)\nbad line\n";
        assert_eq!(agy_models(agy), vec![json!({ "id": "gemini-4-neon-high", "name": "Gemini 4 Neon (High)" })]);
    }
}
