//! `wb-hook <claude|codex|agy>`: the one hook command the app installs for every CLI (cowork2.0-implement.md §3.4).
//! Reads the CLI's request on stdin, asks the app over a named pipe, prints the decision in that CLI's format.
//! Anything that goes wrong prints "hand back to the CLI", never allow: no env (run outside the app), app gone,
//! bad token, timeout. Phase 0 showed agy treats a failed or timed-out hook as a blocked tool, so the answer has
//! to come from here before the CLI's own hook timeout.
//!
//! A named pipe, not loopback TCP: on the dev PC about one loopback connection in five was reset by a filter
//! driver (measured 2026-11-12), and a pipe is not reachable from the network at all.
//!
//! Wire protocol, one pipe connection per request:
//!   hook -> app: `WBHOOK1 <token> <cli> <byte length>\n` then exactly that many stdin bytes
//!   app -> hook: one line, `allow` | `deny <message>` | `pass`
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub const PIPE_PREFIX: &str = r"\\.\pipe\agent-workbench-hook-";

#[derive(Debug, PartialEq)]
enum Decision {
    Allow,
    Deny(String),
    Pass,
}

mod mcp;

fn main() {
    let cli = std::env::args().nth(1).unwrap_or_default();
    if cli == "mcp" {
        mcp::run();
        return;
    }
    let mut input = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut input);
    let decision = ask(&cli, &input).unwrap_or(Decision::Pass);
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(render(&cli, &decision).as_bytes());
    let _ = stdout.flush();
    // A reader thread may still be blocked on the pipe after a timeout; leaving main ends it
    std::process::exit(0);
}

fn ask(cli: &str, input: &[u8]) -> Option<Decision> {
    try_ask(cli, input).ok().flatten()
}

fn env_ok(name: &str, ok: impl Fn(&str) -> bool) -> Result<String, String> {
    std::env::var(name).ok().filter(|v| ok(v)).ok_or_else(|| format!("{name} missing or invalid"))
}

fn safe_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn try_ask(cli: &str, input: &[u8]) -> Result<Option<Decision>, String> {
    if !matches!(cli, "claude" | "codex" | "agy") {
        return Err("unknown CLI".into());
    }
    // Only the app's own pipe namespace: an injected value cannot point the hook at a file
    let pipe = env_ok("WB_HOOK_PIPE", |v| v.strip_prefix(PIPE_PREFIX).is_some_and(safe_id))?;
    let token = env_ok("WB_HOOK_TOKEN", safe_id)?;
    let wait = Duration::from_millis(std::env::var("WB_HOOK_TIMEOUT_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(590_000));
    let mut pipe = open(&pipe)?;
    pipe.write_all(format!("WBHOOK1 {token} {cli} {}\n", input.len()).as_bytes()).map_err(|e| e.to_string())?;
    pipe.write_all(input).map_err(|e| e.to_string())?;
    pipe.flush().map_err(|e| e.to_string())?;
    // std has no read timeout on a pipe, so the read runs on its own thread
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let r = BufReader::new(pipe.take(64 * 1024)).read_line(&mut line).map(|_| line);
        let _ = tx.send(r);
    });
    let reply = rx.recv_timeout(wait).map_err(|_| "timed out")?.map_err(|e| e.to_string())?;
    Ok(parse(&reply))
}

