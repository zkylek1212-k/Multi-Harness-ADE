//! An external AG-UI agent over HTTP (cowork-agui-implement.md §4.5): every turn POSTs a `RunAgentInput` with the
//! whole thread and reads the agent's Server-Sent Events, which already are AG-UI. The harness is pure like the CLI
//! ones; `HttpLines` stands in for the CLI process, so a chat node runs the same way either way.
//! The agent's own tools run on its side: nothing here can approve or refuse them.
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::*;

pub const PROTOCOL: &str = "agui-http";

/// Only https, or http when the user explicitly allowed it for this endpoint
pub fn url_ok(url: &str, allow_http: bool) -> Result<(), String> {
    let lower = url.trim().to_ascii_lowercase();
    let rest = lower.strip_prefix("https://").or_else(|| lower.strip_prefix("http://").filter(|_| allow_http));
    match rest {
        Some(r) if !r.is_empty() && !r.starts_with('/') && !url.chars().any(|c| c.is_whitespace() || c.is_control()) => Ok(()),
        _ if lower.starts_with("http://") => Err("Plain http needs \"Allow http\" for this endpoint".into()),
        _ => Err("The AG-UI endpoint must be an https:// URL".into()),
    }
}

/// `data:` lines up to a blank line make one event; comments and other fields are skipped
#[derive(Default)]
pub struct Sse {
    data: Vec<String>,
}

impl Sse {
    pub fn line(&mut self, line: &str) -> Option<String> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            return if self.data.is_empty() { None } else { Some(std::mem::take(&mut self.data).join("\n")) };
        }
        if let Some(d) = line.strip_prefix("data:") {
            self.data.push(d.strip_prefix(' ').unwrap_or(d).to_string());
        }
        None
    }
}

const TERMINAL: [&str; 2] = ["RUN_FINISHED", "RUN_ERROR"];

fn run_error(message: &str) -> String {
    json!({ "type": "RUN_ERROR", "message": message }).to_string()
}

type Exit = Box<dyn FnOnce(Option<i32>, String) + Send>;

/// The HTTP side of a chat node: `write_line` gets what the harness sends (a run, or `{"cancel":true}`), the agent's
/// events come back through `on_line`, one JSON object per call
pub struct HttpLines {
    url: String,
    key: String,
    run: Mutex<Option<Arc<AtomicBool>>>,
    on_line: Arc<dyn Fn(String) + Send + Sync>,
    exit: Mutex<Option<Exit>>,
}

impl HttpLines {
    pub fn new(url: &str, key: &str, on_line: Arc<dyn Fn(String) + Send + Sync>, on_exit: Exit) -> Arc<Self> {
        Arc::new(HttpLines { url: url.into(), key: key.into(), run: Mutex::default(), on_line, exit: Mutex::new(Some(on_exit)) })
    }

    fn cancel(&self) {
        if let Some(f) = self.run.lock().unwrap().take() {
            f.store(true, Ordering::SeqCst);
        }
    }

    pub fn write_line(&self, line: &str) {
        self.cancel();
        if serde_json::from_str::<Value>(line).is_ok_and(|v| v["cancel"] == true) || self.exit.lock().unwrap().is_none() {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        *self.run.lock().unwrap() = Some(stop.clone());
        let (url, key, body, on_line) = (self.url.clone(), self.key.clone(), line.to_string(), self.on_line.clone());
        std::thread::spawn(move || {
            let auth = format!("Bearer {key}");
            let mut headers = vec![("Accept", "text/event-stream")];
            if !key.is_empty() {
                headers.push(("Authorization", auth.as_str()));
            }
            let mut sse = Sse::default();
            let mut finished = false;
            let r = crate::http::post_lines(&url, &headers, &body, |l| {
                if stop.load(Ordering::SeqCst) {
                    return false;
                }
                if let Some(ev) = sse.line(l) {
                    // One line per event: the harness reads JSON lines
                    let Ok(v) = serde_json::from_str::<Value>(&ev) else { return true };
                    finished |= TERMINAL.contains(&v["type"].as_str().unwrap_or(""));
                    on_line(v.to_string());
                }
                true
            });
            if stop.load(Ordering::SeqCst) || finished {
                return;
            }
            on_line(run_error(&match r {
                Ok((s, body)) if !(200..300).contains(&s) => format!("The agent answered HTTP {s}: {}", body.trim()),
                Ok(_) => "The agent closed the connection before the run finished".into(),
                Err(e) => format!("Could not reach the agent: {e}"),
            }));
        });
    }

    /// Ends the node: the open run is dropped, and the node is told it ended (off this thread: callers hold locks)
    pub fn kill(&self) {
        self.cancel();
        if let Some(f) = self.exit.lock().unwrap().take() {
            std::thread::spawn(move || f(Some(0), String::new()));
        }
    }
}

/// The thread lives here: AG-UI agents get all of it with every run
pub struct AguiHttp {
    thread: String,
    messages: Vec<Value>,
    run: u64,
    in_run: bool,
    /// After a stop, until that run's last event
    ignoring: bool,
    open: Vec<String>,
    texts: HashMap<String, String>,
}

impl AguiHttp {
    /// `history`: (role, text) of the conversation so far, so an app restart keeps the thread
    pub fn new(thread: &str, history: Vec<(String, String)>) -> Self {
        let messages = history.into_iter().enumerate().map(|(i, (role, content))| json!({ "id": format!("h{i}"), "role": role, "content": content })).collect();
        AguiHttp { thread: thread.into(), messages, run: 0, in_run: false, ignoring: false, open: vec![], texts: HashMap::new() }
    }

