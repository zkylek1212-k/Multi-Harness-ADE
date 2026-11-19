//! A long-lived child that talks in lines: stdin stays open, every stdout line reaches a callback as it arrives.
//! For CLIs driven over a structured protocol (stream-json, JSON-RPC); `runner` is for one-shot runs.
use std::io::{BufRead, BufReader, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::Mutex;

use crate::launch::launch_plan;
use crate::runner::kill_tree;

/// A line longer than this is dropped, not buffered: no protocol line of ours comes close
const MAX_LINE: usize = 16 * 1024 * 1024;

pub struct LineProcess {
    pub pid: u32,
    stdin: Mutex<Option<ChildStdin>>,
}

impl LineProcess {
    /// False once the child has gone or stdin was closed
    pub fn write_line(&self, line: &str) -> bool {
        let mut g = self.stdin.lock().unwrap();
        let Some(s) = g.as_mut() else { return false };
        let ok = s.write_all(line.as_bytes()).and_then(|_| s.write_all(b"\n")).and_then(|_| s.flush()).is_ok();
        if !ok {
            *g = None;
        }
        ok
    }

    /// Ends the whole tree; the exit callback still runs
    pub fn kill(&self) {
        self.stdin.lock().unwrap().take();
        kill_tree(self.pid);
    }
}

fn read_lines(from: impl std::io::Read, mut each: impl FnMut(String)) {
    let mut r = BufReader::new(from);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match r.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) if buf.len() > MAX_LINE => continue,
            Ok(_) => {
                let s = String::from_utf8_lossy(&buf);
                let s = s.trim_end_matches(['\n', '\r']);
                if !s.is_empty() {
                    each(s.to_string());
                }
            }
        }
    }
}

/// `env` replaces the whole environment. `on_exit` runs once, after stdout has been read to the end.
pub fn spawn_lines(
    command: &str,
    args: &[String],
    cwd: &str,
    env: &[(String, String)],
    on_pid: impl FnOnce(u32),
    on_stdout: impl FnMut(String) + Send + 'static,
    on_stderr: impl FnMut(String) + Send + 'static,
    on_exit: impl FnOnce(Option<i32>) + Send + 'static,
) -> Result<std::sync::Arc<LineProcess>, String> {
    let plan = launch_plan(command, args, cfg!(windows))?;
    let mut c = Command::new(&plan.file);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
        if plan.verbatim {
            c.raw_arg(plan.args.join(" "));
        } else {
            c.args(&plan.args);
        }
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::process::CommandExt;
        c.args(&plan.args);
        c.process_group(0);
    }
    c.current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).env_clear().envs(env.iter().map(|(k, v)| (k, v)));
    let mut child = c.spawn().map_err(|e| format!("{command}: {e}"))?;
    let pid = child.id();
    on_pid(pid);
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let proc = std::sync::Arc::new(LineProcess { pid, stdin: Mutex::new(child.stdin.take()) });
    std::thread::spawn(move || read_lines(stderr, on_stderr));
    std::thread::spawn(move || {
        read_lines(stdout, on_stdout);
        let code = child.wait().ok().and_then(|s| s.code());
        on_exit(code);
    });
    Ok(proc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn node_env() -> Vec<(String, String)> {
        std::env::vars().collect()
    }

    #[test]
    fn echoes_lines_until_stdin_closes() {
        let (tx, rx) = mpsc::channel::<String>();
        let (done_tx, done_rx) = mpsc::channel::<Option<i32>>();
        let script = "process.stdin.setEncoding('utf8');let b='';process.stdin.on('data',d=>{b+=d;let i;while((i=b.indexOf('\\n'))>=0){console.log('got:'+b.slice(0,i));b=b.slice(i+1)}});process.stdin.on('end',()=>process.exit(3))";
        let p = spawn_lines("node", &["-e".into(), script.into()], ".", &node_env(), |_| {}, move |l| tx.send(l).unwrap(), |_| {}, move |c| done_tx.send(c).unwrap()).unwrap();
        assert!(p.write_line("中文 {\"a\":1}"));
        assert_eq!(rx.recv_timeout(Duration::from_secs(20)).unwrap(), "got:中文 {\"a\":1}");
        p.stdin.lock().unwrap().take();
        assert_eq!(done_rx.recv_timeout(Duration::from_secs(20)).unwrap(), Some(3));
        assert!(!p.write_line("late"), "nothing goes to a closed stdin");
    }

    #[test]
    fn kill_ends_the_child_and_still_reports_exit() {
        let (done_tx, done_rx) = mpsc::channel::<Option<i32>>();
        let p = spawn_lines("node", &["-e".into(), "setInterval(()=>{},1000)".into()], ".", &node_env(), |_| {}, |_| {}, |_| {}, move |c| done_tx.send(c).unwrap()).unwrap();
        p.kill();
        assert!(done_rx.recv_timeout(Duration::from_secs(20)).is_ok());
    }

    #[test]
    fn a_missing_program_is_an_error() {
        assert!(spawn_lines("definitely-not-a-program-wb", &[], ".", &node_env(), |_| {}, |_| {}, |_| {}, |_| {}).is_err());
    }
}
