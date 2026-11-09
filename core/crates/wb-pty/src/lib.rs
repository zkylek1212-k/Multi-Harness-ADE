//! PTY sessions: a reader thread, a headless mirror, optional credit-based flow control.
//!
//! `on_data` runs while holding the mirror lock. A caller that queues frames there and also queues snapshots under
//! the same lock gets a stream where a snapshot sits at an exact position: nothing missed, nothing repeated.
pub mod approval;
pub mod mirror;

use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use mirror::Mirror;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

const DSR_QUERY: &[u8] = b"\x1b[6n";

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

pub struct PtyOptions {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    /// None inherits this process's environment; Some replaces it.
    pub env: Option<Vec<(String, String)>>,
    pub cols: u16,
    pub rows: u16,
    pub scrollback: usize,
    /// Bytes the consumer may hold before the reader waits. None = no limit.
    pub credit: Option<i64>,
}

type SharedWriter = Arc<Mutex<Option<Box<dyn Write + Send>>>>;

pub struct PtySession {
    master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    writer: SharedWriter,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    mirror: Arc<Mutex<Mirror>>,
    credit: Arc<(Mutex<i64>, Condvar)>,
    stalls: Arc<AtomicU64>,
    pid: Option<u32>,
}

impl PtySession {
    pub fn spawn(o: PtyOptions, mut on_data: impl FnMut(&[u8]) + Send + 'static, on_exit: impl FnOnce(i32) + Send + 'static) -> Result<Self, String> {
        let pair = native_pty_system().openpty(PtySize { rows: o.rows, cols: o.cols, pixel_width: 0, pixel_height: 0 }).map_err(|e| e.to_string())?;
        let mut cmd = CommandBuilder::new(&o.program);
        cmd.args(&o.args);
        if let Some(cwd) = &o.cwd {
            cmd.cwd(cwd);
        }
        if let Some(env) = &o.env {
            cmd.env_clear();
            for (k, v) in env {
                cmd.env(k, v);
            }
        }
        let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        drop(pair.slave);
        let pid = child.process_id();
        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer: SharedWriter = Arc::new(Mutex::new(Some(pair.master.take_writer().map_err(|e| e.to_string())?)));
        let master = Arc::new(Mutex::new(Some(pair.master)));
        let child = Arc::new(Mutex::new(child));
        let mirror = Arc::new(Mutex::new(Mirror::new(o.cols, o.rows, o.scrollback)));
        let credit = Arc::new((Mutex::new(o.credit.unwrap_or(i64::MAX / 2)), Condvar::new()));
        let stalls = Arc::new(AtomicU64::new(0));
        let code: Arc<Mutex<Option<i32>>> = Arc::default();

        // ConPTY keeps the output pipe open after the child exits, until the pseudoconsole is closed. So a waiter has
        // to notice the exit and close it, and that is also what makes the reader see the final output and then EOF.
        {
            let (child, master, writer, code, credit) = (child.clone(), master.clone(), writer.clone(), code.clone(), credit.clone());
            std::thread::spawn(move || loop {
                let status = child.lock().unwrap().try_wait();
                match status {
                    Ok(Some(s)) => {
                        *code.lock().unwrap() = Some(s.exit_code() as i32);
                        // A reader waiting for credit must still reach EOF, or closing the pseudoconsole can hang.
                        *credit.0.lock().unwrap() = i64::MAX / 2;
                        credit.1.notify_all();
                        writer.lock().unwrap().take();
                        master.lock().unwrap().take();
                        return;
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                    Err(_) => {
                        *code.lock().unwrap() = Some(-1);
                        master.lock().unwrap().take();
                        return;
                    }
                }
            });
        }

        let (mirror2, credit2, stalls2, writer2, code2) = (mirror.clone(), credit.clone(), stalls.clone(), writer.clone(), code.clone());
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            let mut answered_startup_query = false;
            loop {
                {
                    let (lock, cv) = &*credit2;
                    let mut left = lock.lock().unwrap();
                    if *left <= 0 {
                        stalls2.fetch_add(1, Ordering::Relaxed);
                        while *left <= 0 {
                            left = cv.wait(left).unwrap();
                        }
                    }
                }
                let n = match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                // portable-pty asks ConPTY to inherit the cursor, so conhost sends ESC[6n and produces no output until
                // it gets an answer. Answer that first one here; later queries belong to the terminal front end.
                if !answered_startup_query && contains(&buf[..n], DSR_QUERY) {
                    answered_startup_query = true;
                    if let Some(w) = writer2.lock().unwrap().as_mut() {
                        let _ = w.write_all(b"\x1b[1;1R");
                    }
                }
                let mut m = mirror2.lock().unwrap();
                m.process(&buf[..n]);
                *credit2.0.lock().unwrap() -= n as i64;
                on_data(&buf[..n]);
            }
            let t = Instant::now();
            let exit = loop {
                if let Some(c) = *code2.lock().unwrap() {
                    break c;
                }
                if t.elapsed() > Duration::from_secs(3) {
                    break -1;
                }
                std::thread::sleep(Duration::from_millis(20));
            };
            on_exit(exit);
        });
        Ok(Self { master, writer, child, mirror, credit, stalls, pid })
    }

    pub fn add_credit(&self, n: i64) {
        let (lock, cv) = &*self.credit;
        *lock.lock().unwrap() += n;
        cv.notify_all();
    }

    pub fn write(&self, data: &[u8]) -> std::io::Result<()> {
        match self.writer.lock().unwrap().as_mut() {
            Some(w) => w.write_all(data),
            None => Ok(()), // already exited
        }
    }

    /// Resize in stream order: the prelude goes in first, the size changes after it, and `emit` sees them under the
    /// mirror lock. ConPTY's repaint arrives through the reader, so it lands behind both.
    pub fn resize_ordered(&self, cols: u16, rows: u16, prelude: &[u8], emit: impl FnOnce(&[u8])) -> Result<(), String> {
        if let Some(m) = self.master.lock().unwrap().as_ref() {
            m.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(|e| e.to_string())?;
        }
        let mut m = self.mirror.lock().unwrap();
        if !prelude.is_empty() {
            m.process(prelude);
        }
        emit(prelude);
        m.resize(cols, rows);
        Ok(())
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    pub fn kill(&self) -> std::io::Result<()> {
        self.child.lock().unwrap().kill()
    }

    pub fn stalls(&self) -> u64 {
        self.stalls.load(Ordering::Relaxed)
    }

    /// Runs with the mirror locked, so whatever `f` queues holds an exact position in the output stream.
    pub fn with_mirror<T>(&self, f: impl FnOnce(&Mirror) -> T) -> T {
        f(&self.mirror.lock().unwrap())
    }

    /// Bytes that redraw the current screen, for a client attaching or catching up.
    pub fn snapshot(&self) -> Vec<u8> {
        self.with_mirror(|m| m.serialize(0).into_bytes())
    }

    /// Visible rows, the input of approval detection.
    pub fn screen_text(&self) -> String {
        self.with_mirror(|m| m.rows().join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn node_opts(script: &str, credit: Option<i64>) -> PtyOptions {
        PtyOptions {
            program: if cfg!(windows) { "node.exe".into() } else { "node".into() },
            args: vec!["-e".into(), script.into()],
            cwd: None,
            env: None,
            cols: 80,
            rows: 24,
            scrollback: 100,
            credit,
        }
    }

    #[test]
    fn credit_stops_the_reader_until_returned() {
        // ConPTY only forwards screen changes, so the output has to keep changing over time, like a real agent.
        let (tx, rx) = channel::<usize>();
        let session = PtySession::spawn(
            node_opts("let i=0;setInterval(()=>console.log('line '+(i++)+' '+'x'.repeat(60)),2)", Some(4096)),
            move |b: &[u8]| {
                let _ = tx.send(b.len());
            },
            |_| {},
        )
        .unwrap();
        let drain = |ms: u64| {
            let end = Instant::now() + Duration::from_millis(ms);
            let mut got = 0;
            while let Some(left) = end.checked_duration_since(Instant::now()) {
                if let Ok(n) = rx.recv_timeout(left) {
                    got += n;
                }
            }
            got
        };
        let first = drain(1500);
        assert!(first >= 4096, "should reach the credit limit, got {first}");
        assert!(first <= 4096 + 64 * 1024, "reader kept going past its credit: {first}");
        assert!(session.stalls() >= 1, "reader should have stopped");
        assert_eq!(drain(400), 0, "nothing flows without credit");
        session.add_credit(1024 * 1024);
        assert!(drain(800) > 0, "returned credit resumes reading");
        let _ = session.kill();
    }

    #[test]
    fn exit_code_and_final_output_arrive() {
        let (tx, rx) = channel::<i32>();
        let (dtx, drx) = channel::<Vec<u8>>();
        let _session = PtySession::spawn(
            node_opts("console.log('goodbye-marker');process.exit(7)", None),
            move |b: &[u8]| {
                let _ = dtx.send(b.to_vec());
            },
            move |c| {
                let _ = tx.send(c);
            },
        )
        .unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(20)).expect("exit event"), 7);
        let all: Vec<u8> = drx.try_iter().flatten().collect();
        assert!(String::from_utf8_lossy(&all).contains("goodbye-marker"), "{:?}", String::from_utf8_lossy(&all));
    }
}
