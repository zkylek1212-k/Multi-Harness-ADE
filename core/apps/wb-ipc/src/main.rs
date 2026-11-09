// Stdin: {"id":1,"window":1,"channel":"files:read","args":["a.txt"]}
// Stdout: {"id":1,"result":...} or {"id":1,"error":"..."}; events {"event":"files:treeChange","window":1,"args":[]}
// Control: {"id":1,"channel":"test:openWindow","window":2,"args":["<workspace>"]} and test:closeWindow.
// Env: WB_DATA_DIR, WB_VERSION, WB_PICK_FOLDER (what the folder dialog returns).
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use serde_json::{json, Value};
use wb_core::cowork::{CoworkDeps, ResolvedCli, Service};
use wb_core::memsync::{self, RunOpts, RunResult, Runner, Tools};
use wb_core::{AppInfo, Core, Host};

#[derive(Default)]
struct Stub {
    windows: Mutex<Vec<u32>>,
    out: Mutex<()>,
}

impl Stub {
    fn print(&self, v: &Value) {
        let _g = self.out.lock().unwrap();
        let mut o = std::io::stdout().lock();
        let _ = writeln!(o, "{v}");
        let _ = o.flush();
    }
    fn action(&self, name: &str, args: Value) {
        self.print(&json!({ "event": format!("host:{name}"), "window": 0, "args": args }));
    }
}

impl Host for Stub {
    fn static_asset(&self, rel: &str) -> Option<Vec<u8>> {
        std::fs::read(Path::new(&std::env::var("WB_REMOTE_STATIC").ok()?).join(rel)).ok()
    }
    fn send(&self, window: u32, channel: &str, args: Value) {
        self.print(&json!({ "event": channel, "window": window, "args": args }));
    }
    fn send_all(&self, channel: &str, args: Value) {
        for w in self.windows() {
            self.send(w, channel, args.clone());
        }
    }
    fn focused_window(&self) -> Option<u32> {
        self.windows.lock().unwrap().first().copied()
    }
    fn windows(&self) -> Vec<u32> {
        self.windows.lock().unwrap().clone()
    }
    fn pick_folder(&self, _window: u32, _default_path: &str) -> Option<String> {
        std::env::var("WB_PICK_FOLDER").ok()
    }
    fn pick_file(&self, _title: &str) -> Option<String> {
        None
    }
    fn open_path(&self, path: &str) -> Result<(), String> {
        self.action("openPath", json!([path]));
        Ok(())
    }
    fn show_in_folder(&self, path: &str) {
        self.action("showInFolder", json!([path]));
    }
    fn open_url(&self, url: &str) {
        self.action("openUrl", json!([url]));
    }
    fn notify(&self, title: &str, body: &str) {
        self.action("notify", json!([title, body]));
    }
    fn set_title(&self, window: u32, title: &str) {
        self.action("setTitle", json!([window, title]));
    }
    fn open_project_window(&self, workspace: Option<&str>) -> bool {
        self.action("openProjectWindow", json!([workspace]));
        true
    }
    fn open_terminal_window(&self) -> bool {
        true
    }
    fn close_terminal_window(&self) -> bool {
        true
    }
    fn set_title_bar_theme(&self, _window: u32, _theme: &str) -> bool {
        true
    }
}

/// Runs memsync flows with the test's own fake GitHub: each outside call becomes a line out and a reply in
struct RemoteRunner {
    host: Arc<Stub>,
    pending: Arc<Mutex<HashMap<u64, mpsc::Sender<Value>>>>,
    next: AtomicU64,
    call: u64,
}

impl RemoteRunner {
    fn ask(&self, mut q: Value) -> Value {
        let cb = (self.call << 20) | self.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(cb, tx);
        q["cb"] = json!(cb);
        q["call"] = json!(self.call);
        self.host.print(&q);
        rx.recv().unwrap_or(Value::Null)
    }
    fn run_value(v: Value) -> RunResult {
        RunResult { code: v["code"].as_i64().unwrap_or(1) as i32, stdout: v["stdout"].as_str().unwrap_or("").to_string(), stderr: v["stderr"].as_str().unwrap_or("").to_string() }
    }
}

impl Runner for RemoteRunner {
    fn run(&self, cmd: &str, args: &[String], opts: &RunOpts) -> RunResult {
        let env: serde_json::Map<String, Value> = opts.env.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        Self::run_value(self.ask(json!({ "kind": "run", "cmd": cmd, "args": args, "opts": { "cwd": opts.cwd, "timeoutMs": opts.timeout_ms, "env": env, "input": opts.input } })))
    }
    fn http_status(&self, url: &str) -> Result<u16, String> {
        let v = self.ask(json!({ "kind": "http", "url": url }));
        match v["status"].as_u64() {
            Some(s) => Ok(s as u16),
            None => Err(v["error"].as_str().unwrap_or("failed").to_string()),
        }
    }
    fn github_api(&self, method: &str, path: &str, token: &str, body: Option<&Value>) -> Result<(u16, Value), String> {
        let v = self.ask(json!({ "kind": "api", "method": method, "path": path, "token": token, "body": body }));
        Ok((v["status"].as_u64().unwrap_or(500) as u16, v["json"].clone()))
    }
}

