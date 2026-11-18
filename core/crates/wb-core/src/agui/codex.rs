//! Codex `app-server`: JSON-RPC over stdio, one JSON object per line, no `jsonrpc` field (cowork-agui-implement.md §4.2,
//! P0-verified on 0.169.1). Method names come from `codex app-server generate-json-schema`.
use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use super::*;

pub fn args() -> Vec<String> {
    vec!["app-server".into()]
}

/// What this harness needs from an app-server, by the schema file that declares it. Checked against the schema the
/// executable generates itself (`app-server generate-json-schema`), so a Codex upgrade or a fork that drops one of
/// these fails before a chat starts instead of in the middle of one.
pub const REQUIRED: [(&str, &str); 15] = [
    ("ClientRequest.json", "initialize"),
    ("ClientRequest.json", "thread/start"),
    ("ClientRequest.json", "thread/resume"),
    ("ClientRequest.json", "turn/start"),
    ("ClientRequest.json", "turn/interrupt"),
    ("ServerNotification.json", "turn/started"),
    ("ServerNotification.json", "turn/completed"),
    ("ServerNotification.json", "item/started"),
    ("ServerNotification.json", "item/completed"),
    ("ServerNotification.json", "item/agentMessage/delta"),
    ("ServerNotification.json", "thread/tokenUsage/updated"),
    ("ServerRequest.json", "item/commandExecution/requestApproval"),
    ("ServerRequest.json", "item/fileChange/requestApproval"),
    ("CommandExecutionRequestApprovalResponse.json", "acceptForSession"),
    ("CommandExecutionRequestApprovalResponse.json", "decline"),
];

/// The required names a generated schema folder lacks; empty when this harness can drive it
pub fn missing(schema_dir: &std::path::Path) -> Vec<String> {
    REQUIRED
        .iter()
        .filter(|(file, name)| !std::fs::read_to_string(schema_dir.join(file)).is_ok_and(|t| t.contains(&format!("\"{name}\""))))
        .map(|(_, name)| name.to_string())
        .collect()
}

const INIT: u64 = 1;
const THREAD: u64 = 2;

pub struct Codex {
    thread: String,
    cwd: String,
    resume: Option<String>,
    sandbox: String,
    next_id: u64,
    codex_thread: Option<String>,
    queued: Vec<String>,
    turn: Option<String>,
    messages: HashSet<String>,
    tools: HashSet<String>,
    /// approval key -> the request's own JSON-RPC id
    asks: HashMap<String, Value>,
}

impl Codex {
    /// `sandbox`: read-only | workspace-write. Anything outside it comes back as an approval.
    pub fn new(thread: &str, cwd: &str, resume: Option<&str>, sandbox: &str) -> Self {
        Codex { thread: thread.into(), cwd: cwd.into(), resume: resume.map(str::to_string), sandbox: sandbox.into(), next_id: 3, codex_thread: None, queued: vec![], turn: None, messages: HashSet::new(), tools: HashSet::new(), asks: HashMap::new() }
    }

    fn request(&mut self, method: &str, params: Value) -> Out {
        let id = self.next_id;
        self.next_id += 1;
        send(json!({ "id": id, "method": method, "params": params }))
    }

    fn turn_start(&mut self, text: &str) -> Out {
        let tid = self.codex_thread.clone().unwrap_or_default();
        self.request("turn/start", json!({ "threadId": tid, "input": [{ "type": "text", "text": text }] }))
    }

    fn tool_name(item: &Value) -> Option<String> {
        Some(match item["type"].as_str()? {
            "commandExecution" => "shell".into(),
            "fileChange" => "edit".into(),
            "mcpToolCall" => format!("{}.{}", item["server"].as_str().unwrap_or("mcp"), item["tool"].as_str().unwrap_or("tool")),
            "webSearch" => "web_search".into(),
            "dynamicToolCall" => item["tool"].as_str().unwrap_or("tool").into(),
            _ => return None,
        })
    }

