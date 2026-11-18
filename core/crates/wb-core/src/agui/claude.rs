//! Claude Code headless: `claude -p` with stream-json both ways (cowork-agui-implement.md §4.1, P0-verified on 2.1.327).
use std::collections::HashMap;

use serde_json::{json, Value};

use super::*;

/// `permission_mode`: Claude's own mode for this session; tool calls it would ask about come back as approvals
pub fn args(resume: Option<&str>, permission_mode: &str, mcp_config: Option<&str>) -> Vec<String> {
    let mut a: Vec<String> = ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--permission-prompts", "host", "--permission-prompt-tool", "stdio", "--permission-mode", permission_mode]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if let Some(m) = mcp_config {
        a.extend(["--mcp-config".to_string(), m.to_string()]);
    }
    if let Some(r) = resume {
        a.extend(["--resume".to_string(), r.to_string()]);
    }
    a
}

enum Block {
    Text(String),
    Tool(String),
    Other,
}

pub struct Claude {
    thread: String,
    runs: u32,
    run: Option<String>,
    message: String,
    blocks: HashMap<u64, Block>,
    /// request id -> (tool input, the CLI's suggested rules for "allow for this session")
    asks: HashMap<String, (Value, Value)>,
    interrupting: bool,
    session: Option<String>,
}

impl Claude {
    pub fn new(thread: &str) -> Self {
        Claude { thread: thread.into(), runs: 0, run: None, message: String::new(), blocks: HashMap::new(), asks: HashMap::new(), interrupting: false, session: None }
    }

    /// Close what a stream left open (a run that ends mid-block)
    fn close_blocks(&mut self) -> Vec<Out> {
        let mut out = vec![];
        let mut keys: Vec<u64> = self.blocks.keys().copied().collect();
        keys.sort();
        for k in keys {
            match self.blocks.remove(&k) {
                Some(Block::Text(id)) => out.push(Out::Ui(Event::TextMessageEnd { message_id: id })),
                Some(Block::Tool(id)) => out.push(Out::Ui(Event::ToolCallEnd { tool_call_id: id })),
                _ => {}
            }
        }
        out
    }

    fn stream(&mut self, e: &Value) -> Vec<Out> {
        let idx = e["index"].as_u64().unwrap_or(0);
        match e["type"].as_str().unwrap_or("") {
            "message_start" => {
                self.message = e["message"]["id"].as_str().unwrap_or("msg").to_string();
                vec![]
            }
            "content_block_start" => {
                let b = &e["content_block"];
                match b["type"].as_str() {
                    Some("text") => {
                        let id = format!("{}:{idx}", self.message);
                        self.blocks.insert(idx, Block::Text(id.clone()));
                        vec![Out::Ui(Event::TextMessageStart { message_id: id, role: "assistant".into() })]
                    }
                    Some("tool_use") | Some("server_tool_use") | Some("mcp_tool_use") => {
                        let id = b["id"].as_str().unwrap_or_default().to_string();
                        self.blocks.insert(idx, Block::Tool(id.clone()));
                        vec![Out::Ui(Event::ToolCallStart { tool_call_id: id, tool_call_name: b["name"].as_str().unwrap_or("tool").into() })]
                    }
                    _ => {
                        self.blocks.insert(idx, Block::Other);
                        vec![]
                    }
                }
            }
            "content_block_delta" => {
                let d = &e["delta"];
                match (self.blocks.get(&idx), d["type"].as_str()) {
                    (Some(Block::Text(id)), Some("text_delta")) => vec![Out::Ui(Event::TextMessageContent { message_id: id.clone(), delta: d["text"].as_str().unwrap_or("").into() })],
                    (Some(Block::Tool(id)), Some("input_json_delta")) => {
                        let part = d["partial_json"].as_str().unwrap_or("");
                        if part.is_empty() {
                            vec![]
                        } else {
                            vec![Out::Ui(Event::ToolCallArgs { tool_call_id: id.clone(), delta: part.into() })]
                        }
                    }
                    _ => vec![],
                }
            }
            "content_block_stop" => match self.blocks.remove(&idx) {
                Some(Block::Text(id)) => vec![Out::Ui(Event::TextMessageEnd { message_id: id })],
                Some(Block::Tool(id)) => vec![Out::Ui(Event::ToolCallEnd { tool_call_id: id })],
                _ => vec![],
            },
            _ => vec![],
        }
    }
}

pub fn summary(tool: &str, input: &Value) -> String {
    let field = |k: &str| input[k].as_str().map(str::to_string);
    match tool {
        "Bash" | "PowerShell" => field("command"),
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "Read" => field("file_path").or_else(|| field("notebook_path")),
        "WebFetch" => field("url"),
        _ => None,
    }
    .unwrap_or_else(|| tool.to_string())
}

impl Harness for Claude {
    fn start(&mut self) -> Vec<Out> {
        vec![]
    }

