// End-to-end checks drive the real pages through this. Compiled into debug and e2e builds only, because it is full control of the app.
// Line protocol on 127.0.0.1:$WB_E2E_PORT: {"id":1,"label":"w1","js":"<expression>"} -> {"id":1,"ok":true,"value":...}
// and {"id":1,"list":true} -> {"id":1,"ok":true,"value":["w1",...]}
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

pub const INIT: &str = "window.__wbErrors = []; addEventListener('error', (e) => __wbErrors.push(String(e.message))); addEventListener('unhandledrejection', (e) => __wbErrors.push(String(e.reason && e.reason.message || e.reason)));";

type Waiters = Mutex<HashMap<u64, Arc<Mutex<TcpStream>>>>;

fn waiters() -> &'static Waiters {
    static W: OnceLock<Waiters> = OnceLock::new();
    W.get_or_init(Mutex::default)
}

#[tauri::command]
pub fn e2e_result(id: u64, ok: bool, value: Value) {
    if let Some(out) = waiters().lock().unwrap().remove(&id) {
        let _ = writeln!(out.lock().unwrap(), "{}", json!({ "id": id, "ok": ok, "value": value }));
    }
}

pub fn serve(app: AppHandle) {
    let Ok(port) = std::env::var("WB_E2E_PORT") else { return };
    let Ok(listener) = TcpListener::bind(format!("127.0.0.1:{port}")) else { return };
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let app = app.clone();
            std::thread::spawn(move || {
                let Ok(clone) = stream.try_clone() else { return };
                let out = Arc::new(Mutex::new(clone));
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let Ok(m) = serde_json::from_str::<Value>(&line) else { continue };
                    let id = m["id"].as_u64().unwrap_or(0);
                    if m.get("list").is_some() {
                        let labels: Vec<String> = app.windows().keys().cloned().collect();
                        let _ = writeln!(out.lock().unwrap(), "{}", json!({ "id": id, "ok": true, "value": labels }));
                        continue;
                    }
                    // The page's own webview: once a window also holds a child webview, tauri no longer calls it a "webview window"
                    let Some(w) = app.get_webview(m["label"].as_str().unwrap_or("")) else {
                        let _ = writeln!(out.lock().unwrap(), "{}", json!({ "id": id, "ok": false, "value": "no such window" }));
                        continue;
                    };
                    waiters().lock().unwrap().insert(id, out.clone());
                    let js = m["js"].as_str().unwrap_or("null");
                    let wrapped = format!(
                        "(async () => {{ const send = (ok, value) => window.__TAURI__.core.invoke('e2e_result', {{ id: {id}, ok, value: value === undefined ? null : value }}); try {{ send(true, await ({js})) }} catch (e) {{ send(false, String(e && e.message || e)) }} }})()"
                    );
                    let _ = w.eval(&wrapped);
                }
            });
        }
    });
}
