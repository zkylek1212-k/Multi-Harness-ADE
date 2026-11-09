// Real Claude Code output through ConPTY (scripts/fixtures, personal paths replaced), with four resizes in the middle.
// The mirror has to be the one truth: a phone attached midway and one attached last must see the same, with one copy of the chat.
use serde_json::Value;
use wb_pty::mirror::Mirror;

const SCROLLBACK: usize = 5000;

/// What the app sends ahead of a resize, so ConPTY's reprint does not stack on the old screen
const PRELUDE: &[u8] = b"\x1b[H\x1b[2J\x1b[3J";

fn text(m: &Mirror) -> String {
    let rows: Vec<String> = m.all_rows().iter().map(|l| l.trim_end().to_string()).collect();
    rows.join("\n").trim_end().to_string()
}

fn phone(desk: &Mirror) -> Mirror {
    let (cols, rows) = desk.size();
    let mut t = Mirror::new(cols, rows, SCROLLBACK);
    t.process(desk.serialize(SCROLLBACK).as_bytes());
    t
}

#[test]
fn a_phone_sees_the_same_whenever_it_attached() {
    let raw = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../scripts/fixtures/claude-resize-conpty.json")).unwrap();
    let events: Vec<Value> = serde_json::from_str(&raw).unwrap();
    let mut desk = Mirror::new(120, 30, SCROLLBACK);
    let mut early: Option<Mirror> = None;
    for (i, e) in events.iter().enumerate() {
        if e[0] == "d" {
            let chunk = e[1].as_str().unwrap().as_bytes();
            desk.process(chunk);
            if let Some(p) = early.as_mut() {
                p.process(chunk);
            }
        } else {
            let (c, r) = (e[1].as_u64().unwrap() as u16, e[2].as_u64().unwrap() as u16);
            desk.process(PRELUDE);
            desk.resize(c, r);
            if let Some(p) = early.as_mut() {
                p.process(PRELUDE);
                p.resize(c, r);
            }
        }
        if i == 100 {
            early = Some(phone(&desk));
        }
    }
    let late = phone(&desk);
    assert_eq!(text(&early.unwrap()), text(&late), "attached midway matches a fresh snapshot");
    let t = text(&late);
    assert_eq!(t.matches("Claude Code v2.1.280").count(), 1, "one copy of the transcript after resizes");
    assert_eq!(t.matches("5. Scrollback buffers keep earlier output").count(), 1, "no duplicated reply");
}