    fn on_line(&mut self, line: &str) -> Vec<Out> {
        let Some(m) = json_line(line) else { return vec![] };
        match m["type"].as_str().unwrap_or("") {
            "system" if m["subtype"] == "init" => match m["session_id"].as_str() {
                Some(s) if self.session.as_deref() != Some(s) => {
                    self.session = Some(s.to_string());
                    vec![Out::Session(s.to_string())]
                }
                _ => vec![],
            },
            // A subagent's own stream would interleave with the main one; its result comes back as the Task tool's result
            "stream_event" if m["parent_tool_use_id"].is_null() => self.stream(&m["event"]),
            "user" => {
                let mut out = vec![];
                for c in m["message"]["content"].as_array().into_iter().flatten() {
                    if c["type"] == "tool_result" && m["parent_tool_use_id"].is_null() {
                        out.push(tool_result(c["tool_use_id"].as_str().unwrap_or_default(), clip(&text_of(&c["content"]))));
                    }
                }
                out
            }
            "control_request" if m["request"]["subtype"] == "can_use_tool" => {
                let id = rpc_id_key(&m["request_id"]);
                let r = &m["request"];
                let tool = r["tool_name"].as_str().unwrap_or("tool").to_string();
                let suggestions = r.get("permission_suggestions").cloned().unwrap_or(Value::Null);
                let session_scope = suggestions.as_array().is_some_and(|a| !a.is_empty());
                self.asks.insert(id.clone(), (r["input"].clone(), suggestions));
                vec![Out::Approval(Approval { id, summary: summary(&tool, &r["input"]), tool, input: r["input"].clone(), session_scope })]
            }
            "result" => {
                let mut out = self.close_blocks();
                let run = self.run.take().unwrap_or_default();
                if let Some(u) = m.get("usage") {
                    out.push(usage(json!({ "usage": u, "costUsd": m["total_cost_usd"] })));
                }
                let interrupted = std::mem::take(&mut self.interrupting);
                if m["is_error"] == true || m["subtype"] != "success" {
                    let message = m["result"].as_str().map(str::to_string).unwrap_or_else(|| m["subtype"].as_str().unwrap_or("error").to_string());
                    out.push(Out::Ui(Event::RunError { message, code: interrupted.then(|| INTERRUPTED.to_string()) }));
                } else {
                    out.push(Out::Ui(Event::RunFinished { thread_id: self.thread.clone(), run_id: run }));
                }
                out
            }
            _ => vec![],
        }
    }

    fn prompt(&mut self, text: &str) -> Vec<Out> {
        self.runs += 1;
        let run = format!("run-{}", self.runs);
        self.run = Some(run.clone());
        vec![
            Out::Ui(Event::RunStarted { thread_id: self.thread.clone(), run_id: run }),
            send(json!({ "type": "user", "message": { "role": "user", "content": text } })),
        ]
    }

    fn interrupt(&mut self) -> Vec<Out> {
        if self.run.is_none() {
            return vec![];
        }
        self.interrupting = true;
        vec![send(json!({ "type": "control_request", "request_id": format!("interrupt-{}", self.runs), "request": { "subtype": "interrupt" } }))]
    }

    fn answer(&mut self, id: &str, allow: bool, for_session: bool) -> Vec<Out> {
        let Some((input, suggestions)) = self.asks.remove(id) else { return vec![] };
        let mut response = if allow { json!({ "behavior": "allow", "updatedInput": input }) } else { json!({ "behavior": "deny", "message": "The user denied this action." }) };
        if allow && for_session && suggestions.is_array() {
            response["updatedPermissions"] = suggestions;
        }
        vec![send(json!({ "type": "control_response", "response": { "subtype": "success", "request_id": id, "response": response } }))]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agui::replay;

    const STREAM: &str = include_str!("../../../../../scripts/fixtures/agui/claude-stream.ndjson");

    #[test]
    fn replays_the_p0_session() {
        let mut h = Claude::new("t1");
        let seen = replay::run(&mut h, STREAM, |h, v| match v["type"].as_str() {
            Some("user") => h.prompt(v["message"]["content"].as_str().unwrap()),
            Some("control_request") => h.interrupt(),
            Some("control_response") => {
                // The driver allowed every tool; answer the same way so the harness forgets the request
                h.answer(v["response"]["request_id"].as_str().unwrap(), true, false)
            }
            _ => vec![],
        });
        seen.assert_balanced();
        assert_eq!(seen.sessions.len(), 1);
        assert_eq!(seen.count(|e| matches!(e, Event::RunStarted { .. })), 4);
        assert_eq!(seen.count(|e| matches!(e, Event::RunFinished { .. })), 3);
        assert_eq!(seen.count(|e| matches!(e, Event::RunError { code: Some(c), .. } if c == INTERRUPTED)), 1);
        let tools: Vec<&str> = seen.approvals.iter().map(|a| a.tool.as_str()).collect();
        assert_eq!(tools, ["Edit", "PowerShell"]);
        assert_eq!(seen.approvals[1].summary, "node --test");
        assert!(seen.text().contains("KIWI-9"), "memory codeword from the SessionStart hook");
        assert!(seen.count(|e| matches!(e, Event::ToolCallResult { .. })) >= 3);
        // The tool args stream rebuilds the tool input
        let args: String = seen.events.iter().filter_map(|e| if let Event::ToolCallArgs { delta, .. } = e { Some(delta.as_str()) } else { None }).collect();
        assert!(args.contains("node --test"));
    }

    #[test]
    fn answers_once_and_denies_with_a_message() {
        let mut h = Claude::new("t");
        let ask = r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"rm -rf x"}}}"#;
        let [Out::Approval(a)] = &h.on_line(ask)[..] else { panic!() };
        assert_eq!((a.summary.as_str(), a.session_scope), ("rm -rf x", false));
        let [Out::Send(s)] = &h.answer("r1", false, false)[..] else { panic!() };
        let v: Value = serde_json::from_str(s).unwrap();
        assert_eq!(v["response"]["response"]["behavior"], "deny");
        assert!(h.answer("r1", true, false).is_empty(), "a request is answered once");
    }

    #[test]
    fn event_json_is_ag_ui() {
        let e = Event::TextMessageContent { message_id: "m".into(), delta: "hi".into() };
        assert_eq!(serde_json::to_value(&e).unwrap(), json!({ "type": "TEXT_MESSAGE_CONTENT", "messageId": "m", "delta": "hi" }));
        let e = Event::RunError { message: "x".into(), code: None };
        assert_eq!(serde_json::to_value(&e).unwrap(), json!({ "type": "RUN_ERROR", "message": "x" }));
    }
}
