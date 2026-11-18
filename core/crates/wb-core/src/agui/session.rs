//! One chat node's process: a harness on top of a line process. Everything the harness wants written goes to stdin;
//! everything else goes to `sink`, in order, from the reader thread or the caller's thread.
use std::sync::{Arc, Mutex, OnceLock};

use wb_proc::{spawn_lines, LineProcess};

use super::{Harness, Out};

pub type Sink = Arc<dyn Fn(Out) + Send + Sync>;

const STDERR_KEEP: usize = 20;

struct Inner {
    h: Mutex<Box<dyn Harness>>,
    proc: OnceLock<Arc<LineProcess>>,
    /// Lines the harness produced before the process handle existed
    early: Mutex<Vec<String>>,
    stderr: Mutex<Vec<String>>,
    sink: Sink,
}

impl Inner {
    fn dispatch(&self, outs: Vec<Out>) {
        for o in outs {
            match o {
                Out::Send(line) => match self.proc.get() {
                    Some(p) => {
                        p.write_line(&line);
                    }
                    None => self.early.lock().unwrap().push(line),
                },
                other => (self.sink)(other),
            }
        }
    }
}

pub struct Chat {
    inner: Arc<Inner>,
}

pub struct Spawn<'a> {
    pub command: &'a str,
    pub args: &'a [String],
    pub cwd: &'a str,
    pub env: &'a [(String, String)],
}

impl Chat {
    pub fn spawn(s: Spawn, harness: Box<dyn Harness>, on_pid: impl FnOnce(u32), sink: Sink, on_exit: impl FnOnce(Option<i32>, String) + Send + 'static) -> Result<Arc<Chat>, String> {
        let inner = Arc::new(Inner { h: Mutex::new(harness), proc: OnceLock::new(), early: Mutex::default(), stderr: Mutex::default(), sink });
        let (a, b, c) = (inner.clone(), inner.clone(), inner.clone());
        let proc = spawn_lines(
            s.command,
            s.args,
            s.cwd,
            s.env,
            on_pid,
            move |line| {
                let outs = a.h.lock().unwrap().on_line(&line);
                a.dispatch(outs);
            },
            move |line| {
                let mut e = b.stderr.lock().unwrap();
                e.push(line);
                if e.len() > STDERR_KEEP {
                    e.remove(0);
                }
            },
            move |code| {
                let tail = c.stderr.lock().unwrap().join("\n");
                on_exit(code, tail);
            },
        )?;
        let _ = inner.proc.set(proc.clone());
        for line in std::mem::take(&mut *inner.early.lock().unwrap()) {
            proc.write_line(&line);
        }
        let outs = inner.h.lock().unwrap().start();
        inner.dispatch(outs);
        Ok(Arc::new(Chat { inner }))
    }

    fn with(&self, f: impl FnOnce(&mut dyn Harness) -> Vec<Out>) {
        let outs = f(self.inner.h.lock().unwrap().as_mut());
        self.inner.dispatch(outs);
    }

    pub fn prompt(&self, text: &str) {
        self.with(|h| h.prompt(text));
    }

    pub fn interrupt(&self) {
        self.with(|h| h.interrupt());
    }

    pub fn answer(&self, id: &str, allow: bool, for_session: bool) {
        self.with(|h| h.answer(id, allow, for_session));
    }

    pub fn pid(&self) -> Option<u32> {
        self.inner.proc.get().map(|p| p.pid)
    }

    pub fn kill(&self) {
        if let Some(p) = self.inner.proc.get() {
            p.kill();
        }
    }
}

/// Env for a headless CLI: what this app's own parent left behind must not reach it. From inside a Claude Code
/// session a child Claude would inherit auto mode and the parent's messaging socket (P0, cowork-agui-implement.md §4.1).
pub fn clean_env(env: Vec<(String, String)>) -> Vec<(String, String)> {
    env.into_iter().filter(|(k, _)| !(k.to_ascii_uppercase().starts_with("CLAUDE") || k.eq_ignore_ascii_case("AI_AGENT"))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agui::{claude::Claude, Event};
    use std::sync::mpsc;
    use std::time::Duration;

    /// A fake Claude: answers every user line with one streamed text block and a result
    const FAKE: &str = r#"
const rl=require('readline').createInterface({input:process.stdin});
console.log(JSON.stringify({type:'system',subtype:'init',session_id:'s1'}));
rl.on('line',l=>{const m=JSON.parse(l);if(m.type!=='user')return;
const t='echo:'+m.message.content;
for(const e of [{type:'message_start',message:{id:'m'}},{type:'content_block_start',index:0,content_block:{type:'text'}},{type:'content_block_delta',index:0,delta:{type:'text_delta',text:t}},{type:'content_block_stop',index:0}])
 console.log(JSON.stringify({type:'stream_event',event:e,parent_tool_use_id:null}));
console.log(JSON.stringify({type:'result',subtype:'success',is_error:false,result:t}));});
rl.on('close',()=>process.exit(0));
"#;

    #[test]
    fn a_prompt_round_trips_through_a_real_process() {
        let (tx, rx) = mpsc::channel::<Out>();
        let (dtx, drx) = mpsc::channel::<Option<i32>>();
        let env = clean_env(std::env::vars().collect());
        let args = vec!["-e".to_string(), FAKE.to_string()];
        let tx = Mutex::new(tx);
        let chat = Chat::spawn(Spawn { command: "node", args: &args, cwd: ".", env: &env }, Box::new(Claude::new("t")), |_| {}, Arc::new(move |o| tx.lock().unwrap().send(o).unwrap()), move |c, _| dtx.send(c).unwrap()).unwrap();
        chat.prompt("hi");
        let mut got = vec![];
        while let Ok(o) = rx.recv_timeout(Duration::from_secs(20)) {
            let end = matches!(o, Out::Ui(Event::RunFinished { .. }));
            got.push(o);
            if end {
                break;
            }
        }
        assert!(got.contains(&Out::Session("s1".into())));
        assert!(got.iter().any(|o| matches!(o, Out::Ui(Event::TextMessageContent { delta, .. }) if delta == "echo:hi")));
        chat.kill();
        assert!(drx.recv_timeout(Duration::from_secs(20)).is_ok());
    }

    #[test]
    fn clean_env_drops_the_parent_session() {
        let env = clean_env(vec![("CLAUDECODE".into(), "1".into()), ("claude_code_x".into(), "1".into()), ("AI_AGENT".into(), "x".into()), ("PATH".into(), "p".into())]);
        assert_eq!(env, vec![("PATH".to_string(), "p".to_string())]);
    }
}