/// Cowork deps answered by the test: which CLI to run comes from the scenario
struct RemoteDeps {
    runner: Arc<RemoteRunner>,
    handle: u64,
    data_dir: PathBuf,
}

impl RemoteDeps {
    fn resolve(&self, kind: &str, agent: &str) -> Result<ResolvedCli, String> {
        let v = self.runner.ask(json!({ "kind": kind, "agent": agent, "handle": self.handle }));
        if let Some(e) = v.get("error").and_then(Value::as_str) {
            return Err(e.to_string());
        }
        let strs = |x: &Value| -> Vec<String> { x.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect() };
        let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let mut efforts = HashMap::new();
        for (k, list) in v.get("modelEfforts").and_then(Value::as_object).into_iter().flatten() {
            efforts.insert(k.clone(), strs(list));
        }
        Ok(ResolvedCli {
            command: text("command"),
            prefix_args: strs(&v["prefixArgs"]),
            windows_sandbox: v.get("windowsSandbox").and_then(Value::as_str).map(str::to_string),
            env: v.get("env").and_then(Value::as_object).map(|m| m.iter().map(|(k, x)| (k.clone(), x.as_str().unwrap_or("").to_string())).collect()),
            default_model: text("defaultModel"),
            default_effort: text("defaultEffort"),
            model_efforts: efforts,
        })
    }
}

impl CoworkDeps for RemoteDeps {
    fn data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }
    fn resolve_cli(&self, agent: &str) -> Result<ResolvedCli, String> {
        self.resolve("resolveCli", agent)
    }
    fn resolve_exec_cli(&self, agent: &str) -> Result<ResolvedCli, String> {
        self.resolve("resolveExecCli", agent)
    }
    fn emit(&self, run: &Value) {
        self.runner.host.print(&json!({ "event": "cowork:emit", "handle": self.handle, "run": run }));
    }
}

fn memsync_result(r: Result<Value, memsync::MemErr>) -> Value {
    match r {
        Ok(v) => json!({ "ok": true, "value": v }),
        Err(e) => json!({ "ok": false, "code": e.code, "message": e.message }),
    }
}

fn no_tools() -> Tools {
    struct None_;
    impl Runner for None_ {
        fn run(&self, _: &str, _: &[String], _: &RunOpts) -> RunResult {
            RunResult::default()
        }
        fn http_status(&self, _: &str) -> Result<u16, String> {
            Err("unused".into())
        }
        fn github_api(&self, _: &str, _: &str, _: &str, _: Option<&Value>) -> Result<(u16, Value), String> {
            Err("unused".into())
        }
    }
    Tools { git: "git".into(), gh: None, runner: Arc::new(None_) }
}

