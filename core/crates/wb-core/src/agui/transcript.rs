//! What a conversation keeps (cowork-agui-implement.md §3.4): whole messages and tool-call summaries, never the
//! stream's fragments. The UI draws the conversation from these entries plus the live events of the run in progress.
use serde::{Deserialize, Serialize};

use super::{Approval, Event, INTERRUPTED};

/// Per conversation; the oldest go first
pub const MAX_ENTRIES: usize = 2000;
const MAX_TEXT: usize = 64 * 1024;
const MAX_ARGS: usize = 8 * 1024;
/// What a bot answers in free talk when a message is not for its role (cowork-agui-implement.md §6.8); never kept
pub const PASS: &str = "PASS";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Entry {
    /// From the user, or a message another agent sent (`from` is then that node's id)
    #[serde(rename_all = "camelCase")]
    User { id: String, text: String, from: String, at: f64 },
    #[serde(rename_all = "camelCase")]
    Text { id: String, text: String, at: f64 },
    #[serde(rename_all = "camelCase")]
    Tool { id: String, name: String, args: String, output: String, status: String, at: f64 },
    #[serde(rename_all = "camelCase")]
    Approval {
        id: String,
        tool: String,
        summary: String,
        status: String,
        session_scope: bool,
        at: f64,
        /// Who answered when it was not the user: a colleague's name (§13.4)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<String>,
    },
    /// A line from the app itself (took control, handed back)
    #[serde(rename_all = "camelCase")]
    Note { id: String, text: String, at: f64 },
    /// Only runs that did not finish normally leave a line
    #[serde(rename_all = "camelCase")]
    RunError { id: String, message: String, interrupted: bool, at: f64 },
}