    fn tool_args(item: &Value) -> Value {
        match item["type"].as_str() {
            Some("commandExecution") => json!({ "command": item["command"], "cwd": item["cwd"] }),
            Some("fileChange") => json!({ "changes": item["changes"] }),
            Some("mcpToolCall") | Some("dynamicToolCall") => item["arguments"].clone(),
            Some("webSearch") => json!({ "query": item["query"] }),
            _ => Value::Null,
        }
    }

    fn tool_output(item: &Value) -> String {
        match item["type"].as_str() {
            Some("commandExecution") => {
                let out = item["aggregatedOutput"].as_str().unwrap_or("");
                match item["exitCode"].as_i64() {
                    Some(c) if c != 0 => format!("{out}\n(exit {c})"),
                    _ => out.to_string(),
                }
            }
            Some("mcpToolCall") => text_of(if item["error"].is_null() { &item["result"] } else { &item["error"] }),
            _ => item["status"].as_str().unwrap_or("").to_string(),
        }
    }

    fn start_tool(&mut self, item: &Value) -> Vec<Out> {
        let (Some(id), Some(name)) = (item["id"].as_str(), Self::tool_name(item)) else { return vec![] };
        if !self.tools.insert(id.to_string()) {
            return vec![];
        }
        vec![
            Out::Ui(Event::ToolCallStart { tool_call_id: id.into(), tool_call_name: name }),
            Out::Ui(Event::ToolCallArgs { tool_call_id: id.into(), delta: Self::tool_args(item).to_string() }),
        ]
    }

    fn close_all(&mut self) -> Vec<Out> {
        let mut out: Vec<Out> = self.messages.drain().map(|id| Out::Ui(Event::TextMessageEnd { message_id: id })).collect();
        out.extend(self.tools.drain().map(|id| Out::Ui(Event::ToolCallEnd { tool_call_id: id })));
        out
    }

    fn approval(&mut self, method: &str, id: &Value, p: &Value) -> Vec<Out> {
        let key = format!("codex-{}", rpc_id_key(id));
        let (tool, summary) = match method {
            "item/commandExecution/requestApproval" => {
                let cmd = p["commandActions"][0]["command"].as_str().or(p["command"].as_str()).unwrap_or("command");
                ("shell", cmd.to_string())
            }
            "item/fileChange/requestApproval" => ("edit", p["reason"].as_str().unwrap_or("file change").to_string()),
            _ => unreachable!(),
        };
        self.asks.insert(key.clone(), id.clone());
        vec![Out::Approval(Approval { id: key, tool: tool.into(), summary, input: p.clone(), session_scope: true })]
    }
}

impl Harness for Codex {
    fn start(&mut self) -> Vec<Out> {
        let thread = match &self.resume {
            Some(t) => json!({ "id": THREAD, "method": "thread/resume", "params": { "threadId": t, "cwd": self.cwd, "approvalPolicy": "on-request", "sandbox": self.sandbox, "excludeTurns": true } }),
            None => json!({ "id": THREAD, "method": "thread/start", "params": { "cwd": self.cwd, "approvalPolicy": "on-request", "sandbox": self.sandbox } }),
        };
        vec![
            send(json!({ "id": INIT, "method": "initialize", "params": { "clientInfo": { "name": "agent-workbench", "version": env!("CARGO_PKG_VERSION") } } })),
            send(json!({ "method": "initialized" })),
            send(thread),
        ]
    }

