//! `wb-hook mcp`: the stdio MCP server each canvas node's CLI starts (cowork-team-implement.md §4). It only relays:
//! the tool list and every call go to the app over the same named pipe and token as the hook, so the app decides
//! who is asking and what they may do. Outside the app (no pipe or token in the environment) it offers no tools,
//! which matters for agy: its MCP entry is user-wide, so every agy session starts this server.
//!
//! Wire protocol, one pipe connection per request:
//!   mcp -> app: `WBMCP1 <token> <byte length>\n` then a JSON body {"op":"list"} or {"op":"call","name","arguments"}
//!   app -> mcp: `<byte length>\n` then a JSON body {"tools":[...]} or {"ok":bool,"text":"..."}
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{json, Value};

const CALL_WAIT: Duration = Duration::from_secs(120);
const MAX_REPLY: usize = 8 * 1024 * 1024;

pub fn run() {
    let session = crate::env_ok("WB_HOOK_PIPE", |v| v.strip_prefix(crate::PIPE_PREFIX).is_some_and(crate::safe_id)).ok().zip(crate::env_ok("WB_HOOK_TOKEN", crate::safe_id).ok());
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(m) = serde_json::from_str::<Value>(&line) else { continue };
        // Notifications carry no id and get no answer
        let Some(id) = m.get("id").cloned() else { continue };
        let result = handle(&m, session.as_ref());
        let reply = match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err((code, msg)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": msg } }),
        };
        let _ = writeln!(out, "{reply}");
        let _ = out.flush();
    }
}

fn handle(m: &Value, session: Option<&(String, String)>) -> Result<Value, (i64, String)> {
    match m["method"].as_str().unwrap_or("") {
        "initialize" => Ok(json!({
            "protocolVersion": m["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "workbench", "version": env!("CARGO_PKG_VERSION") },
        })),
        "tools/list" => {
            let tools = session.and_then(|s| ask(s, &json!({ "op": "list" })).ok()).and_then(|v| v.get("tools").cloned()).unwrap_or(json!([]));
            Ok(json!({ "tools": tools }))
        }
        "tools/call" => {
            let Some(s) = session else { return Ok(text("This session is not running inside Agent Workbench.", true)) };
            let req = json!({ "op": "call", "name": m["params"]["name"], "arguments": m["params"].get("arguments").cloned().unwrap_or(json!({})) });
            Ok(match ask(s, &req) {
                Ok(v) => text(v["text"].as_str().unwrap_or(""), v["ok"] != json!(true)),
                Err(e) => text(&format!("Agent Workbench did not answer: {e}"), true),
            })
        }
        "ping" => Ok(json!({})),
        // Claude sends server/discover and similar probes; an empty result keeps every client going (Phase 0)
        _ => Ok(json!({})),
    }
}

fn text(t: &str, error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": t }], "isError": error })
}

fn ask((pipe, token): &(String, String), req: &Value) -> Result<Value, String> {
    let body = serde_json::to_vec(req).map_err(|e| e.to_string())?;
    let mut conn = crate::open(pipe)?;
    conn.write_all(format!("WBMCP1 {token} {}\n", body.len()).as_bytes()).map_err(|e| e.to_string())?;
    conn.write_all(&body).map_err(|e| e.to_string())?;
    conn.flush().map_err(|e| e.to_string())?;
    // std has no read timeout on a pipe, so the read runs on its own thread
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut r = BufReader::new(conn);
        let mut head = String::new();
        let res = r.read_line(&mut head).map_err(|e| e.to_string()).and_then(|_| {
            let len: usize = head.trim().parse().map_err(|_| "bad reply".to_string())?;
            if len > MAX_REPLY {
                return Err("reply too large".into());
            }
            let mut buf = vec![0; len];
            r.read_exact(&mut buf).map_err(|e| e.to_string())?;
            serde_json::from_slice::<Value>(&buf).map_err(|e| e.to_string())
        });
        let _ = tx.send(res);
    });
    rx.recv_timeout(CALL_WAIT).map_err(|_| "timed out".to_string())?
}