    fn close_texts(&mut self) -> Vec<Out> {
        let ids = std::mem::take(&mut self.open);
        ids.into_iter().map(|id| Out::Ui(Event::TextMessageEnd { message_id: id })).collect()
    }

    fn keep_answers(&mut self) {
        let texts = std::mem::take(&mut self.texts);
        let mut ids: Vec<_> = texts.into_iter().filter(|(_, t)| !t.trim().is_empty()).collect();
        ids.sort();
        for (id, t) in ids {
            self.messages.push(json!({ "id": id, "role": "assistant", "content": t }));
        }
    }

    fn start_text(&mut self, id: &str) -> Vec<Out> {
        if self.open.iter().any(|o| o == id) {
            return vec![];
        }
        self.open.push(id.into());
        self.texts.entry(id.into()).or_default();
        vec![Out::Ui(Event::TextMessageStart { message_id: id.into(), role: "assistant".into() })]
    }
}

impl Harness for AguiHttp {
    fn start(&mut self) -> Vec<Out> {
        vec![]
    }

    fn on_line(&mut self, line: &str) -> Vec<Out> {
        let Some(v) = json_line(line) else { return vec![] };
        let ty = v["type"].as_str().unwrap_or("");
        if self.ignoring {
            self.ignoring = !TERMINAL.contains(&ty);
            return vec![];
        }
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        match ty {
            "RUN_STARTED" => {
                self.in_run = true;
                let thread_id = Some(s("threadId")).filter(|t| !t.is_empty()).unwrap_or_else(|| self.thread.clone());
                vec![Out::Ui(Event::RunStarted { thread_id, run_id: s("runId") })]
            }
            "TEXT_MESSAGE_START" => self.start_text(&s("messageId")),
            "TEXT_MESSAGE_CONTENT" | "TEXT_MESSAGE_CHUNK" => {
                let id = Some(s("messageId")).filter(|i| !i.is_empty()).or_else(|| self.open.last().cloned()).unwrap_or_else(|| format!("m{}", self.run));
                let mut out = self.start_text(&id);
                let delta = s("delta");
                if !delta.is_empty() {
                    self.texts.entry(id.clone()).or_default().push_str(&delta);
                    out.push(Out::Ui(Event::TextMessageContent { message_id: id, delta }));
                }
                out
            }
            "TEXT_MESSAGE_END" => {
                let id = s("messageId");
                self.open.retain(|o| *o != id);
                vec![Out::Ui(Event::TextMessageEnd { message_id: id })]
            }
            "TOOL_CALL_START" => vec![Out::Ui(Event::ToolCallStart { tool_call_id: s("toolCallId"), tool_call_name: s("toolCallName") })],
            "TOOL_CALL_ARGS" => vec![Out::Ui(Event::ToolCallArgs { tool_call_id: s("toolCallId"), delta: s("delta") })],
            "TOOL_CALL_END" => vec![Out::Ui(Event::ToolCallEnd { tool_call_id: s("toolCallId") })],
            "TOOL_CALL_RESULT" => vec![tool_result(&s("toolCallId"), clip(&text_of(&v["content"])))],
            "RUN_FINISHED" => {
                let mut out = self.close_texts();
                self.keep_answers();
                self.in_run = false;
                let thread_id = Some(s("threadId")).filter(|t| !t.is_empty()).unwrap_or_else(|| self.thread.clone());
                out.push(Out::Ui(Event::RunFinished { thread_id, run_id: s("runId") }));
                out
            }
            "RUN_ERROR" => {
                let mut out = self.close_texts();
                self.keep_answers();
                self.in_run = false;
                out.push(Out::Ui(Event::RunError { message: s("message"), code: Some(s("code")).filter(|c| !c.is_empty()) }));
                out
            }
            _ => vec![],
        }
    }

    fn prompt(&mut self, text: &str) -> Vec<Out> {
        self.run += 1;
        self.messages.push(json!({ "id": format!("u{}", self.run), "role": "user", "content": text }));
        self.in_run = true;
        self.ignoring = false;
        let input = json!({ "threadId": self.thread, "runId": format!("{}-{}", self.thread, self.run), "state": {}, "messages": self.messages, "tools": [], "context": [], "forwardedProps": {} });
        vec![Out::Send(input.to_string())]
    }

