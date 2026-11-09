//! One headless CLI run: prompt on stdin, bounded output, timeout and cancel take the whole tree down.
//! Mirrors `runProcess` in `src/main/cowork/runner.ts`.
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::launch::launch_plan;

#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn same(&self, other: &CancelToken) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

pub struct RunSpec {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub stdin: String,
    pub timeout_ms: u64,
    /// Each of stdout and stderr; going over stops the process.
    pub max_bytes: usize,
    /// None inherits this process's environment; Some replaces it entirely.
    pub env: Option<Vec<(String, String)>>,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct RunResult {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub cancelled: bool,
    pub truncated: bool,
    pub ms: u64,
    pub spawn_error: Option<String>,
}

#[cfg(windows)]
pub fn kill_tree(pid: u32) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).creation_flags(CREATE_NO_WINDOW).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

#[cfg(not(windows))]
pub fn kill_tree(pid: u32) {
    let _ = Command::new("kill").args(["-KILL", &format!("-{pid}")]).status();
}

/// Receives output as it arrives. Called from one thread at a time.
pub trait OutputSink: Send {
    fn on_spawn(&mut self, _pid: u32) {}
    /// stream: 0 stdout, 1 stderr
    fn on_output(&mut self, stream: u8, bytes: &[u8]);
}

#[derive(Default)]
struct Collect {
    out: Vec<u8>,
    err: Vec<u8>,
}

impl OutputSink for Collect {
    fn on_output(&mut self, stream: u8, bytes: &[u8]) {
        if stream == 0 {
            self.out.extend_from_slice(bytes);
        } else {
            self.err.extend_from_slice(bytes);
        }
    }
}

fn pump(mut from: impl Read, stream: u8, max: usize, over: &AtomicBool, sink: &Mutex<&mut dyn OutputSink>) {
    let mut total = 0usize;
    let mut buf = [0u8; 16 * 1024];
    while let Ok(n) = from.read(&mut buf) {
        if n == 0 {
            break;
        }
        total += n;
        if total > max {
            over.store(true, Ordering::SeqCst);
            continue; // keep draining so the child never blocks on a full pipe
        }
        sink.lock().unwrap().on_output(stream, &buf[..n]);
    }
}

pub fn run_process(spec: &RunSpec, cancel: &CancelToken) -> RunResult {
    let mut sink = Collect::default();
    let mut r = run_process_with(spec, cancel, &mut sink, |_| {});
    r.stdout = String::from_utf8_lossy(&sink.out).into_owned();
    r.stderr = String::from_utf8_lossy(&sink.err).into_owned();
    r
}

/// Output goes to the sink; stdout and stderr in the result stay empty.
/// `on_pid` runs right after spawning: assign the process to a job there, before it can start children.
pub fn run_process_with(spec: &RunSpec, cancel: &CancelToken, sink: &mut dyn OutputSink, on_pid: impl FnOnce(u32)) -> RunResult {
    let t0 = Instant::now();
    let plan = match launch_plan(&spec.command, &spec.args, cfg!(windows)) {
        Ok(p) => p,
        Err(e) => return RunResult { spawn_error: Some(e), ..Default::default() },
    };
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
    c.current_dir(&spec.cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(env) = &spec.env {
        c.env_clear();
        for (k, v) in env {
            c.env(k, v);
        }
    }
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => return RunResult { spawn_error: Some(e.to_string()), ms: t0.elapsed().as_millis() as u64, ..Default::default() },
    };
    let pid = child.id();
    on_pid(pid);
    sink.on_spawn(pid);
    let stdin = child.stdin.take();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let over = AtomicBool::new(false);
    let shared = Mutex::new(sink);
    let deadline = Duration::from_millis(spec.timeout_ms);
    let (mut timed_out, mut cancelled, mut truncated) = (false, false, false);

    let code = std::thread::scope(|scope| {
        // Writing stdin can fail if the process dies at once; that must not take the host down.
        scope.spawn(|| {
            if let Some(mut s) = stdin {
                let _ = s.write_all(spec.stdin.as_bytes());
            }
        });
        scope.spawn(|| pump(stdout, 0, spec.max_bytes, &over, &shared));
        scope.spawn(|| pump(stderr, 1, spec.max_bytes, &over, &shared));
        let mut stopped = false;
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                break status.code();
            }
            if !stopped {
                if t0.elapsed() >= deadline {
                    timed_out = true;
                    stopped = true;
                } else if cancel.is_cancelled() {
                    cancelled = true;
                    stopped = true;
                } else if over.load(Ordering::SeqCst) {
                    truncated = true;
                    stopped = true;
                }
                if stopped {
                    kill_tree(pid);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    RunResult { code, stdout: String::new(), stderr: String::new(), timed_out, cancelled, truncated: truncated || over.load(Ordering::SeqCst), ms: t0.elapsed().as_millis() as u64, spawn_error: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(script: &str, stdin: &str, timeout_ms: u64, max_bytes: usize) -> RunSpec {
        RunSpec {
            command: if cfg!(windows) { "node.exe".into() } else { "node".into() },
            args: vec!["-e".into(), script.into()],
            cwd: std::env::temp_dir().to_string_lossy().into_owned(),
            stdin: stdin.into(),
            timeout_ms,
            max_bytes,
            env: None,
        }
    }

    #[test]
    fn echoes_stdin_with_unicode_and_percent() {
        let s = node("process.stdin.pipe(process.stdout)", "中文 \"quotes\" %PATH%\nline2", 20000, 1_000_000);
        let r = run_process(&s, &CancelToken::default());
        assert_eq!(r.code, Some(0), "{r:?}");
        assert_eq!(r.stdout, "中文 \"quotes\" %PATH%\nline2");
    }

    #[test]
    fn exit_code_and_stderr() {
        let r = run_process(&node("console.error('boom');process.exit(3)", "", 20000, 1_000_000), &CancelToken::default());
        assert_eq!(r.code, Some(3));
        assert!(r.stderr.contains("boom"));
    }

    #[test]
    fn timeout_kills() {
        let r = run_process(&node("setInterval(()=>{},1000)", "", 300, 1_000_000), &CancelToken::default());
        assert!(r.timed_out);
        assert!(r.ms < 15_000);
    }

    #[test]
    fn cancel_kills() {
        let c = CancelToken::default();
        let c2 = c.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            c2.cancel();
        });
        let r = run_process(&node("setInterval(()=>{},1000)", "", 30000, 1_000_000), &c);
        assert!(r.cancelled);
    }

    #[test]
    fn too_much_output_stops_it() {
        let r = run_process(&node("setInterval(()=>process.stdout.write('x'.repeat(65536)),1)", "", 30000, 200_000), &CancelToken::default());
        assert!(r.truncated, "{:?}", r.code);
        assert!(r.stdout.len() <= 200_000);
    }

    #[test]
    fn missing_executable_is_reported() {
        let mut s = node("", "", 1000, 100);
        s.command = "definitely-not-a-command-xyz".into();
        assert!(run_process(&s, &CancelToken::default()).spawn_error.is_some());
    }

    #[test]
    fn unsafe_cmd_argument_is_refused_before_spawning() {
        let mut s = node("", "", 1000, 100);
        s.command = "C:/missing/tool.cmd".into();
        s.args = vec!["100%".into()];
        if cfg!(windows) {
            assert!(run_process(&s, &CancelToken::default()).spawn_error.unwrap().contains("refusing"));
        }
    }
}
