//! Chat nodes (cowork-agui-implement.md): a CLI runs headless and speaks a structured protocol; a harness turns its lines
//! into AG-UI events for the UI and turns "prompt / interrupt / answer an approval" into lines for its stdin.
//! Harnesses are pure: no process, no clock, no IO, so the P0 recordings in scripts/fixtures/agui replay them in tests.
use serde::Serialize;
use serde_json::Value;

pub mod acp;
pub mod claude;
pub mod codex;
pub mod http;
pub mod session;
pub mod transcript;

/// The AG-UI events this app emits (https://docs.ag-ui.com). Serialized as the protocol's JSON: `type` in
/// SCREAMING_SNAKE_CASE, fields in camelCase.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Event {
    #[serde(rename_all = "camelCase")]
    RunStarted { thread_id: String, run_id: String },
    #[serde(rename_all = "camelCase")]
    RunFinished { thread_id: String, run_id: String },
    #[serde(rename_all = "camelCase")]
    RunError { message: String, #[serde(skip_serializing_if = "Option::is_none")] code: Option<String> },
    #[serde(rename_all = "camelCase")]
    TextMessageStart { message_id: String, role: String },
    #[serde(rename_all = "camelCase")]
    TextMessageContent { message_id: String, delta: String },
    #[serde(rename_all = "camelCase")]
    TextMessageEnd { message_id: String },
    #[serde(rename_all = "camelCase")]
    ToolCallStart { tool_call_id: String, tool_call_name: String },
    #[serde(rename_all = "camelCase")]
    ToolCallArgs { tool_call_id: String, delta: String },
    #[serde(rename_all = "camelCase")]
    ToolCallEnd { tool_call_id: String },
    #[serde(rename_all = "camelCase")]
    ToolCallResult { message_id: String, tool_call_id: String, content: String, role: String },
    Custom { name: String, value: Value },
}

/// The code a stopped run carries, so the UI can tell "you stopped it" from a failure
pub const INTERRUPTED: &str = "interrupted";

/// A tool call waiting for the user. `id` is the harness's own key for `answer`.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub id: String,
    pub tool: String,
    /// One line the card shows: the command, the file, or the CLI's own title
    pub summary: String,
    pub input: Value,
    /// The CLI can remember "allow for this session"
    pub session_scope: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    /// For the UI
    Ui(Event),
    /// One line for the CLI's stdin, without the newline
    Send(String),
    /// Ask the user; the answer goes back through `Harness::answer`
    Approval(Approval),
    /// The CLI's session id, for resume
    Session(String),
}

pub trait Harness: Send {
    /// Lines to send right after the process starts
    fn start(&mut self) -> Vec<Out>;
    fn on_line(&mut self, line: &str) -> Vec<Out>;
    fn prompt(&mut self, text: &str) -> Vec<Out>;
    fn interrupt(&mut self) -> Vec<Out>;
    fn answer(&mut self, id: &str, allow: bool, for_session: bool) -> Vec<Out>;
}

pub(crate) fn send(v: Value) -> Out {
    Out::Send(v.to_string())
}

pub(crate) fn usage(value: Value) -> Out {
    Out::Ui(Event::Custom { name: "wb.usage".into(), value })
}

pub(crate) fn tool_result(tool_call_id: &str, content: String) -> Out {
    Out::Ui(Event::ToolCallResult { message_id: format!("{tool_call_id}:result"), tool_call_id: tool_call_id.into(), content, role: "tool".into() })
}

/// Tool output kept for the UI; the CLI's own record has the rest
pub(crate) const MAX_RESULT: usize = 4096;