    fn on_line(&mut self, line: &str) -> Vec<Out> {
        let Some(m) = json_line(line) else { return vec![] };
        let method = m["method"].as_str().unwrap_or("");
        let p = &m["params"];
        // Server -> client requests carry both an id and a method
        if !method.is_empty() && m.get("id").is_some() {
            return match method {
                "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => self.approval(method, &m["id"], p),
                // Anything else this client cannot answer is refused, never left hanging
                _ => vec![send(json!({ "id": m["id"], "error": { "code": -32601, "message": format!("{method} is not supported by Agent Workbench") } }))],
            };
        }
        if method.is_empty() {
            if m["id"] == THREAD {
                if let Some(t) = m["result"]["thread"]["id"].as_str() {
                    self.codex_thread = Some(t.to_string());
                    let mut out = vec![Out::Session(t.to_string())];
                    for text in std::mem::take(&mut self.queued) {
                        out.push(self.turn_start(&text));
                    }
                    return out;
                }
                let message = m["error"]["message"].as_str().unwrap_or("codex could not open the thread").to_string();
                return vec![Out::Ui(Event::RunError { message, code: None })];
            }
            return vec![];
        }
        match method {
            "turn/started" => {
                let run = p["turn"]["id"].as_str().unwrap_or_default().to_string();
                self.turn = Some(run.clone());
                vec![Out::Ui(Event::RunStarted { thread_id: self.thread.clone(), run_id: run })]
            }
            "item/started" | "item/completed" => {
                let item = &p["item"];
                let id = item["id"].as_str().unwrap_or_default().to_string();
                let done = method == "item/completed";
                if item["type"] == "agentMessage" {
                    let mut out = vec![];
                    if self.messages.insert(id.clone()) {
                        out.push(Out::Ui(Event::TextMessageStart { message_id: id.clone(), role: "assistant".into() }));
                    }
                    if done {
                        self.messages.remove(&id);
                        out.push(Out::Ui(Event::TextMessageEnd { message_id: id }));
                    }
                    return out;
                }
                let mut out = self.start_tool(item);
                if done && self.tools.remove(&id) {
                    out.push(Out::Ui(Event::ToolCallEnd { tool_call_id: id.clone() }));
                    out.push(tool_result(&id, clip(&Self::tool_output(item))));
                }
                out
            }
            "item/agentMessage/delta" => {
                let id = p["itemId"].as_str().unwrap_or_default().to_string();
                let mut out = vec![];
                if self.messages.insert(id.clone()) {
                    out.push(Out::Ui(Event::TextMessageStart { message_id: id.clone(), role: "assistant".into() }));
                }
                out.push(Out::Ui(Event::TextMessageContent { message_id: id, delta: p["delta"].as_str().unwrap_or("").into() }));
                out
            }
            "thread/tokenUsage/updated" => vec![usage(p["tokenUsage"].clone())],
            "turn/completed" => {
                let mut out = self.close_all();
                let t = &p["turn"];
                let run = t["id"].as_str().unwrap_or_default().to_string();
                self.turn = None;
                out.push(Out::Ui(match t["status"].as_str() {
                    Some("completed") => Event::RunFinished { thread_id: self.thread.clone(), run_id: run },
                    Some("interrupted") => Event::RunError { message: "stopped".into(), code: Some(INTERRUPTED.into()) },
                    _ => Event::RunError { message: t["error"]["message"].as_str().unwrap_or("the turn failed").into(), code: None },
                }));
                out
            }
            _ => vec![],
        }
    }

    fn prompt(&mut self, text: &str) -> Vec<Out> {
        if self.codex_thread.is_none() {
            self.queued.push(text.to_string());
            return vec![];
        }
        vec![self.turn_start(text)]
    }

    fn interrupt(&mut self) -> Vec<Out> {
        match (self.codex_thread.clone(), self.turn.clone()) {
            (Some(t), Some(turn)) => vec![self.request("turn/interrupt", json!({ "threadId": t, "turnId": turn }))],
            _ => vec![],
        }
    }