    fn interrupt(&mut self) -> Vec<Out> {
        if !self.in_run {
            return vec![];
        }
        self.in_run = false;
        self.ignoring = true;
        let mut out = self.close_texts();
        self.keep_answers();
        out.push(Out::Send(json!({ "cancel": true }).to_string()));
        out.push(Out::Ui(Event::RunError { message: "Stopped".into(), code: Some(INTERRUPTED.into()) }));
        out
    }

    fn answer(&mut self, _: &str, _: bool, _: bool) -> Vec<Out> {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_unless_allowed() {
        assert!(url_ok("https://agent.example/agui", false).is_ok());
        assert!(url_ok("http://10.0.0.5:8000/agui", false).unwrap_err().contains("Allow http"));
        assert!(url_ok("http://10.0.0.5:8000/agui", true).is_ok());
        assert!(url_ok("file:///c:/x", true).is_err() && url_ok("https://", false).is_err() && url_ok("https://a b", false).is_err());
    }

    #[test]
    fn sse_lines_make_events() {
        let mut s = Sse::default();
        let mut got = vec![];
        for l in [": ping", "event: message", "data: {\"a\":", "data: 1}", "", "", "data:{\"b\":2}\r", "\r"] {
            got.extend(s.line(l));
        }
        assert_eq!(got, ["{\"a\":\n1}", "{\"b\":2}"]);
    }

    /// The event stream of an OpenBot-style agent: chunks without a start, a tool call, the run's end
    #[test]
    fn a_run_streams_and_the_thread_grows() {
        let mut h = AguiHttp::new("c/n", vec![("user".into(), "earlier".into()), ("assistant".into(), "earlier answer".into())]);
        let Out::Send(body) = &h.prompt("hi")[0] else { panic!() };
        let input: Value = serde_json::from_str(body).unwrap();
        assert_eq!(input["messages"].as_array().unwrap().iter().map(|m| m["content"].as_str().unwrap()).collect::<Vec<_>>(), ["earlier", "earlier answer", "hi"]);
        assert!(input["runId"].is_string() && input["tools"].is_array());
        let lines = [
            r#"{"type":"RUN_STARTED","threadId":"c/n","runId":"r1"}"#,
            r#"{"type":"TEXT_MESSAGE_CHUNK","messageId":"a1","delta":"Hel"}"#,
            r#"{"type":"TEXT_MESSAGE_CHUNK","messageId":"a1","delta":"lo"}"#,
            r#"{"type":"TOOL_CALL_START","toolCallId":"t1","toolCallName":"search"}"#,
            r#"{"type":"TOOL_CALL_ARGS","toolCallId":"t1","delta":"{\"q\":1}"}"#,
            r#"{"type":"TOOL_CALL_END","toolCallId":"t1"}"#,
            r#"{"type":"TOOL_CALL_RESULT","messageId":"x","toolCallId":"t1","content":"found"}"#,
            r#"{"type":"STATE_SNAPSHOT","snapshot":{}}"#,
            r#"{"type":"RUN_FINISHED","threadId":"c/n","runId":"r1"}"#,
        ];
        let events: Vec<Event> = lines.iter().flat_map(|l| h.on_line(l)).filter_map(|o| if let Out::Ui(e) = o { Some(e) } else { None }).collect();
        replay::Seen { events: events.clone(), approvals: vec![], sent: vec![], sessions: vec![] }.assert_balanced();
        assert!(events.contains(&Event::TextMessageContent { message_id: "a1".into(), delta: "lo".into() }));
        assert!(matches!(events.last(), Some(Event::RunFinished { .. })));
        let Out::Send(body) = &h.prompt("next")[0] else { panic!() };
        let input: Value = serde_json::from_str(body).unwrap();
        assert_eq!(input["messages"][3], json!({ "id": "a1", "role": "assistant", "content": "Hello" }), "the answer joins the thread");
    }

    #[test]
    fn a_stop_ends_the_run_at_once_and_drops_its_tail() {
        let mut h = AguiHttp::new("t", vec![]);
        h.prompt("go");
        h.on_line(r#"{"type":"RUN_STARTED","runId":"r"}"#);
        h.on_line(r#"{"type":"TEXT_MESSAGE_START","messageId":"a","role":"assistant"}"#);
        let out = h.interrupt();
        assert!(out.contains(&Out::Send(r#"{"cancel":true}"#.into())));
        assert!(out.iter().any(|o| matches!(o, Out::Ui(Event::TextMessageEnd { .. }))));
        assert!(matches!(out.last(), Some(Out::Ui(Event::RunError { code: Some(c), .. })) if c == INTERRUPTED));
        assert!(h.on_line(r#"{"type":"TEXT_MESSAGE_CONTENT","messageId":"a","delta":"late"}"#).is_empty());
        assert!(h.on_line(r#"{"type":"RUN_FINISHED","runId":"r"}"#).is_empty(), "the stopped run's end is not a second end");
        h.prompt("again");
        assert!(!h.on_line(r#"{"type":"RUN_STARTED","runId":"r2"}"#).is_empty(), "the next run is shown");
    }
}