fn cap(s: &mut String, max: usize) {
    if s.len() > max {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
        s.push('…');
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Transcript {
    pub entries: Vec<Entry>,
}

impl Transcript {
    fn find(&mut self, kind_id: impl Fn(&Entry) -> bool) -> Option<&mut Entry> {
        self.entries.iter_mut().rev().find(|e| kind_id(e))
    }

    fn push(&mut self, e: Entry) {
        self.entries.push(e);
        if self.entries.len() > MAX_ENTRIES {
            let cut = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(..cut);
        }
    }

    pub fn user(&mut self, id: &str, text: &str, from: &str, at: f64) {
        let mut text = text.to_string();
        cap(&mut text, MAX_TEXT);
        self.push(Entry::User { id: id.into(), text, from: from.into(), at });
    }

    pub fn note(&mut self, id: &str, text: &str, at: f64) {
        self.push(Entry::Note { id: id.into(), text: text.into(), at });
    }

    pub fn approval(&mut self, a: &Approval, at: f64) {
        self.push(Entry::Approval { id: a.id.clone(), tool: a.tool.clone(), summary: a.summary.clone(), status: "pending".into(), session_scope: a.session_scope, at, by: None });
    }

    /// pending -> allowed | denied | expired; false when there is no such pending approval
    pub fn settle(&mut self, id: &str, status: &str) -> bool {
        self.settle_by(id, status, None)
    }

    pub fn settle_by(&mut self, id: &str, status: &str, who: Option<&str>) -> bool {
        match self.find(|e| matches!(e, Entry::Approval { id: i, .. } if i == id)) {
            Some(Entry::Approval { status: s, by, .. }) if s == "pending" => {
                *s = status.into();
                *by = who.map(str::to_string);
                true
            }
            _ => false,
        }
    }

    /// Approvals still open when the process went away can never be answered
    pub fn expire_pending(&mut self) {
        for e in &mut self.entries {
            if let Entry::Approval { status, .. } = e {
                if status == "pending" {
                    *status = "expired".into();
                }
            }
        }
    }

    /// Applies a live event; returns the entry it created or changed, for the UI to upsert by id
    pub fn apply(&mut self, ev: &Event, at: f64) -> Option<Entry> {
        let touched = match ev {
            Event::TextMessageStart { message_id, .. } | Event::TextMessageContent { message_id, .. } => Some(message_id.clone()),
            Event::ToolCallStart { tool_call_id, .. } | Event::ToolCallArgs { tool_call_id, .. } | Event::ToolCallResult { tool_call_id, .. } => Some(tool_call_id.clone()),
            _ => None,
        };
        self.apply_inner(ev, at);
        if let Event::RunError { .. } = ev {
            return self.entries.last().cloned();
        }
        let id = touched?;
        self.entries.iter().rev().find(|e| match e {
            Entry::Text { id: i, .. } | Entry::Tool { id: i, .. } => *i == id,
            _ => false,
        }).cloned()
    }

    fn apply_inner(&mut self, ev: &Event, at: f64) {
        match ev {
            Event::TextMessageStart { message_id, .. } => self.push(Entry::Text { id: message_id.clone(), text: String::new(), at }),
            Event::TextMessageContent { message_id, delta } => {
                if let Some(Entry::Text { text, .. }) = self.find(|e| matches!(e, Entry::Text { id, .. } if id == message_id)) {
                    text.push_str(delta);
                    cap(text, MAX_TEXT);
                }
            }
            Event::ToolCallStart { tool_call_id, tool_call_name } => {
                self.push(Entry::Tool { id: tool_call_id.clone(), name: tool_call_name.clone(), args: String::new(), output: String::new(), status: "running".into(), at })
            }
            Event::ToolCallArgs { tool_call_id, delta } => {
                if let Some(Entry::Tool { args, .. }) = self.find(|e| matches!(e, Entry::Tool { id, .. } if id == tool_call_id)) {
                    args.push_str(delta);
                    cap(args, MAX_ARGS);
                }
            }
            Event::ToolCallResult { tool_call_id, content, .. } => {
                if let Some(Entry::Tool { output, status, .. }) = self.find(|e| matches!(e, Entry::Tool { id, .. } if id == tool_call_id)) {
                    *output = content.clone();
                    *status = "done".into();
                }
            }
            Event::RunError { message, code } => {
                // Whatever was still running stopped with the run
                for e in &mut self.entries {
                    if let Entry::Tool { status, .. } = e {
                        if status == "running" {
                            *status = "stopped".into();
                        }
                    }
                }
                self.push(Entry::RunError { id: format!("err-{at}"), message: message.clone(), interrupted: code.as_deref() == Some(INTERRUPTED), at });
            }
            Event::RunFinished { .. } => {
                for e in &mut self.entries {
                    if let Entry::Tool { status, .. } = e {
                        if status == "running" {
                            *status = "done".into();
                        }
                    }
                }
            }
            // Text that never got content is noise, and so is a free-talk PASS
            Event::TextMessageEnd { message_id } => {
                if let Some(i) = self.entries.iter().rposition(|e| matches!(e, Entry::Text { id, text, .. } if id == message_id && (text.trim().is_empty() || text.trim() == PASS))) {
                    self.entries.remove(i);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agui::{claude::Claude, replay};

    #[test]
    fn a_replayed_session_keeps_whole_messages() {
        let mut h = Claude::new("t");
        let seen = replay::run(&mut h, include_str!("../../../../../scripts/fixtures/agui/claude-stream.ndjson"), |h, v| match v["type"].as_str() {
            Some("user") => h.prompt(v["message"]["content"].as_str().unwrap()),
            Some("control_request") => h.interrupt(),
            Some("control_response") => h.answer(v["response"]["request_id"].as_str().unwrap(), true, false),
            _ => vec![],
        });
        let mut t = Transcript::default();
        for (i, e) in seen.events.iter().enumerate() {
            t.apply(e, i as f64);
        }
        let texts: Vec<&str> = t.entries.iter().filter_map(|e| if let Entry::Text { text, .. } = e { Some(text.as_str()) } else { None }).collect();
        assert!(texts.iter().any(|x| x.contains("KIWI-9")));
        assert!(texts.iter().all(|x| !x.trim().is_empty()));
        let tools: Vec<(&str, &str)> = t.entries.iter().filter_map(|e| if let Entry::Tool { name, status, .. } = e { Some((name.as_str(), status.as_str())) } else { None }).collect();
        assert!(tools.contains(&("PowerShell", "done")));
        assert!(tools.iter().all(|(_, s)| *s != "running"));
        assert!(t.entries.iter().any(|e| matches!(e, Entry::RunError { interrupted: true, .. })));
        // Tool args come back whole, so the UI can show the command
        assert!(t.entries.iter().any(|e| matches!(e, Entry::Tool { args, .. } if args.contains("node --test"))));
    }

    #[test]
    fn an_approval_settles_once_and_expires_when_the_process_dies() {
        let mut t = Transcript::default();
        let a = Approval { id: "a1".into(), tool: "Bash".into(), summary: "ls".into(), input: serde_json::Value::Null, session_scope: false };
        t.approval(&a, 1.0);
        assert!(t.settle("a1", "allowed"));
        assert!(!t.settle("a1", "denied"));
        t.approval(&Approval { id: "a2".into(), ..a }, 2.0);
        t.expire_pending();
        assert!(matches!(&t.entries[1], Entry::Approval { status, .. } if status == "expired"));
    }

    #[test]
    fn a_pass_leaves_nothing() {
        let mut t = Transcript::default();
        for (id, text) in [("a", " PASS\n"), ("b", "PASS on this one: I can help")] {
            t.apply(&Event::TextMessageStart { message_id: id.into(), role: "assistant".into() }, 0.0);
            t.apply(&Event::TextMessageContent { message_id: id.into(), delta: text.into() }, 0.0);
            t.apply(&Event::TextMessageEnd { message_id: id.into() }, 0.0);
        }
        assert_eq!(t.entries.len(), 1, "only an exact PASS is dropped");
    }

    #[test]
    fn keeps_the_newest_entries() {
        let mut t = Transcript::default();
        for i in 0..MAX_ENTRIES + 5 {
            t.user(&i.to_string(), "x", "user", i as f64);
        }
        assert_eq!(t.entries.len(), MAX_ENTRIES);
        assert!(matches!(&t.entries[0], Entry::User { id, .. } if id == "5"));
    }
}