/// ERROR_PIPE_BUSY means every server instance is taken for a moment; anything else means the app is not there
fn open(path: &str) -> Result<std::fs::File, String> {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        match std::fs::OpenOptions::new().read(true).write(true).open(path) {
            Ok(f) => return Ok(f),
            Err(e) if e.raw_os_error() == Some(231) && Instant::now() < until => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn parse(reply: &str) -> Option<Decision> {
    let line = reply.lines().next()?.trim();
    match line.split_once(' ').unwrap_or((line, "")) {
        ("allow", "") => Some(Decision::Allow),
        ("deny", msg) => Some(Decision::Deny(if msg.trim().is_empty() { "Rejected in Agent Workbench".into() } else { msg.trim().into() })),
        _ => None,
    }
}

/// Claude and Codex: PermissionRequest output, nothing at all to hand back. agy: PreToolUse `decision`, `ask` to hand back.
fn render(cli: &str, d: &Decision) -> String {
    match (cli, d) {
        ("agy", Decision::Allow) => r#"{"decision":"allow"}"#.into(),
        ("agy", Decision::Deny(m)) => format!(r#"{{"decision":"deny","reason":{}}}"#, json_str(m)),
        ("agy", Decision::Pass) => r#"{"decision":"ask"}"#.into(),
        (_, Decision::Allow) => r#"{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}"#.into(),
        (_, Decision::Deny(m)) => format!(r#"{{"hookSpecificOutput":{{"hookEventName":"PermissionRequest","decision":{{"behavior":"deny","message":{}}}}}}}"#, json_str(m)),
        (_, Decision::Pass) => String::new(),
    }
}

fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::ServerOptions;

    #[test]
    fn formats_per_cli() {
        assert_eq!(render("claude", &Decision::Pass), "");
        assert_eq!(render("codex", &Decision::Pass), "");
        assert_eq!(render("agy", &Decision::Pass), r#"{"decision":"ask"}"#);
        assert_eq!(render("agy", &Decision::Allow), r#"{"decision":"allow"}"#);
        assert_eq!(
            render("claude", &Decision::Deny("no \"x\"\n".into())),
            r#"{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"deny","message":"no \"x\"\n"}}}"#
        );
    }

    #[test]
    fn anything_unexpected_hands_back() {
        for r in ["", "ALLOW", "allowed", "allow now", "yes", "pass"] {
            assert_eq!(parse(r), None, "{r:?}");
        }
        assert_eq!(parse("allow\n"), Some(Decision::Allow));
        assert_eq!(parse("deny  "), Some(Decision::Deny("Rejected in Agent Workbench".into())));
    }

    /// Env-dependent paths run in one test so parallel tests never see each other's variables
    #[test]
    fn talks_to_the_app_and_fails_closed() {
        std::env::remove_var("WB_HOOK_PIPE");
        assert!(try_ask("claude", b"{}").is_err(), "outside the app");
        let name = format!("{PIPE_PREFIX}test-{}", std::process::id());
        std::env::set_var("WB_HOOK_PIPE", &name);
        std::env::set_var("WB_HOOK_TOKEN", "tok-1");
        std::env::set_var("WB_HOOK_TIMEOUT_MS", "3000");
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        // Answers in order; "silent" never replies, to prove the timeout hands back
        // The first instance exists before any client opens, as in the app
        let first = rt.block_on(async { ServerOptions::new().first_pipe_instance(true).create(&name) }).unwrap();
        let server = rt.spawn({
            let name = name.clone();
            async move {
                let mut seen = Vec::new();
                let mut server = first;
                for answer in ["allow\n", "deny not now\n", "silent"] {
                    server.connect().await.unwrap();
                    let mut conn = server;
                    server = ServerOptions::new().create(&name).unwrap();
                    let mut r = tokio::io::BufReader::new(&mut conn);
                    let mut head = String::new();
                    r.read_line(&mut head).await.unwrap();
                    let len: usize = head.trim_end().rsplit(' ').next().unwrap().parse().unwrap();
                    let mut body = vec![0; len];
                    r.read_exact(&mut body).await.unwrap();
                    seen.push(head + &String::from_utf8(body).unwrap());
                    if answer != "silent" {
                        conn.write_all(answer.as_bytes()).await.unwrap();
                        conn.flush().await.unwrap();
                    } else {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
                seen
            }
        });
        assert_eq!(try_ask("codex", br#"{"tool_name":"apply_patch"}"#), Ok(Some(Decision::Allow)));
        assert_eq!(try_ask("agy", b"x"), Ok(Some(Decision::Deny("not now".into()))));
        assert_eq!(try_ask("claude", b"y"), Err("timed out".into()));
        let seen = rt.block_on(server).unwrap();
        assert_eq!(seen, vec!["WBHOOK1 tok-1 codex 27\n{\"tool_name\":\"apply_patch\"}", "WBHOOK1 tok-1 agy 1\nx", "WBHOOK1 tok-1 claude 1\ny"]);
        assert!(try_ask("bash", b"x").is_err(), "unknown CLI");
        std::env::set_var("WB_HOOK_PIPE", r"C:\Windows\win.ini");
        assert!(try_ask("claude", b"x").is_err(), "only the app's pipe namespace");
        std::env::set_var("WB_HOOK_PIPE", &name);
        std::env::set_var("WB_HOOK_TOKEN", "bad token");
        assert!(try_ask("claude", b"x").is_err(), "token with spaces");
    }
}