pub(crate) fn clip(s: &str) -> String {
    if s.len() <= MAX_RESULT {
        return s.to_string();
    }
    let mut end = MAX_RESULT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

pub(crate) fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(|x| x.get("text").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| text_of(x))).collect::<Vec<_>>().join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub(crate) fn rpc_id_key(id: &Value) -> String {
    match id {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub(crate) fn json_line(line: &str) -> Option<Value> {
    serde_json::from_str::<Value>(line.trim()).ok().filter(Value::is_object)
}

#[cfg(test)]
pub(crate) mod replay {
    //! Feeds a P0 recording through a harness: the CLI's stdout lines go to `on_line`, the driver's prompts and
    //! interrupts go through the harness the way the session manager would send them.
    use super::*;

    pub struct Seen {
        pub events: Vec<Event>,
        pub approvals: Vec<Approval>,
        pub sent: Vec<String>,
        pub sessions: Vec<String>,
    }

    impl Seen {
        pub fn text(&self) -> String {
            self.events.iter().filter_map(|e| if let Event::TextMessageContent { delta, .. } = e { Some(delta.as_str()) } else { None }).collect()
        }
        pub fn count(&self, f: impl Fn(&Event) -> bool) -> usize {
            self.events.iter().filter(|e| f(e)).count()
        }
        /// Every message and tool call that started also ended, and runs never overlap
        pub fn assert_balanced(&self) {
            let mut open_msgs = std::collections::HashSet::new();
            let mut open_tools = std::collections::HashSet::new();
            let mut in_run = false;
            for e in &self.events {
                match e {
                    Event::RunStarted { .. } => {
                        assert!(!in_run, "run started inside a run");
                        in_run = true;
                    }
                    Event::RunFinished { .. } | Event::RunError { .. } => {
                        assert!(open_msgs.is_empty(), "run ended with open messages {open_msgs:?}");
                        assert!(open_tools.is_empty(), "run ended with open tool calls {open_tools:?}");
                        in_run = false;
                    }
                    Event::TextMessageStart { message_id, .. } => assert!(open_msgs.insert(message_id.clone()), "message started twice"),
                    Event::TextMessageContent { message_id, .. } => assert!(open_msgs.contains(message_id), "content for a closed message {message_id}"),
                    Event::TextMessageEnd { message_id } => assert!(open_msgs.remove(message_id), "end for a closed message {message_id}"),
                    Event::ToolCallStart { tool_call_id, .. } => assert!(open_tools.insert(tool_call_id.clone()), "tool started twice {tool_call_id}"),
                    Event::ToolCallArgs { tool_call_id, .. } => assert!(open_tools.contains(tool_call_id), "args for a closed tool call"),
                    Event::ToolCallEnd { tool_call_id } => assert!(open_tools.remove(tool_call_id), "end for a closed tool call {tool_call_id}"),
                    _ => {}
                }
            }
            assert!(!in_run, "a run never ended");
        }
    }

    /// `on_stdin` gets each line the driver wrote; it returns what the harness produced for it (prompt, interrupt…)
    pub fn run(h: &mut dyn Harness, fixture: &str, mut on_stdin: impl FnMut(&mut dyn Harness, &Value) -> Vec<Out>) -> Seen {
        let mut seen = Seen { events: vec![], approvals: vec![], sent: vec![], sessions: vec![] };
        let take = |outs: Vec<Out>, seen: &mut Seen| {
            for o in outs {
                match o {
                    Out::Ui(e) => seen.events.push(e),
                    Out::Send(s) => seen.sent.push(s),
                    Out::Approval(a) => seen.approvals.push(a),
                    Out::Session(s) => seen.sessions.push(s),
                }
            }
        };
        let outs = h.start();
        take(outs, &mut seen);
        for row in fixture.lines().filter(|l| !l.trim().is_empty()) {
            let r: Value = serde_json::from_str(row).unwrap();
            let line = r["line"].as_str().unwrap();
            match r["src"].as_str() {
                Some("stdout") => {
                    let outs = h.on_line(line);
                    take(outs, &mut seen);
                }
                Some("stdin") => {
                    let v: Value = serde_json::from_str(line).unwrap();
                    let outs = on_stdin(h, &v);
                    take(outs, &mut seen);
                }
                _ => {}
            }
        }
        seen
    }
}