fn main() {
    // One-shot for the test's synchronous helpers: wb-ipc --memsync <name>, arguments as JSON on stdin
    let argv: Vec<String> = std::env::args().collect();
    if argv.get(1).map(String::as_str) == Some("--memsync") {
        let mut input = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut input).expect("stdin");
        let a: serde_json::Map<String, Value> = serde_json::from_str(&input).unwrap_or_default();
        println!("{}", memsync_result(memsync::test_call(&argv[2], &no_tools(), &a)));
        return;
    }
    if argv.get(1).map(String::as_str) == Some("--core") {
        let mut input = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut input).expect("stdin");
        let a: Vec<Value> = serde_json::from_str(&input).unwrap_or_default();
        println!("{}", match wb_core::test_call(&argv[2], &a) {
            Ok(v) => json!({ "value": v }),
            Err(e) => json!({ "err": e }),
        });
        return;
    }
    if argv.get(1).map(String::as_str) == Some("--cowork-shared") {
        let mut input = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut input).expect("stdin");
        let a: Vec<Value> = serde_json::from_str(&input).unwrap_or_default();
        println!("{}", match wb_core::cowork_test::shared_call(&argv[2], &a) {
            Ok(Some(v)) => json!({ "value": v }),
            Ok(None) => json!({ "undefined": true }),
            Err(e) => json!({ "err": e }),
        });
        return;
    }
    let host = Arc::new(Stub::default());
    let services: Arc<Mutex<HashMap<u64, Arc<Service>>>> = Arc::default();
    let pending: Arc<Mutex<HashMap<u64, mpsc::Sender<Value>>>> = Arc::default();
    let info = AppInfo { version: std::env::var("WB_VERSION").unwrap_or_else(|_| "0.0.0".into()), is_packaged: false, exe_path: String::new(), resources: PathBuf::from(std::env::var("WB_RESOURCES").unwrap_or_default()) };
    let core = Core::new(host.clone(), PathBuf::from(std::env::var("WB_DATA_DIR").expect("WB_DATA_DIR")), info);
    {
        let h = host.clone();
        core.pty.on_event(Arc::new(move |ev| {
            if let wb_core::pty::PtyEvent::Approval(id) = ev {
                h.print(&json!({ "event": "test:ptyEvent", "kind": "approval", "id": id }));
            }
        }));
    }
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(cb) = msg.get("cbReply").and_then(Value::as_u64) {
            if let Some(tx) = pending.lock().unwrap().remove(&cb) {
                let _ = tx.send(msg["value"].clone());
            }
            continue;
        }
        let (core, host, pending, services) = (core.clone(), host.clone(), pending.clone(), services.clone());
        // Calls run concurrently, as they do from a real window
        std::thread::spawn(move || {
            let id = msg["id"].clone();
            let window = msg["window"].as_u64().unwrap_or(1) as u32;
            let channel = msg["channel"].as_str().unwrap_or("").to_string();
            let args = msg["args"].as_array().cloned().unwrap_or_default();
            let res = match channel.as_str() {
                "test:openWindow" => {
                    host.windows.lock().unwrap().push(window);
                    Ok(json!(core.register_window(window, args.first().and_then(Value::as_str))))
                }
                "test:closeWindow" => {
                    host.windows.lock().unwrap().retain(|w| *w != window);
                    core.window_closed(window);
                    Ok(Value::Null)
                }
                "test:memsync" => {
                    let a = args.get(1).and_then(Value::as_object).cloned().unwrap_or_default();
                    let runner = Arc::new(RemoteRunner { host: host.clone(), pending, next: AtomicU64::new(1), call: id.as_u64().unwrap_or(0) });
                    let name = args.first().and_then(Value::as_str).unwrap_or("");
                    if name == "applyBundledPlugin" {
                        let homes = memsync::Homes { claude: PathBuf::from(a["homes"]["claude"].as_str().unwrap_or("")), codex: PathBuf::from(a["homes"]["codex"].as_str().unwrap_or("")), antigravity: PathBuf::from(a["homes"]["antigravity"].as_str().unwrap_or("")) };
                        let exec = |cli_args: &[String]| RemoteRunner::run_value(runner.ask(json!({ "kind": "cli", "args": cli_args })));
                        Ok(memsync_result(memsync::apply_bundled_plugin(a["agent"].as_str().unwrap_or(""), &exec, std::path::Path::new(a["marketplaceDir"].as_str().unwrap_or("")), &homes).map(|l| json!(l))))
                    } else {
                        let tools = Tools { git: a.get("git").and_then(Value::as_str).unwrap_or("git").to_string(), gh: a.get("gh").and_then(Value::as_str).map(str::to_string), runner: runner.clone() };
                        Ok(memsync_result(memsync::test_call(name, &tools, &a)))
                    }
                }
                "test:coworkNew" => {
                    let handle = args[0].as_u64().unwrap_or(0);
                    let runner = Arc::new(RemoteRunner { host: host.clone(), pending, next: AtomicU64::new(1), call: handle + 1_000_000 });
                    let deps = Arc::new(RemoteDeps { runner, handle, data_dir: PathBuf::from(args[1].as_str().unwrap_or("")) });
                    services.lock().unwrap().insert(handle, Service::new(deps));
                    Ok(Value::Null)
                }
                "test:coworkCall" => {
                    let svc = services.lock().unwrap().get(&args[0].as_u64().unwrap_or(0)).cloned();
                    let list = args[2].as_array().cloned().unwrap_or_default();
                    Ok(match svc.map(|s| wb_core::cowork_test::service_call(&s, args[1].as_str().unwrap_or(""), &list)) {
                        Some(Ok(v)) => json!({ "ok": true, "value": v }),
                        Some(Err(e)) => json!({ "ok": false, "code": e.code, "message": e.message, "data": e.data }),
                        None => json!({ "ok": false, "code": "no-service" }),
                    })
                }
                "test:ptyInfo" => Ok(core.get_pty_session(args[0].as_str().unwrap_or("")).unwrap_or(Value::Null)),
                "test:ptyScreen" => Ok(json!(core.pty_screen_text(args[0].as_str().unwrap_or("")).unwrap_or_default())),
                "test:quit" => {
                    core.shutdown();
                    std::process::exit(0);
                }
                _ => core.invoke(window, &channel, &args),
            };
            host.print(&match res {
                Ok(v) => json!({ "id": id, "result": v }),
                Err(e) => json!({ "id": id, "error": e }),
            });
        });
    }
    core.shutdown();
    std::process::exit(0);
}