    fn answer(&mut self, id: &str, allow: bool, for_session: bool) -> Vec<Out> {
        let Some(rid) = self.asks.remove(id) else { return vec![] };
        let decision = match (allow, for_session) {
            (false, _) => "decline",
            (true, true) => "acceptForSession",
            (true, false) => "accept",
        };
        vec![send(json!({ "id": rid, "result": { "decision": decision } }))]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agui::replay;

    const RUN: &str = include_str!("../../../../../scripts/fixtures/agui/codex-app-server.ndjson");
    const RESUME: &str = include_str!("../../../../../scripts/fixtures/agui/codex-app-server-resume.ndjson");

    /// The driver numbered its requests like the harness does (initialize 1, thread 2), so its turn/start and
    /// approval answers can be replayed through the harness and compared.
    fn drive(h: &mut dyn Harness, v: &Value) -> Vec<Out> {
        match v["method"].as_str() {
            Some("turn/start") => h.prompt(v["params"]["input"][0]["text"].as_str().unwrap()),
            Some("turn/interrupt") => h.interrupt(),
            None if v.get("result").is_some() => h.answer(&format!("codex-{}", rpc_id_key(&v["id"])), true, false),
            _ => vec![],
        }
    }

    #[test]
    fn replays_the_p0_session() {
        let mut h = Codex::new("t1", "C:/repo", None, "read-only");
        let seen = replay::run(&mut h, RUN, drive);
        seen.assert_balanced();
        assert_eq!(seen.sessions, ["01a1ede7-fb43-7773-9026-673829d84683"]);
        assert_eq!(seen.count(|e| matches!(e, Event::RunStarted { .. })), 4);
        assert_eq!(seen.count(|e| matches!(e, Event::RunFinished { .. })), 3);
        assert_eq!(seen.count(|e| matches!(e, Event::RunError { code: Some(c), .. } if c == INTERRUPTED)), 1);
        assert_eq!(seen.approvals.len(), 2);
        assert_eq!(seen.approvals[1].summary, "node --test");
        assert!(seen.text().contains("KIWI-9") && seen.text().contains("MANGO-3"));
        assert!(seen.count(|e| matches!(e, Event::ToolCallResult { .. })) >= 5);
        let first: Value = serde_json::from_str(&seen.sent[0]).unwrap();
        assert_eq!(first["method"], "initialize");
        let accept: Vec<&String> = seen.sent.iter().filter(|s| s.contains("\"decision\":\"accept\"")).collect();
        assert_eq!(accept.len(), 2);
    }

    #[test]
    fn resume_asks_for_the_old_thread_and_waits_for_it() {
        let mut h = Codex::new("t1", "C:/repo", Some("01a1ede7-fb43-7773-9026-673829d84683"), "workspace-write");
        let seen = replay::run(&mut h, RESUME, drive);
        seen.assert_balanced();
        assert!(seen.sent[2].contains("thread/resume"));
        assert!(seen.text().contains("sum.js"));
    }

    #[test]
    fn prompt_before_the_thread_opens_is_queued() {
        let mut h = Codex::new("t", "C:/r", None, "read-only");
        h.start();
        assert!(h.prompt("hi").is_empty());
        let outs = h.on_line(r#"{"id":2,"result":{"thread":{"id":"th"}}}"#);
        assert!(matches!(&outs[0], Out::Session(s) if s == "th"));
        assert!(matches!(&outs[1], Out::Send(s) if s.contains("turn/start") && s.contains("\"th\"")));
    }

    #[test]
    fn missing_lists_what_a_schema_folder_lacks() {
        let dir = std::env::temp_dir().join(format!("wb-codex-schema-{}", crate::util::uuid()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(missing(&dir).len(), REQUIRED.len());
        for (file, name) in REQUIRED {
            let path = dir.join(file);
            let mut t = std::fs::read_to_string(&path).unwrap_or_default();
            t.push_str(&format!("\"{name}\"
"));
            std::fs::write(&path, t).unwrap();
        }
        assert!(missing(&dir).is_empty());
        std::fs::write(dir.join("ServerRequest.json"), "\"item/fileChange/requestApproval\"").unwrap();
        assert_eq!(missing(&dir), ["item/commandExecution/requestApproval"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_server_requests_are_refused() {
        let mut h = Codex::new("t", "C:/r", None, "read-only");
        let outs = h.on_line(r#"{"id":9,"method":"item/tool/requestUserInput","params":{}}"#);
        assert!(matches!(&outs[..], [Out::Send(s)] if s.contains("\"error\"")));
    }
}
