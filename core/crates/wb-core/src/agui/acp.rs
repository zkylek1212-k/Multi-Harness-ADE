//! Agent Client Protocol (JSON-RPC 2.0 over stdio), as `agy acp` speaks it (cowork-agui-implement.md §4.3, P0-verified
//! on agy 1.5.8). Any CLI with an ACP server can use this harness.
use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use super::*;

const INIT: u64 = 1;
const SESSION: u64 = 2;

pub struct Acp {
    thread: String,
    cwd: String,
    resume: Option<String>,
    next_id: u64,
    session: Option<String>,
    /// session/load replays the history as updates; the UI already has it
    loading: bool,
    queued: Vec<String>,
    /// prompt request id -> run id
    prompts: HashMap<u64, String>,
    runs: u32,
    run: Option<String>,
    message: Option<String>,
    messages: u32,
    tools: HashSet<String>,
    /// approval key -> (request id, allow option, reject option)
    asks: HashMap<String, (Value, Option<String>, Option<String>)>,
}

impl Acp {
    pub fn new(thread: &str, cwd: &str, resume: Option<&str>) -> Self {
        Acp { thread: thread.into(), cwd: cwd.into(), resume: resume.map(str::to_string), next_id: 3, session: None, loading: resume.is_some(), queued: vec![], prompts: HashMap::new(), runs: 0, run: None, message: None, messages: 0, tools: HashSet::new(), asks: HashMap::new() }
    }

    fn rpc(id: u64, method: &str, params: Value) -> Out {
        send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
    }

    fn send_prompt(&mut self, text: &str) -> Vec<Out> {
        let id = self.next_id;
        self.next_id += 1;
        self.runs += 1;
        let run = format!("run-{}", self.runs);
        self.prompts.insert(id, run.clone());
        self.run = Some(run.clone());
        let sid = self.session.clone().unwrap_or_default();
        vec![Out::Ui(Event::RunStarted { thread_id: self.thread.clone(), run_id: run }), Self::rpc(id, "session/prompt", json!({ "sessionId": sid, "prompt": [{ "type": "text", "text": text }] }))]
    }

    fn end_message(&mut self) -> Vec<Out> {
        self.message.take().map(|id| Out::Ui(Event::TextMessageEnd { message_id: id })).into_iter().collect()
    }

    fn update(&mut self, u: &Value) -> Vec<Out> {
        match u["sessionUpdate"].as_str().unwrap_or("") {
            "agent_message_chunk" => {
                let text = u["content"]["text"].as_str().unwrap_or("");
                let mut out = vec![];
                if self.message.is_none() {
                    self.messages += 1;
                    let id = format!("{}:m{}", self.run.as_deref().unwrap_or("run"), self.messages);
                    self.message = Some(id.clone());
                    out.push(Out::Ui(Event::TextMessageStart { message_id: id, role: "assistant".into() }));
                }
                out.push(Out::Ui(Event::TextMessageContent { message_id: self.message.clone().unwrap(), delta: text.into() }));
                out
            }
            "tool_call" | "tool_call_update" => {
                let id = u["toolCallId"].as_str().unwrap_or_default().to_string();
                let mut out = self.end_message();
                if self.tools.insert(id.clone()) {
                    let name = u["name"].as_str().or(u["kind"].as_str()).unwrap_or("tool").to_string();
                    out.push(Out::Ui(Event::ToolCallStart { tool_call_id: id.clone(), tool_call_name: name }));
                    let args = u.get("rawInput").or(u.get("args")).cloned().unwrap_or(Value::Null);
                    out.push(Out::Ui(Event::ToolCallArgs { tool_call_id: id.clone(), delta: args.to_string() }));
                }
                if matches!(u["status"].as_str(), Some("completed") | Some("failed")) && self.tools.remove(&id) {
                    out.push(Out::Ui(Event::ToolCallEnd { tool_call_id: id.clone() }));
                    let body = text_of(&u["content"]);
                    let content = if body.is_empty() { format!("{} {}", u["status"].as_str().unwrap_or(""), u["title"].as_str().unwrap_or("")).trim().to_string() } else { body };
                    out.push(tool_result(&id, clip(&content)));
                }
                out
            }
            "usage_update" => vec![usage(json!({ "used": u["used"], "size": u["size"] }))],
            _ => vec![],
        }
    }

    fn close_run(&mut self) -> Vec<Out> {
        let mut out = self.end_message();
        out.extend(self.tools.drain().map(|id| Out::Ui(Event::ToolCallEnd { tool_call_id: id })));
        out
    }
}

impl Harness for Acp {
    fn start(&mut self) -> Vec<Out> {
        let session = match &self.resume {
            Some(s) => Self::rpc(SESSION, "session/load", json!({ "sessionId": s, "cwd": self.cwd, "mcpServers": [] })),
            None => Self::rpc(SESSION, "session/new", json!({ "cwd": self.cwd, "mcpServers": [] })),
        };
        vec![Self::rpc(INIT, "initialize", json!({ "protocolVersion": 1, "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false } })), session]
    }

    fn on_line(&mut self, line: &str) -> Vec<Out> {
        let Some(m) = json_line(line) else { return vec![] };
        let method = m["method"].as_str().unwrap_or("");
        if method == "session/request_permission" {
            let key = format!("acp-{}", rpc_id_key(&m["id"]));
            let tc = &m["params"]["toolCall"];
            let pick = |kind: &str| m["params"]["options"].as_array().and_then(|a| a.iter().find(|o| o["kind"] == kind)).and_then(|o| o["optionId"].as_str()).map(str::to_string);
            self.asks.insert(key.clone(), (m["id"].clone(), pick("allow_once"), pick("reject_once")));
            let tool = tc["name"].as_str().or(tc["kind"].as_str()).unwrap_or("tool").to_string();
            let summary = tc["title"].as_str().map(str::to_string).unwrap_or_else(|| tool.clone());
            return vec![Out::Approval(Approval { id: key, tool, summary, input: tc.get("rawInput").cloned().unwrap_or(Value::Null), session_scope: false })];
        }
        if !method.is_empty() && m.get("id").is_some() {
            return vec![send(json!({ "jsonrpc": "2.0", "id": m["id"], "error": { "code": -32601, "message": format!("{method} is not supported by Agent Workbench") } }))];
        }
        if method == "session/update" {
            return if self.loading { vec![] } else { self.update(&m["params"]["update"]) };
        }
        let Some(id) = m["id"].as_u64() else { return vec![] };
        if id == SESSION {
            self.loading = false;
            let sid = m["result"]["sessionId"].as_str().map(str::to_string).or_else(|| self.resume.clone().filter(|_| m.get("error").is_none()));
            let Some(sid) = sid else {
                let message = m["error"]["message"].as_str().unwrap_or("the agent could not open a session").to_string();
                return vec![Out::Ui(Event::RunError { message, code: None })];
            };
            self.session = Some(sid.clone());
            let mut out = vec![Out::Session(sid)];
            for text in std::mem::take(&mut self.queued) {
                out.extend(self.send_prompt(&text));
            }
            return out;
        }
        let Some(run) = self.prompts.remove(&id) else { return vec![] };
        let mut out = self.close_run();
        self.run = None;
        let r = &m["result"];
        if let Some(u) = r.get("usage") {
            out.push(usage(u.clone()));
        }
        out.push(Out::Ui(match (r["stopReason"].as_str(), m.get("error")) {
            (_, Some(e)) => Event::RunError { message: e["message"].as_str().unwrap_or("the prompt failed").into(), code: None },
            (Some("cancelled"), _) => Event::RunError { message: "stopped".into(), code: Some(INTERRUPTED.into()) },
            (Some("end_turn"), _) | (None, _) => Event::RunFinished { thread_id: self.thread.clone(), run_id: run },
            (Some(other), _) => Event::RunError { message: format!("stopped: {other}"), code: None },
        }));
        out
    }

    fn prompt(&mut self, text: &str) -> Vec<Out> {
        if self.session.is_none() {
            self.queued.push(text.to_string());
            return vec![];
        }
        self.send_prompt(text)
    }

    fn interrupt(&mut self) -> Vec<Out> {
        match (&self.session, &self.run) {
            (Some(s), Some(_)) => vec![send(json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": s } }))],
            _ => vec![],
        }
    }

    fn answer(&mut self, id: &str, allow: bool, _for_session: bool) -> Vec<Out> {
        let Some((rid, yes, no)) = self.asks.remove(id) else { return vec![] };
        let outcome = match if allow { yes } else { no } {
            Some(option) => json!({ "outcome": "selected", "optionId": option }),
            None => json!({ "outcome": "cancelled" }),
        };
        vec![send(json!({ "jsonrpc": "2.0", "id": rid, "result": { "outcome": outcome } }))]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agui::replay;

    const RUN: &str = include_str!("../../../../../scripts/fixtures/agui/agy-acp.ndjson");
    const LOAD: &str = include_str!("../../../../../scripts/fixtures/agui/agy-acp-load.ndjson");

    fn drive(h: &mut dyn Harness, v: &Value) -> Vec<Out> {
        match v["method"].as_str() {
            Some("session/prompt") => h.prompt(v["params"]["prompt"][0]["text"].as_str().unwrap()),
            Some("session/cancel") => h.interrupt(),
            None if v.get("result").is_some() => h.answer(&format!("acp-{}", rpc_id_key(&v["id"])), true, false),
            _ => vec![],
        }
    }

    #[test]
    fn replays_the_p0_session() {
        let mut h = Acp::new("t1", "C:/repo", None);
        let seen = replay::run(&mut h, RUN, drive);
        seen.assert_balanced();
        assert_eq!(seen.sessions, ["0bffbeb2-a4ca-4bd3-a8bd-0cfd854e98b2"]);
        assert_eq!(seen.count(|e| matches!(e, Event::RunStarted { .. })), 4);
        assert_eq!(seen.count(|e| matches!(e, Event::RunFinished { .. })), 3);
        assert_eq!(seen.count(|e| matches!(e, Event::RunError { code: Some(c), .. } if c == INTERRUPTED)), 1);
        let tools: Vec<&str> = seen.approvals.iter().map(|a| a.tool.as_str()).collect();
        assert_eq!(tools, ["edit_file", "run_command"]);
        assert_eq!(seen.approvals[1].summary, "node --test");
        assert!(seen.text().contains("KIWI-9") && seen.text().contains("MANGO-3"));
        assert_eq!(seen.count(|e| matches!(e, Event::ToolCallResult { .. })), 4);
        assert_eq!(seen.sent.iter().filter(|s| s.contains("\"optionId\":\"allow\"")).count(), 2);
    }

    #[test]
    fn load_skips_the_replayed_history() {
        let mut h = Acp::new("t1", "C:/repo", Some("0bffbeb2-a4ca-4bd3-a8bd-0cfd854e98b2"));
        let seen = replay::run(&mut h, LOAD, drive);
        seen.assert_balanced();
        assert!(seen.sent[1].contains("session/load"));
        assert_eq!(seen.count(|e| matches!(e, Event::ToolCallStart { .. })), 0, "history is not re-sent to the UI");
        assert_eq!(seen.text().trim(), "sum.js");
    }

    #[test]
    fn deny_picks_the_reject_option() {
        let mut h = Acp::new("t", "C:/r", None);
        let ask = r#"{"jsonrpc":"2.0","id":7,"method":"session/request_permission","params":{"toolCall":{"name":"run_command","title":"rm x"},"options":[{"optionId":"allow","kind":"allow_once"},{"optionId":"deny","kind":"reject_once"}]}}"#;
        let [Out::Approval(a)] = &h.on_line(ask)[..] else { panic!() };
        let [Out::Send(s)] = &h.answer(&a.id, false, false)[..] else { panic!() };
        assert!(s.contains("\"optionId\":\"deny\"") && s.contains("\"id\":7"));
    }
}
