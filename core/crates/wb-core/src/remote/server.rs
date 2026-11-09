// Remote Bridge: lets a phone on the same LAN operate the desktop's terminals.
//
//   https://<lan>:<port>    phone app, WebSocket (/ws), pairing and push APIs
//   http://<lan>:<port+1>   only the "install the CA" page; everything else goes to https
//   https://<lan>:<port+2>  phone preview proxy (preview.rs)
//
// Only private-network peers get past TCP accept, even if someone forwards the port on their router; with mesh VPN on, also peers
// that really came in through the mesh adapter (certs::is_mesh_connection).
//
// The phone can switch between computers within one page, so the WebSocket is cross-origin. The Origin check lets through the same
// app on the LAN, and pairing also exists over WebSocket since a cross-origin fetch would be blocked by CORS.
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::future::Future;
use std::net::{SocketAddr, TcpListener as StdListener};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full, Limited, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::header::{HeaderValue, AUTHORIZATION, CONNECTION, HOST, ORIGIN, RANGE, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY, UPGRADE};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::{Handle, Runtime};
use tokio::sync::{mpsc, Notify};
use tokio_rustls::rustls;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role, WebSocketConfig};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use url::Url;

use super::certs::{self, CertPair};
use super::files::{self, RangeOut, RemoteFiles};
use super::preview::Preview;
use super::store::{ct_eq, StoredCerts};
use super::webpush::{self, VapidKeys};
use crate::core::Core;
use crate::pty::{PtyEvent, PtySub};
use crate::util::{basename, js_len, js_prefix, now_ms, resolve, sv, uuid};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub type Body = http_body_util::combinators::BoxBody<Bytes, BoxError>;

const PAIR_TTL_MS: f64 = 5.0 * 60.0 * 1000.0;
const PAIR_MAX_FAILURES: u32 = 10;
const PAIR_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789"; // without 0/O and 1/I/L
const PUSH_SUBJECT: &str = "https://github.com/zkylek1212-k/Multi-Harness-ADE";
const MAX_BACKLOG: usize = 8 * 1024 * 1024;
pub const PHONE_COWORK_OPS: [&str; 29] = ["capabilities", "models", "list", "get", "start", "cancel", "discuss", "summarizer", "summarize", "retry", "drop", "note", "feedback", "dismiss", "approve", "raiseLimits", "delete", "execPreview", "execDiff", "execStart", "execMessage", "execRetry", "execAcceptScope", "execRevertScope", "execWaive", "execPause", "execResume", "execMerge", "execCleanup"];
pub const PHONE_AUDITED_OPS: [&str; 8] = ["approve", "execStart", "execAcceptScope", "execRevertScope", "execWaive", "execMerge", "execCleanup", "delete"];
const BUILTIN: [(&str, &str, &str); 5] = [("claude", "Claude Code", "agent"), ("codex", "Codex", "agent"), ("antigravity", "Antigravity", "agent"), ("powershell", "PowerShell", "shell"), ("cmd", "Command Prompt", "shell")];

// A preview iframe is on another port, maybe another computer, so frame-src cannot be just 'self'. A framed page reaches nothing here.
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; connect-src 'self' wss:; worker-src 'self'; manifest-src 'self'; frame-src 'self' https:; frame-ancestors 'none'; base-uri 'none'";

pub fn full(b: impl Into<Bytes>) -> Body {
    Full::new(b.into()).map_err(|e: Infallible| match e {}).boxed()
}

pub fn text(status: StatusCode, msg: &str) -> Response<Body> {
    Response::builder().status(status).header("Content-Type", "text/plain; charset=utf-8").header("Cache-Control", "no-store").body(full(msg.to_string())).unwrap()
}

fn send_json(status: StatusCode, v: Value) -> Response<Body> {
    Response::builder().status(status).header("Content-Type", "application/json; charset=utf-8").header("Cache-Control", "no-store").body(full(v.to_string())).unwrap()
}

fn plain(status: StatusCode) -> Response<Body> {
    Response::builder().status(status).body(full(Bytes::new())).unwrap()
}

/// Sent only to known push services, so a subscription cannot make the desktop request an arbitrary address
pub fn is_push_endpoint(endpoint: &str) -> bool {
    let Ok(u) = Url::parse(endpoint) else { return false };
    let Some(h) = u.host_str() else { return false };
    u.scheme() == "https" && (h == "web.push.apple.com" || h == "fcm.googleapis.com" || h == "push.services.mozilla.com" || h.ends_with(".push.services.mozilla.com") || h.ends_with(".notify.windows.com"))
}

/// Only two shapes of path a phone ever fetches. Each segment must be word plus extension, so `..` and `.` cannot pass.
pub fn static_rel(p: &str) -> Option<String> {
    use std::sync::OnceLock;
    static ASSET: OnceLock<regex::Regex> = OnceLock::new();
    static PUBLIC: OnceLock<regex::Regex> = OnceLock::new();
    if p == "/" || p == "/index.html" {
        return Some("remote.html".into());
    }
    let asset = ASSET.get_or_init(|| regex::Regex::new(r"^/assets/[\w-]+(?:\.[\w-]+)+$").unwrap());
    let public = PUBLIC.get_or_init(|| regex::Regex::new(r"^/remote[-.][\w-]+(?:\.[\w-]+)*$").unwrap());
    (asset.is_match(p) || public.is_match(p)).then(|| p[1..].to_string())
}

fn mime_static(rel: &str) -> &'static str {
    match rel.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "webmanifest" => "application/manifest+json",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        _ => "application/octet-stream",
    }
}

fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    js_prefix(&lines[lines.len().saturating_sub(n)..].join(" · "), 160)
}

fn js_suffix(s: &str, n: usize) -> String {
    let units: Vec<u16> = s.encode_utf16().collect();
    String::from_utf16_lossy(&units[units.len().saturating_sub(n)..])
}

enum Out {
    Text(String),
    Close(u16),
}

struct Client {
    conn_id: String,
    tx: mpsc::UnboundedSender<Out>,
    backlog: AtomicUsize,
    device: Mutex<Option<Value>>,
    attached: Mutex<HashSet<String>>,
    visible: AtomicBool,
    /// The address the phone reached us on: a preview URL has to use the same one, not a guess between adapters
    host: String,
    kill: Notify,
    handle: Handle,
}

impl Client {
    fn device_id(&self) -> Option<String> {
        self.device.lock().unwrap().as_ref().map(|d| sv(d, "id").to_string())
    }

    fn authed(&self) -> bool {
        self.device.lock().unwrap().is_some()
    }

    fn send(self: &Arc<Self>, msg: &Value) {
        // A stuck phone past 8 MB gets dropped; it reconnects and takes a fresh snapshot rather than eating all the memory
        if self.backlog.load(Ordering::SeqCst) > MAX_BACKLOG {
            return self.drop_with(1013);
        }
        let s = msg.to_string();
        self.backlog.fetch_add(s.len(), Ordering::SeqCst);
        let _ = self.tx.send(Out::Text(s));
    }

    /// Asks nicely, then ends it: a socket that stopped draining would never get to the close frame
    fn drop_with(self: &Arc<Self>, code: u16) {
        let _ = self.tx.send(Out::Close(code));
        let me = self.clone();
        self.handle.spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            me.kill.notify_one();
        });
    }
}

/// Holds output back until the snapshot is out, so a chunk from another thread cannot overtake it and be erased by it
struct RemoteSub {
    client: Weak<Client>,
    pending: Mutex<Option<Vec<Value>>>,
}

impl RemoteSub {
    fn out(&self, msg: Value) {
        let mut p = self.pending.lock().unwrap();
        if let Some(q) = p.as_mut() {
            q.push(msg);
        } else if let Some(c) = self.client.upgrade() {
            c.send(&msg);
        }
    }
}

impl PtySub for RemoteSub {
    fn data(&self, id: &str, chunk: &str) {
        self.out(json!({ "t": "data", "id": id, "d": chunk }));
    }
    fn exit(&self, id: &str, _code: i32) {
        if let Some(c) = self.client.upgrade() {
            c.attached.lock().unwrap().remove(id);
        }
    }
    fn resized(&self, id: &str, cols: u16, rows: u16) {
        self.out(json!({ "t": "resized", "id": id, "cols": cols, "rows": rows }));
    }
}

struct Pairing {
    code: String,
    expires_at: f64,
    failures: u32,
}

#[derive(Default)]
struct State {
    port: u16,
    addresses: Vec<String>,
    mesh_addrs: Vec<String>,
    allow_mesh: bool,
    ca_cert: Option<String>,
    vapid: Option<VapidKeys>,
    pairing: Option<Pairing>,
    last_error: Option<String>,
}

pub struct BridgeStatus {
    pub running: bool,
    pub addresses: Vec<String>,
    pub mesh_detected: Vec<String>,
    pub mesh_needs_new_ca: bool,
    pub ca_fingerprint: Option<String>,
    pub connected_devices: Vec<String>,
    pub connected_device_ids: Vec<String>,
    pub error: Option<String>,
}

pub struct Bridge {
    core: Weak<Core>,
    me: Weak<Bridge>,
    /// One start or stop at a time
    ops: Mutex<()>,
    rt: Mutex<Option<Runtime>>,
    handle: Mutex<Option<Handle>>,
    st: Mutex<State>,
    clients: Mutex<HashMap<String, Arc<Client>>>,
    state_due: AtomicBool,
    push_throttle: Mutex<HashMap<String, f64>>,
    vibe: Mutex<HashMap<u32, Value>>,
    pub(super) preview: Arc<Preview>,
    files: RemoteFiles,
}

type HandlerFuture = Pin<Box<dyn Future<Output = Response<Body>> + Send>>;

/// The TCP close is left to the peer. Sent right behind TLS's close_notify, a FIN got about 1% of responses reset before the client
/// had read them (measured on Windows, 0 of 600 without). The reader below ends it if the peer never closes.
struct Linger(Option<TcpStream>);

impl Drop for Linger {
    fn drop(&mut self) {
        let (Some(mut s), Ok(handle)) = (self.0.take(), Handle::try_current()) else { return };
        handle.spawn(async move {
            let mut sink = [0u8; 1024];
            let _ = tokio::time::timeout(Duration::from_secs(2), async {
                while matches!(s.read(&mut sink).await, Ok(n) if n > 0) {}
            })
            .await;
        });
    }
}

impl tokio::io::AsyncRead for Linger {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &mut tokio::io::ReadBuf<'_>) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(self.0.as_mut().unwrap()).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for Linger {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &[u8]) -> std::task::Poll<std::io::Result<usize>> {
        Pin::new(self.0.as_mut().unwrap()).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(self.0.as_mut().unwrap()).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        // The peer closes after it has read everything; Drop below ends it if the peer never does
        std::task::Poll::Ready(Ok(()))
    }
}

impl Bridge {
    pub fn new(core: &Arc<Core>) -> Arc<Self> {
        let weak = Arc::downgrade(core);
        let for_preview = weak.clone();
        let bridge = Arc::new_cyclic(|me: &Weak<Bridge>| Self {
            core: weak,
            me: me.clone(),
            ops: Mutex::new(()),
            rt: Mutex::new(None),
            handle: Mutex::new(None),
            st: Mutex::default(),
            clients: Mutex::default(),
            state_due: AtomicBool::new(false),
            push_throttle: Mutex::default(),
            vibe: Mutex::default(),
            // Only ports a terminal really printed, and only ever on 127.0.0.1
            preview: Arc::new(Preview::new(Box::new(move |p| for_preview.upgrade().is_some_and(|c| c.list_pty_sessions().iter().any(|s| s["devPort"].as_u64() == Some(p as u64)))))),
            files: RemoteFiles::default(),
        });
        let me = Arc::downgrade(&bridge);
        core.pty.on_event(Arc::new(move |ev| {
            if let Some(b) = me.upgrade() {
                b.on_pty_event(ev);
            }
        }));
        bridge
    }

    fn core(&self) -> Option<Arc<Core>> {
        self.core.upgrade()
    }

    fn store(&self) -> Option<Arc<super::store::Store>> {
        self.core().map(|c| c.features().remote.store.clone())
    }

    fn audit(&self, event: &str, detail: Value) {
        if let Some(s) = self.store() {
            s.audit(event, detail);
        }
    }

    fn notify_status(&self) {
        if let Some(core) = self.core() {
            super::broadcast_status(&core);
        }
    }

    fn guard(&self, peer: &SocketAddr, local: &SocketAddr) -> bool {
        let remote = peer.ip().to_string();
        certs::is_allowed_remote(&remote) || certs::is_mesh_connection(&remote, &local.ip().to_string(), &self.st.lock().unwrap().mesh_addrs)
    }

    pub fn running(&self) -> bool {
        self.rt.lock().unwrap().is_some()
    }

    pub fn start(self: &Arc<Self>, port: u16, allow_mesh: bool) -> Result<(), String> {
        let _op = self.ops.lock().unwrap();
        self.stop_locked();
        {
            let mut st = self.st.lock().unwrap();
            st.port = port;
            st.allow_mesh = allow_mesh;
            st.last_error = None;
        }
        let res = self.start_locked(port);
        if let Err(e) = &res {
            self.stop_locked();
            self.st.lock().unwrap().last_error = Some(if e.is_empty() { "failed to start".into() } else { e.clone() });
        }
        self.notify_status();
        res
    }

    fn start_locked(self: &Arc<Self>, port: u16) -> Result<(), String> {
        let store = self.store().ok_or("no core")?;
        let pair = self.ensure_certs()?;
        let vapid = match store.load_vapid()? {
            Some(v) => v,
            None => {
                let v = webpush::generate_vapid_keys()?;
                store.save_vapid(&v)?;
                v
            }
        };
        self.st.lock().unwrap().vapid = Some(vapid);

        let tls = tls_acceptor(&pair)?;
        let main = StdListener::bind(("0.0.0.0", port)).map_err(|e| e.to_string())?;
        let setup = StdListener::bind(("0.0.0.0", port + 1)).map_err(|e| e.to_string())?;
        // Without the preview port there is just no preview; remote control carries on
        let preview = StdListener::bind(("0.0.0.0", port + 2));
        for l in [Some(&main), Some(&setup), preview.as_ref().ok()].into_iter().flatten() {
            l.set_nonblocking(true).map_err(|e| e.to_string())?;
        }

        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().thread_name("wb-remote").build().map_err(|e| e.to_string())?;
        *self.handle.lock().unwrap() = Some(rt.handle().clone());
        let me = self.clone();
        let tls_main = tls.clone();
        rt.spawn(me.clone().accept_loop(main, Some(tls_main), {
            let me = me.clone();
            Arc::new(move |req, peer| -> HandlerFuture {
                let me = me.clone();
                Box::pin(async move { me.handle_https(req, peer).await })
            })
        }));
        rt.spawn(me.clone().accept_loop(setup, None, {
            let me = me.clone();
            Arc::new(move |req, _| -> HandlerFuture {
                let me = me.clone();
                Box::pin(async move { me.handle_setup(req) })
            })
        }));
        match preview {
            Ok(l) => {
                self.preview.started(port + 2);
                let p = self.preview.clone();
                rt.spawn(me.clone().accept_loop(l, Some(tls), Arc::new(move |req, _| -> HandlerFuture { Box::pin(p.clone().handle(req)) })));
            }
            Err(e) => eprintln!("[Remote] preview proxy failed to start: {e}"),
        }
        *self.rt.lock().unwrap() = Some(rt);
        store.audit("bridge.start", json!({ "port": port, "addresses": self.st.lock().unwrap().addresses }));
        Ok(())
    }

    pub fn stop(&self) {
        let _op = self.ops.lock().unwrap();
        self.stop_locked();
        self.notify_status();
    }

    fn stop_locked(&self) {
        let clients: Vec<Arc<Client>> = self.clients.lock().unwrap().values().cloned().collect();
        for c in &clients {
            let _ = c.tx.send(Out::Close(1001));
        }
        let rt = self.rt.lock().unwrap().take();
        *self.handle.lock().unwrap() = None;
        if let Some(rt) = rt {
            if !clients.is_empty() {
                std::thread::sleep(Duration::from_millis(100));
            }
            // A runtime cannot be dropped from inside another runtime's context, and this may be running on the host's
            let _ = std::thread::spawn(move || rt.shutdown_timeout(Duration::from_secs(1))).join();
        }
        for c in &clients {
            self.client_gone(c);
        }
        self.st.lock().unwrap().pairing = None;
        self.files.clear();
        self.preview.stopped();
    }

    async fn accept_loop(self: Arc<Self>, listener: StdListener, tls: Option<TlsAcceptor>, handler: Arc<dyn Fn(Request<Incoming>, SocketAddr) -> HandlerFuture + Send + Sync>) {
        let Ok(listener) = TcpListener::from_std(listener) else { return };
        loop {
            let Ok((tcp, peer)) = listener.accept().await else {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            };
            let Ok(local) = tcp.local_addr() else { continue };
            if !self.guard(&peer, &local) {
                continue; // dropping it closes the connection
            }
            let _ = tcp.set_nodelay(true);
            let (tls, handler) = (tls.clone(), handler.clone());
            let tcp = Linger(Some(tcp));
            tokio::spawn(async move {
                let service = service_fn(move |req| {
                    let f = handler(req, peer);
                    async move { Ok::<_, Infallible>(f.await) }
                });
                let builder = || {
                    let mut b = hyper::server::conn::http1::Builder::new();
                    b.timer(TokioTimer::new()).header_read_timeout(Duration::from_secs(30));
                    b
                };
                match tls {
                    Some(tls) => {
                        let Ok(Ok(stream)) = tokio::time::timeout(Duration::from_secs(10), tls.accept(tcp)).await else { return };
                        let _ = builder().serve_connection(TokioIo::new(stream), service).with_upgrades().await;
                    }
                    None => {
                        let _ = builder().serve_connection(TokioIo::new(tcp), service).await;
                    }
                }
            });
        }
    }

    pub fn status(&self) -> BridgeStatus {
        let (addresses, error) = {
            let st = self.st.lock().unwrap();
            (st.addresses.clone(), st.last_error.clone())
        };
        let running = self.running();
        let pem = self.st.lock().unwrap().ca_cert.clone().or_else(|| self.store().and_then(|s| s.ca_cert()));
        let (mut names, mut ids) = (Vec::new(), Vec::new());
        for c in self.authed_clients() {
            if let Some(d) = c.device.lock().unwrap().as_ref() {
                let (name, id) = (sv(d, "name").to_string(), sv(d, "id").to_string());
                if !names.contains(&name) {
                    names.push(name);
                }
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        BridgeStatus {
            running,
            addresses: if running { addresses } else { certs::lan_addresses() },
            mesh_detected: certs::mesh_addresses(),
            mesh_needs_new_ca: pem.as_ref().is_none_or(|p| !certs::ca_permits_mesh(p)),
            ca_fingerprint: pem.as_ref().and_then(|p| certs::fingerprint256(p)),
            connected_devices: names,
            connected_device_ids: ids,
            error,
        }
    }

    fn authed_clients(&self) -> Vec<Arc<Client>> {
        self.clients.lock().unwrap().values().filter(|c| c.authed()).cloned().collect()
    }

    /// The CA stays so phones need no reinstall; the server certificate is re-issued when addresses change or it nears expiry.
    /// The one exception is mesh VPN with an old CA that may not sign that range: only a new CA can, and phones reinstall once.
    fn ensure_certs(&self) -> Result<CertPair, String> {
        let store = self.store().ok_or("no core")?;
        let allow_mesh = self.st.lock().unwrap().allow_mesh;
        let mesh_addrs = if allow_mesh { certs::mesh_addresses() } else { vec![] };
        let mut addresses = certs::lan_addresses();
        addresses.extend(mesh_addrs.iter().cloned());
        let dns: Vec<String> = certs::local_hostname().into_iter().collect();
        let mesh = !mesh_addrs.is_empty();
        let mut stored = store.load_certs()?;
        if stored.as_ref().is_none_or(|s| mesh && !certs::ca_permits_mesh(&s.ca_cert)) {
            // A replaced CA is kept: turning mesh off and on again needs no second install
            let label: String = certs::os_hostname().chars().take(40).collect();
            let ca = certs::create_ca(&label, mesh)?;
            store.audit(if stored.is_some() { "ca.replaced.mesh" } else { "ca.created" }, json!({}));
            stored = Some(StoredCerts { ca_cert: ca.cert_pem, ca_key: ca.key_pem, server_cert: None, server_key: None });
        }
        let mut stored = stored.unwrap();
        // No 127.0.0.1: the CA's constraints only allow private ranges, and a phone never connects to loopback
        let covered = stored.server_cert.as_ref().is_some_and(|c| stored.server_key.is_some() && certs::cert_covers(c, &addresses, &dns));
        if !covered {
            let leaf = certs::issue_server_cert(&CertPair { cert_pem: stored.ca_cert.clone(), key_pem: stored.ca_key.clone() }, &addresses, &dns)?;
            stored.server_cert = Some(leaf.cert_pem);
            stored.server_key = Some(leaf.key_pem);
        }
        store.save_certs(&stored)?;
        let mut st = self.st.lock().unwrap();
        st.mesh_addrs = mesh_addrs;
        st.addresses = addresses;
        st.ca_cert = Some(stored.ca_cert.clone());
        Ok(CertPair { cert_pem: stored.server_cert.unwrap(), key_pem: stored.server_key.unwrap() })
    }

    // Pairing

    pub fn create_pairing_code(&self) -> (String, f64) {
        let bytes = super::random_bytes(8);
        let code: String = bytes.iter().map(|b| PAIR_ALPHABET[*b as usize % PAIR_ALPHABET.len()] as char).collect();
        let expires_at = now_ms() + PAIR_TTL_MS;
        self.st.lock().unwrap().pairing = Some(Pairing { code: code.clone(), expires_at, failures: 0 });
        (code, expires_at)
    }

    fn try_pair(&self, raw_code: &str, name: &str) -> Option<(String, String)> {
        let store = self.store()?;
        {
            let mut st = self.st.lock().unwrap();
            let p = st.pairing.as_mut()?;
            if now_ms() > p.expires_at {
                return None;
            }
            let code: String = raw_code.to_uppercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            if !ct_eq(code.as_bytes(), p.code.as_bytes()) {
                p.failures += 1;
                let failures = p.failures;
                if failures >= PAIR_MAX_FAILURES {
                    st.pairing = None;
                }
                drop(st);
                store.audit("pair.fail", json!({ "failures": failures }));
                return None;
            }
            st.pairing = None; // one use
        }
        let (device, token) = store.add_device(if name.is_empty() { "iPhone" } else { name });
        store.audit("pair.ok", json!({ "deviceId": device["id"], "name": device["name"] }));
        if let Some(core) = self.core() {
            core.host.notify("Remote control", &format!("「{}」已配對，可以遠端操作終端", sv(&device, "name")));
        }
        self.notify_status();
        Some((token, sv(&device, "id").to_string()))
    }

    pub fn revoke_device(&self, id: &str) {
        let Some(store) = self.store() else { return };
        store.remove_device(id);
        // A preview cookie is not tied to a device, so the only way to end a revoked phone's is to end them all; others reopen Preview
        self.preview.rotate();
        for c in self.clients.lock().unwrap().values() {
            if c.device_id().as_deref() == Some(id) {
                c.drop_with(4001);
            }
        }
        store.audit("device.revoke", json!({ "deviceId": id }));
        self.notify_status();
    }

    // HTTP

    fn auth_device(&self, req: &Request<Incoming>) -> Option<Value> {
        let token = req.headers().get(AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ")?;
        self.store()?.find_device_by_token(token)
    }

    async fn read_json(body: Incoming) -> Option<Value> {
        let collected = tokio::time::timeout(Duration::from_secs(30), Limited::new(body, 64 * 1024).collect()).await.ok()?.ok()?;
        let bytes = collected.to_bytes();
        if bytes.is_empty() {
            return Some(json!({}));
        }
        serde_json::from_slice(&bytes).ok()
    }

    async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
        tokio::task::spawn_blocking(f).await.ok()
    }

    async fn handle_https(self: Arc<Self>, req: Request<Incoming>, _peer: SocketAddr) -> Response<Body> {
        if req.headers().contains_key(UPGRADE) {
            return self.handle_upgrade(req);
        }
        let p = req.uri().path().to_string();
        if p.starts_with("/_files/") {
            return self.serve_file(req).await;
        }
        let internal = || send_json(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "internal" }));
        if p == "/api/pair" && req.method() == Method::POST {
            let Some(body) = Self::read_json(req.into_body()).await else { return internal() };
            let me = self.clone();
            let result = Self::blocking(move || me.try_pair(body["code"].as_str().unwrap_or(""), body["name"].as_str().unwrap_or(""))).await.flatten();
            return match result {
                Some((token, device_id)) => send_json(StatusCode::OK, json!({ "token": token, "deviceId": device_id })),
                None => send_json(StatusCode::FORBIDDEN, json!({ "error": "invalid or expired code" })),
            };
        }
        if p == "/api/vapid" && req.method() == Method::GET {
            if self.auth_device(&req).is_none() {
                return send_json(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }));
            }
            let key = self.st.lock().unwrap().vapid.as_ref().map(|v| v.public_key.clone());
            return send_json(StatusCode::OK, json!({ "publicKey": key }));
        }
        if p == "/api/push" && req.method() == Method::POST {
            let Some(device) = self.auth_device(&req) else { return send_json(StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" })) };
            let Some(body) = Self::read_json(req.into_body()).await else { return internal() };
            let sub = body.get("subscription").filter(|s| !s.is_null()).cloned();
            if let Some(s) = &sub {
                if !is_push_endpoint(sv(s, "endpoint")) || sv(&s["keys"], "p256dh").is_empty() || sv(&s["keys"], "auth").is_empty() {
                    return send_json(StatusCode::BAD_REQUEST, json!({ "error": "bad subscription" }));
                }
            }
            let me = self.clone();
            let test = body.get("test").is_some_and(crate::settings::truthy);
            let id = sv(&device, "id").to_string();
            let _ = Self::blocking(move || {
                let Some(store) = me.store() else { return };
                store.update_device(&id, "push", sub.clone().unwrap_or(Value::Null));
                store.audit(if sub.is_some() { "push.subscribe" } else { "push.unsubscribe" }, json!({ "deviceId": id }));
                let vapid = me.st.lock().unwrap().vapid.clone();
                if let (Some(s), true, Some(v)) = (&sub, test, vapid) {
                    webpush::send_push(s, &json!({ "title": "Agent Workbench", "body": "通知已開啟 ✓", "tag": "test" }), &v, PUSH_SUBJECT);
                }
                me.notify_status();
            })
            .await;
            return send_json(StatusCode::OK, json!({ "ok": true }));
        }
        if p.starts_with("/api/") {
            return send_json(StatusCode::NOT_FOUND, json!({ "error": "not found" }));
        }
        self.serve_static(&p)
    }

    fn serve_static(&self, p: &str) -> Response<Body> {
        let is_root = p == "/" || p == "/index.html";
        let found = static_rel(p).and_then(|rel| self.core().and_then(|c| c.host.static_asset(&rel)).map(|b| (rel, b)));
        let Some((rel, bytes)) = found else {
            return if is_root { text(StatusCode::SERVICE_UNAVAILABLE, "Mobile client not built. Run `npm run build` once, then restart the app.") } else { plain(StatusCode::NOT_FOUND) };
        };
        // Vite's assets have a hash in the name and can be cached for good; html, service worker and manifest are always rechecked
        let mut r = Response::builder().status(StatusCode::OK).header("Content-Type", mime_static(&rel)).header("X-Content-Type-Options", "nosniff").header("Referrer-Policy", "no-referrer").header("Content-Security-Policy", CSP).header("Cache-Control", if rel.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" });
        if rel == "remote-sw.js" {
            r = r.header("Service-Worker-Allowed", "/");
        }
        r.body(full(bytes)).unwrap()
    }

    async fn serve_file(self: Arc<Self>, req: Request<Incoming>) -> Response<Body> {
        if req.method() != Method::GET && req.method() != Method::HEAD {
            return plain(StatusCode::METHOD_NOT_ALLOWED);
        }
        let parts: Vec<&str> = req.uri().path().split('/').collect();
        let forbidden = || plain(StatusCode::FORBIDDEN);
        let Some(grant) = parts.get(2).and_then(|k| self.files.grant(k)) else { return forbidden() };
        let authorized = {
            let me = self.clone();
            let g = grant.clone();
            Self::blocking(move || me.core().zip(me.store()).is_some_and(|(c, s)| s.load_devices().iter().any(|d| sv(d, "id") == g.device_id) && c.project_workspaces().iter().any(|(_, ws)| *ws == g.root))).await.unwrap_or(false)
        };
        if !authorized {
            return forbidden();
        }
        let not_found = || plain(StatusCode::NOT_FOUND);
        let mut segments = Vec::new();
        for s in &parts[3..] {
            let Ok(d) = percent_encoding::percent_decode_str(s).decode_utf8() else { return not_found() };
            segments.push(d.into_owned());
        }
        let Ok(file) = files::workspace_path(&grant.root, &segments.join("/")) else { return not_found() };
        let Ok(meta) = tokio::fs::metadata(&file).await else { return not_found() };
        if !meta.is_file() {
            return not_found();
        }
        let size = meta.len();
        let mut r = Response::builder().header("Content-Type", files::mime_of(&file)).header("Cache-Control", "no-store").header("Referrer-Policy", "no-referrer").header("X-Content-Type-Options", "nosniff").header("Access-Control-Allow-Origin", "*").header("Accept-Ranges", "bytes").header("Content-Security-Policy", files::csp_of(&file));
        let mut span = (0u64, size);
        let mut status = StatusCode::OK;
        if let Some(range) = req.headers().get(RANGE) {
            match range.to_str().ok().and_then(|h| files::parse_range(h, size)) {
                None => return not_found(),
                Some(RangeOut::Unsatisfiable) => return Response::builder().status(StatusCode::RANGE_NOT_SATISFIABLE).header("Content-Range", format!("bytes */{size}")).body(full(Bytes::new())).unwrap(),
                Some(RangeOut::Whole) => {}
                Some(RangeOut::Part(a, b)) => {
                    r = r.header("Content-Range", format!("bytes {a}-{b}/{size}"));
                    span = (a, b - a + 1);
                    status = StatusCode::PARTIAL_CONTENT;
                }
            }
        }
        r = r.status(status).header("Content-Length", span.1);
        if req.method() == Method::HEAD || size == 0 {
            return r.body(full(Bytes::new())).unwrap();
        }
        let Ok(mut f) = tokio::fs::File::open(&file).await else { return not_found() };
        if f.seek(std::io::SeekFrom::Start(span.0)).await.is_err() {
            return not_found();
        }
        let stream = futures_util::stream::unfold((f, span.1), |(mut f, left)| async move {
            if left == 0 {
                return None;
            }
            let mut buf = vec![0u8; (64 * 1024).min(left as usize)];
            match f.read(&mut buf).await {
                Ok(0) => None,
                Ok(n) => {
                    buf.truncate(n);
                    Some((Ok::<_, BoxError>(Frame::data(Bytes::from(buf))), (f, left - n as u64)))
                }
                Err(e) => Some((Err(Box::new(e) as BoxError), (f, 0))),
            }
        });
        r.body(BodyExt::boxed(StreamBody::new(stream))).unwrap()
    }

    /// The http page: the only one an iPhone can open before it trusts the CA
    fn handle_setup(&self, req: Request<Incoming>) -> Response<Body> {
        let p = req.uri().path();
        let host = req.headers().get(HOST).and_then(|h| h.to_str().ok()).unwrap_or("").split(':').next().unwrap_or("").to_string();
        let (port, ca) = {
            let st = self.st.lock().unwrap();
            (st.port, st.ca_cert.clone())
        };
        let app_url = format!("https://{host}:{port}/");
        if p == "/ca.crt" {
            if let Some(der) = ca.as_deref().and_then(|c| certs::cert_der(c).ok()) {
                return Response::builder().status(StatusCode::OK).header("Content-Type", "application/x-x509-ca-cert").header("Content-Disposition", "attachment; filename=\"AgentWorkbench-LocalCA.crt\"").header("Cache-Control", "no-store").body(full(der)).unwrap();
            }
        }
        if p != "/" {
            return match HeaderValue::from_str(&app_url) {
                Ok(v) => Response::builder().status(StatusCode::FOUND).header("Location", v).body(full(Bytes::new())).unwrap(),
                Err(_) => plain(StatusCode::BAD_REQUEST),
            };
        }
        let fp = ca.as_deref().and_then(certs::fingerprint256).unwrap_or_default();
        Response::builder().status(StatusCode::OK).header("Content-Type", "text/html; charset=utf-8").header("Cache-Control", "no-store").body(full(setup_page(&app_url, &fp))).unwrap()
    }

    // WebSocket

    /// Blocks cross-site WebSockets. Passing still only earns a connection with five seconds to show a valid token.
    fn handle_upgrade(self: Arc<Self>, mut req: Request<Incoming>) -> Response<Body> {
        let host = req.headers().get(HOST).and_then(|h| h.to_str().ok()).unwrap_or("").to_string();
        let origin = req.headers().get(ORIGIN).and_then(|h| h.to_str().ok()).map(str::to_string);
        let mesh = !self.st.lock().unwrap().mesh_addrs.is_empty();
        let is_ws = req.headers().get(UPGRADE).and_then(|h| h.to_str().ok()).is_some_and(|u| u.eq_ignore_ascii_case("websocket"));
        let key = req.headers().get(SEC_WEBSOCKET_KEY).cloned();
        if req.uri().path() != "/ws" || req.uri().query().is_some() || !is_ws || !certs::is_allowed_origin(origin.as_deref(), &host, mesh) {
            return plain(StatusCode::FORBIDDEN);
        }
        let Some(key) = key else { return plain(StatusCode::BAD_REQUEST) };
        let accept = derive_accept_key(key.as_bytes());
        let upgraded = hyper::upgrade::on(&mut req);
        let me = self.clone();
        let host_only = host.split(':').next().unwrap_or("").to_string();
        tokio::spawn(async move {
            if let Ok(u) = upgraded.await {
                me.run_client(TokioIo::new(u), host_only).await;
            }
        });
        Response::builder().status(StatusCode::SWITCHING_PROTOCOLS).header(CONNECTION, "Upgrade").header(UPGRADE, "websocket").header(SEC_WEBSOCKET_ACCEPT, accept).body(full(Bytes::new())).unwrap()
    }

    async fn run_client<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static>(self: Arc<Self>, io: S, host: String) {
        let mut cfg = WebSocketConfig::default();
        cfg.max_message_size = Some(1024 * 1024);
        cfg.max_frame_size = Some(1024 * 1024);
        let ws = WebSocketStream::from_raw_socket(io, Role::Server, Some(cfg)).await;
        let (mut sink, mut stream) = ws.split();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let client = Arc::new(Client { conn_id: uuid(), tx, backlog: AtomicUsize::new(0), device: Mutex::new(None), attached: Mutex::default(), visible: AtomicBool::new(true), host, kill: Notify::new(), handle: Handle::current() });
        self.clients.lock().unwrap().insert(client.conn_id.clone(), client.clone());

        let writer = {
            let client = client.clone();
            async move {
                while let Some(out) = rx.recv().await {
                    match out {
                        Out::Text(s) => {
                            client.backlog.fetch_sub(s.len(), Ordering::SeqCst);
                            if sink.send(Message::Text(s.into())).await.is_err() {
                                return;
                            }
                        }
                        Out::Close(code) => {
                            let _ = sink.send(Message::Close(Some(CloseFrame { code: CloseCode::from(code), reason: "".into() }))).await;
                            return;
                        }
                    }
                }
            }
        };
        let reader = {
            let (me, client) = (self.clone(), client.clone());
            async move {
                // No valid token within five seconds, no connection
                let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
                loop {
                    let next = if client.authed() {
                        stream.next().await
                    } else {
                        match tokio::time::timeout_at(deadline, stream.next()).await {
                            Ok(n) => n,
                            Err(_) => {
                                client.drop_with(4001);
                                return std::future::pending::<()>().await;
                            }
                        }
                    };
                    let raw = match next {
                        Some(Ok(Message::Text(t))) => t.to_string(),
                        Some(Ok(Message::Binary(b))) => String::from_utf8_lossy(&b).into_owned(),
                        Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                        Some(Ok(_)) => continue,
                    };
                    let Ok(msg) = serde_json::from_str::<Value>(&raw) else { continue };
                    me.clone().on_message(client.clone(), msg).await;
                }
            }
        };
        tokio::select! {
            _ = writer => {}
            _ = reader => {}
            _ = client.kill.notified() => {}
        }
        let me = self.clone();
        let c = client.clone();
        let _ = Self::blocking(move || me.client_gone(&c)).await;
    }

    fn client_gone(&self, c: &Arc<Client>) {
        if self.clients.lock().unwrap().remove(&c.conn_id).is_none() {
            return;
        }
        if let Some(core) = self.core() {
            let key = format!("remote:{}", c.conn_id);
            for id in c.attached.lock().unwrap().iter() {
                core.unsubscribe_pty(id, &key);
            }
        }
        if let (Some(id), Some(store)) = (c.device_id(), self.store()) {
            store.update_device(&id, "lastSeenAt", json!(now_ms() as u64));
            store.audit("device.disconnect", json!({ "deviceId": id }));
        }
        self.notify_status();
    }

    async fn on_message(self: Arc<Self>, c: Arc<Client>, msg: Value) {
        if !c.authed() {
            let me = self.clone();
            let client = c.clone();
            let _ = Self::blocking(move || me.pre_auth(&client, &msg)).await;
            return;
        }
        let t = sv(&msg, "t").to_string();
        match t.as_str() {
            "ping" => return c.send(&json!({ "t": "pong" })),
            "visibility" => {
                c.visible.store(msg.get("visible").is_some_and(crate::settings::truthy), Ordering::SeqCst);
                return;
            }
            "resize" => return, // The desktop owns the shared terminal's size; older phone pages still send this
            _ => {}
        }
        let (me, client) = (self.clone(), c.clone());
        let work = async move {
            let (m2, c2, msg2) = (me.clone(), client.clone(), msg.clone());
            let joined = tokio::task::spawn_blocking(move || m2.handle_message(&c2, &msg2)).await;
            let error = match joined {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e),
                Err(_) => Some("internal error".to_string()),
            };
            if let Some(e) = error {
                client.send(&json!({ "t": "error", "message": e }));
            }
        };
        // Keystrokes must stay in order, so most things finish before the next message is read. Long jobs must not hold up typing.
        if matches!(t.as_str(), "cowork" | "git" | "spawn" | "files" | "file" | "status") {
            tokio::spawn(work);
        } else {
            work.await;
        }
    }

    fn pre_auth(&self, c: &Arc<Client>, msg: &Value) {
        let Some(store) = self.store() else { return };
        let host_name = certs::os_hostname();
        match sv(msg, "t") {
            // Same protection as /api/pair: one-time code, five minutes, ten wrong tries voids it
            "pairRequest" => {
                let Some((token, device_id)) = self.try_pair(sv(msg, "code"), sv(msg, "name")) else {
                    return c.send(&json!({ "t": "error", "message": "invalid or expired code" }));
                };
                let Some(device) = store.find_device_by_token(&token) else { return c.drop_with(4001) };
                c.send(&json!({ "t": "paired", "token": token, "deviceId": device_id }));
                c.send(&json!({ "t": "authed", "deviceId": device["id"], "deviceName": device["name"], "hostName": host_name }));
                *c.device.lock().unwrap() = Some(device);
                if let Some(s) = self.build_state() {
                    c.send(&s);
                }
                self.notify_status();
            }
            "auth" => {
                let Some(device) = store.find_device_by_token(sv(msg, "token")) else { return c.drop_with(4001) };
                store.update_device(sv(&device, "id"), "lastSeenAt", json!(now_ms() as u64));
                store.audit("device.connect", json!({ "deviceId": device["id"] }));
                c.send(&json!({ "t": "authed", "deviceId": device["id"], "deviceName": device["name"], "hostName": host_name }));
                *c.device.lock().unwrap() = Some(device);
                if let Some(s) = self.build_state() {
                    c.send(&s);
                }
                self.notify_status();
            }
            _ => c.drop_with(4001),
        }
    }

    fn broadcast(&self, msg: &Value, filter: impl Fn(&Arc<Client>) -> bool) {
        for c in self.authed_clients() {
            if filter(&c) {
                c.send(msg);
            }
        }
    }

    fn workspace_of(&self, core: &Core, window: Option<u64>) -> Option<String> {
        let id = window? as u32;
        // A window with no folder has no workspace, or its reads would land in the app's current directory
        self.window_root(core, id).filter(|ws| !ws.is_empty())
    }

    /// The window itself, folder or not: the configured custom harness also starts in a window without one
    fn window_root(&self, core: &Core, id: u32) -> Option<String> {
        core.project_workspaces().into_iter().find(|(w, _)| *w == id).map(|(_, ws)| ws)
    }

    fn handle_message(&self, c: &Arc<Client>, msg: &Value) -> Result<(), String> {
        let core = self.core().ok_or("shutting down")?;
        let store = core.features().remote.store.clone();
        let device_id = c.device_id();
        let sub_key = format!("remote:{}", c.conn_id);
        let id = sv(msg, "id");
        let window = msg.get("windowId").and_then(Value::as_u64);
        match sv(msg, "t") {
            "setBypass" => {
                let enabled = msg.get("enabled").is_some_and(crate::settings::truthy);
                core.set_cli_bypass(enabled);
                store.audit(if enabled { "bypass.enable" } else { "bypass.disable" }, json!({ "deviceId": device_id }));
                // Turning off the safety from a phone is a high-risk change: the desktop always has to show it
                if enabled {
                    let name = c.device.lock().unwrap().as_ref().map(|d| sv(d, "name").to_string()).unwrap_or_default();
                    core.host.notify("Bypass mode enabled", &format!("「{name}」開啟了 Bypass 模式：之後啟動的 agent 不會再詢問就執行指令"));
                }
                if let Some(s) = self.build_state() {
                    self.broadcast(&s, |_| true);
                }
            }
            "attach" => {
                let sub = Arc::new(RemoteSub { client: Arc::downgrade(c), pending: Mutex::new(Some(Vec::new())) });
                let Some((data, cols, rows)) = core.subscribe_pty(id, &sub_key, sub.clone()) else {
                    c.send(&json!({ "t": "error", "message": "session not found" }));
                    return Ok(());
                };
                c.attached.lock().unwrap().insert(id.to_string());
                let mut pending = sub.pending.lock().unwrap();
                c.send(&json!({ "t": "snapshot", "id": id, "data": data, "cols": cols, "rows": rows }));
                for m in pending.take().unwrap_or_default() {
                    c.send(&m);
                }
            }
            "detach" => {
                core.unsubscribe_pty(id, &sub_key);
                c.attached.lock().unwrap().remove(id);
            }
            "input" => {
                if let Some(data) = msg.get("data").and_then(Value::as_str).filter(|d| js_len(d) <= 64 * 1024) {
                    core.write_pty(id, data);
                }
            }
            "kill" => {
                store.audit("session.kill", json!({ "deviceId": device_id, "session": id }));
                core.kill_pty(id);
            }
            "spawn" => {
                let Some(win) = window.map(|w| w as u32) else {
                    c.send(&json!({ "t": "error", "message": "window not found" }));
                    return Ok(());
                };
                let Some(ws) = self.window_root(&core, win) else {
                    c.send(&json!({ "t": "error", "message": "window not found" }));
                    return Ok(());
                };
                let launchers = self.launchers_for(&core, &ws);
                let Some(launcher) = launchers.iter().find(|l| sv(l, "key") == sv(msg, "launcherKey")) else {
                    c.send(&json!({ "t": "error", "message": "launcher not available" }));
                    return Ok(());
                };
                let key = sv(launcher, "key");
                let title = sv(launcher, "title");
                let mut opts = json!({ "title": title, "cols": 120, "rows": 34 });
                opts[if BUILTIN.iter().any(|b| b.0 == key) { "command" } else { "launcherId" }] = json!(key);
                let new_id = core.spawn_pty(&opts, &ws, win, false)?;
                // A tab on the desktop joins the same terminal, so what the phone started can be seen and taken over there.
                // Events wait in the host's queue until the window subscribes, so a window still loading gets it too.
                core.host.send(win, "pty:remoteSpawned", json!([{ "ptyId": new_id, "launcherKey": key, "title": title }]));
                store.audit("session.spawn", json!({ "deviceId": device_id, "launcher": key, "workspace": ws }));
                c.send(&json!({ "t": "spawned", "id": new_id }));
            }
            "preview" => {
                let port = core.get_pty_session(id).and_then(|s| s["devPort"].as_u64());
                let Some(port) = port else {
                    c.send(&json!({ "t": "preview", "id": id, "url": null, "error": "no dev server detected" }));
                    return Ok(());
                };
                match self.preview.issue_url(&c.host, port as u16) {
                    Some(url) => {
                        store.audit("preview.open", json!({ "deviceId": device_id, "session": id, "devPort": port }));
                        c.send(&json!({ "t": "preview", "id": id, "url": url }));
                    }
                    None => c.send(&json!({ "t": "preview", "id": id, "url": null, "error": "preview unavailable" })),
                }
            }
            "openWorkspace" => {
                // Only paths on the desktop's own recent list: a phone cannot name any folder and have an agent run there
                let open: Vec<String> = core.project_workspaces().into_iter().map(|(_, w)| w).collect();
                let wanted = self.workspace_options(&core, &open).into_iter().find(|w| sv(w, "path") == sv(msg, "path"));
                let Some(wanted) = wanted else {
                    c.send(&json!({ "t": "error", "message": "workspace not available" }));
                    return Ok(());
                };
                core.host.open_project_window(Some(sv(&wanted, "path")));
                store.audit("workspace.open", json!({ "deviceId": device_id, "workspace": sv(&wanted, "path") }));
                if let Some(s) = self.build_state() {
                    self.broadcast(&s, |_| true);
                }
            }
            "handoff" => {
                let text = self.workspace_of(&core, window).and_then(|ws| std::fs::read(std::path::Path::new(&ws).join(".project-memory").join("handoff.md")).ok()).map(|b| js_prefix(&String::from_utf8_lossy(&b), 200_000));
                c.send(&json!({ "t": "handoff", "windowId": window, "text": text }));
            }
            "status" => {
                let ws = self.workspace_of(&core, window);
                let status = window.and_then(|w| self.vibe.lock().unwrap().get(&(w as u32)).cloned()).filter(|s| ws.as_deref() == Some(sv(s, "workspace")));
                let mut out = json!({ "t": "status", "windowId": window, "status": status, "bgTasks": ws.as_deref().map(|w| core.bg.scan(w)).unwrap_or_default() });
                if ws.is_none() {
                    out["error"] = json!("no workspace");
                }
                c.send(&out);
            }
            "cowork" => {
                let op = sv(msg, "op").to_string();
                let req_id = msg.get("reqId").cloned().unwrap_or(Value::Null);
                if !PHONE_COWORK_OPS.contains(&op.as_str()) {
                    c.send(&json!({ "t": "cowork", "reqId": req_id, "result": { "ok": false, "code": "not-allowed" } }));
                    return Ok(());
                }
                let args: Vec<Value> = msg.get("args").and_then(Value::as_array).cloned().unwrap_or_default();
                let result = match self.workspace_of(&core, window) {
                    Some(ws) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::cowork_ipc::wrap(crate::cowork_ipc::invoke(&core, &op, &ws, &args)))).unwrap_or_else(|_| json!({ "ok": false, "code": "internal" })),
                    None => json!({ "ok": false, "code": "no-workspace" }),
                };
                if PHONE_AUDITED_OPS.contains(&op.as_str()) {
                    store.audit(&format!("cowork.{op}"), cowork_audit(&op, &args, &result, device_id.as_deref()));
                }
                c.send(&json!({ "t": "cowork", "reqId": req_id, "result": result }));
            }
            "files" => {
                let path = sv(msg, "path");
                let listed = self.workspace_of(&core, window).ok_or_else(|| "no workspace".to_string()).and_then(|ws| files::list_workspace_files(&ws, path));
                c.send(&match listed {
                    Ok(entries) => json!({ "t": "files", "windowId": window, "path": path, "entries": entries }),
                    Err(e) => json!({ "t": "files", "windowId": window, "path": path, "entries": [], "error": e }),
                });
            }
            "file" => {
                let path = sv(msg, "path");
                let ext = std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                let kind = match ext.as_str() {
                    "pdf" => "pdf",
                    "html" | "htm" => "html",
                    "md" | "markdown" => "markdown",
                    _ => "text",
                };
                let port = self.st.lock().unwrap().port;
                let opened = (|| -> Result<(Option<String>, String), String> {
                    let ws = self.workspace_of(&core, window).ok_or("no workspace")?;
                    let text = if kind == "markdown" || kind == "text" { Some(files::read_workspace_text(&ws, path)?) } else { None };
                    let url = self.files.issue_url(&format!("{}:{port}", c.host), &ws, path, device_id.as_deref().unwrap_or(""))?;
                    Ok((text, url))
                })();
                c.send(&match opened {
                    Ok((text, url)) => json!({ "t": "file", "windowId": window, "path": path, "kind": kind, "text": text, "url": url }),
                    Err(e) => json!({ "t": "file", "windowId": window, "path": path, "kind": kind, "text": null, "url": null, "error": e }),
                });
            }
            "git" => {
                let failed = |e: String| json!({ "t": "git", "windowId": window, "branch": null, "ahead": 0, "behind": 0, "files": [], "error": e });
                c.send(&match self.workspace_of(&core, window) {
                    None => failed("no workspace".into()),
                    Some(ws) => match wb_git::git(std::path::Path::new(&ws), &["status", "--porcelain", "-b", "-u", "--null"]) {
                        Ok(out) => {
                            let mut v = parse_status(&out);
                            v["t"] = json!("git");
                            v["windowId"] = json!(window);
                            v
                        }
                        Err(e) => failed(e),
                    },
                });
            }
            _ => {}
        }
        Ok(())
    }

    fn launchers_for(&self, core: &Arc<Core>, ws: &str) -> Vec<Value> {
        let mut out: Vec<Value> = BUILTIN.iter().filter(|b| core.is_cli_enabled(b.0)).map(|b| json!({ "key": b.0, "title": b.1, "kind": b.2 })).collect();
        // An alias of a built-in CLI with no args or env of its own is not listed twice
        if let Ok(Value::Array(list)) = core.list_launchers(ws) {
            for l in list {
                let cli = sv(&l, "cli");
                let direct = BUILTIN.iter().any(|b| b.0 == cli);
                if l["hasExtras"] == json!(true) || !direct || !core.is_cli_enabled(cli) {
                    out.push(json!({ "key": l["id"], "title": l["name"], "kind": "custom" }));
                }
            }
        }
        out
    }

    /// Recently used workspaces with no window now: the phone's workspace menu
    fn workspace_options(&self, core: &Core, open: &[String]) -> Vec<Value> {
        let norm = |p: &str| resolve(p).replace('\\', "/").to_lowercase();
        let open: HashSet<String> = open.iter().filter(|p| !p.is_empty()).map(|p| norm(p)).collect();
        core.get_recent_workspaces().into_iter().filter(|p| !open.contains(&norm(p))).take(12).map(|p| {
            let name = basename(&p);
            json!({ "path": p, "name": if name.is_empty() { p.clone() } else { name } })
        }).collect()
    }

    fn build_state(&self) -> Option<Value> {
        let core = self.core()?;
        let mut windows = core.project_workspaces();
        windows.sort();
        let mut list = core.list_pty_sessions();
        list.sort_by_key(|s| s["startTime"].as_u64().unwrap_or(0));
        let sessions: Vec<Value> = list.iter().filter_map(|s| {
            let owner = s["ownerId"].as_u64()? as u32;
            windows.iter().any(|(w, _)| *w == owner).then(|| ())?;
            let id = sv(s, "id");
            let launcher = s.get("launcherId").and_then(Value::as_str).filter(|l| !l.is_empty()).unwrap_or(sv(s, "command"));
            let name = basename(sv(s, "workspace"));
            let mut row = json!({ "id": id, "title": s["title"], "launcherKey": launcher, "windowId": owner, "workspaceName": if name.is_empty() { "~" } else { &name }, "startTime": s["startTime"], "cols": s["cols"], "rows": s["rows"], "needsApproval": s["needsApproval"], "lastOutputAt": s["lastOutputAt"], "busy": s["busy"], "devPort": s["devPort"] });
            if s["needsApproval"] == json!(true) {
                row["approvalTail"] = json!(js_suffix(&core.pty_screen_text(id).unwrap_or_default(), 2000));
            }
            Some(row)
        }).collect();
        let remote_windows: Vec<Value> = windows.iter().map(|(id, ws)| {
            let name = basename(ws);
            json!({ "id": id, "workspace": ws, "workspaceName": if name.is_empty() { "(no folder)" } else { &name }, "launchers": self.launchers_for(&core, ws) })
        }).collect();
        let open: Vec<String> = windows.iter().map(|(_, w)| w.clone()).collect();
        Some(json!({ "t": "state", "sessions": sessions, "windows": remote_windows, "workspaces": self.workspace_options(&core, &open), "bypass": core.is_cli_bypass() }))
    }

    pub fn schedule_state(&self) {
        let Some(handle) = self.handle.lock().unwrap().clone() else { return };
        if self.state_due.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(me) = self.me.upgrade() else { return };
        handle.spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            me.state_due.store(false, Ordering::SeqCst);
            if me.clients.lock().unwrap().is_empty() {
                return;
            }
            let m2 = me.clone();
            if let Some(Some(s)) = Self::blocking(move || m2.build_state()).await {
                me.broadcast(&s, |_| true);
            }
        });
    }

    fn on_pty_event(&self, ev: &PtyEvent) {
        match ev {
            PtyEvent::Changed => self.schedule_state(),
            // Only on a flip: no need to rebuild the whole state for this
            PtyEvent::Activity(id, busy) => self.broadcast(&json!({ "t": "activity", "id": id, "busy": busy }), |_| true),
            PtyEvent::Approval(id) => {
                let Some(handle) = self.handle.lock().unwrap().clone() else { return };
                let (Some(me), id) = (self.me.upgrade(), id.clone()) else { return };
                handle.spawn_blocking(move || me.on_approval(&id));
            }
            PtyEvent::Exit { id, code, title, killed } => {
                self.broadcast(&json!({ "t": "exit", "id": id, "code": code, "title": title }), |_| true);
                // A terminal the user closed needs no notification
                if !killed {
                    self.push(id, "exit", json!({ "title": format!("✓ {title} 已結束"), "body": format!("exit code {code}"), "tag": format!("exit-{id}"), "url": "/" }));
                }
            }
        }
    }

    fn on_approval(&self, id: &str) {
        let Some(core) = self.core() else { return };
        let Some(s) = core.get_pty_session(id) else { return };
        let title = sv(&s, "title");
        let tail = js_suffix(&core.pty_screen_text(id).unwrap_or_default(), 3000);
        self.broadcast(&json!({ "t": "approval", "id": id, "title": title, "tail": tail }), |_| true);
        let lines = last_lines(&tail, 2);
        let body = format!("{} · {}", basename(sv(&s, "workspace")), if lines.is_empty() { "Approval needed" } else { &lines });
        self.push(id, "approval", json!({ "title": format!("⏳ {title} 等待你的回覆"), "body": body, "tag": format!("approval-{id}"), "url": format!("/#s={id}") }));
    }

    /// The Cowork run goes to phones without `context`, which is skill and project text a phone has no use for
    pub fn on_cowork(&self, run: &Value) {
        if self.clients.lock().unwrap().is_empty() {
            return;
        }
        let mut r = run.clone();
        if let Some(o) = r.as_object_mut() {
            o.shift_remove("context");
        }
        self.broadcast(&json!({ "t": "coworkRun", "run": r }), |c| c.visible.load(Ordering::SeqCst));
    }

    pub fn report_vibe_status(&self, window: u32, status: &Value) {
        let Some(core) = self.core() else { return };
        if self.workspace_of(&core, Some(window as u64)).as_deref() == Some(sv(status, "workspace")) {
            self.vibe.lock().unwrap().insert(window, status.clone());
        }
    }

    /// A device with the app in the foreground already sees it over WebSocket; other subscribed ones get Web Push
    fn push(&self, id: &str, kind: &str, payload: Value) {
        let Some(vapid) = self.st.lock().unwrap().vapid.clone() else { return };
        let Some(handle) = self.handle.lock().unwrap().clone() else { return };
        let Some(store) = self.store() else { return };
        let now = now_ms();
        {
            let mut t = self.push_throttle.lock().unwrap();
            let key = format!("{kind}:{id}");
            if now - t.get(&key).copied().unwrap_or(0.0) < 20_000.0 {
                return;
            }
            t.insert(key, now);
        }
        let foreground: HashSet<String> = self.authed_clients().iter().filter(|c| c.visible.load(Ordering::SeqCst)).filter_map(|c| c.device_id()).collect();
        for d in store.load_devices() {
            let Some(sub) = d.get("push").cloned() else { continue };
            let device_id = sv(&d, "id").to_string();
            if foreground.contains(&device_id) {
                continue;
            }
            let (payload, vapid, store) = (payload.clone(), vapid.clone(), store.clone());
            handle.spawn_blocking(move || {
                let r = webpush::send_push(&sub, &payload, &vapid, PUSH_SUBJECT);
                if r.gone {
                    store.update_device(&device_id, "push", Value::Null);
                    store.audit("push.gone", json!({ "deviceId": device_id, "status": r.status }));
                }
            });
        }
    }
}

/// Content identifiers only (run, hash, commit); never a reason's text or anything secret
fn cowork_audit(op: &str, args: &[Value], result: &Value, device_id: Option<&str>) -> Value {
    let ids: Vec<Value> = args.iter().map(|v| match v {
        Value::String(s) => json!(js_prefix(s, 80)),
        Value::Object(o) => json!({ "commit": o.get("commit"), "targetHead": o.get("targetHead"), "approval": o.get("approval"), "waived": o.get("waive").is_some_and(crate::settings::truthy), "mode": o.get("mode") }),
        other => other.clone(),
    }).collect();
    let shown = if op == "execWaive" { &ids[..ids.len().min(2)] } else { &ids[..] };
    let mut row = json!({ "deviceId": device_id, "args": shown, "ok": result["ok"] });
    if result["ok"] != json!(true) {
        row["code"] = result["code"].clone();
    }
    row
}

/// `git status --porcelain -b -u --null`, as the phone's panel expects it
fn parse_status(out: &str) -> Value {
    let (mut branch, mut ahead, mut behind, mut files) = (Value::Null, 0u64, 0u64, Vec::new());
    let mut parts = out.split('\0');
    while let Some(entry) = parts.next() {
        if let Some(rest) = entry.strip_prefix("## ") {
            let rest = rest.strip_prefix("No commits yet on ").or(rest.strip_prefix("Initial commit on ")).unwrap_or(rest);
            let name = if rest.starts_with("HEAD (no branch)") { "HEAD" } else { rest.split("...").next().unwrap_or(rest).split(" [").next().unwrap_or(rest) };
            branch = json!(name);
            if let Some(tail) = rest.rsplit_once(" [").map(|x| x.1) {
                for item in tail.trim_end_matches(']').split(", ") {
                    match item.split_once(' ') {
                        Some(("ahead", n)) => ahead = n.parse().unwrap_or(0),
                        Some(("behind", n)) => behind = n.parse().unwrap_or(0),
                        _ => {}
                    }
                }
            }
            continue;
        }
        if entry.len() < 4 || !entry.is_char_boundary(3) {
            continue;
        }
        let (index, work, path) = (&entry[..1], &entry[1..2], &entry[3..]);
        if index == "R" || index == "C" {
            parts.next(); // a rename's next field is the old name
        }
        if files.len() < 500 {
            files.push(json!({ "path": path, "index": index, "workingDir": work }));
        }
    }
    json!({ "branch": branch, "ahead": ahead, "behind": behind, "files": files })
}

pub fn tls_acceptor(pair: &CertPair) -> Result<TlsAcceptor, String> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs1KeyDer, PrivatePkcs8KeyDer};
    let cert = CertificateDer::from(certs::cert_der(&pair.cert_pem)?);
    let key_der = certs::cert_der(&pair.key_pem)?;
    let key: PrivateKeyDer = if pair.key_pem.contains("RSA PRIVATE KEY") { PrivatePkcs1KeyDer::from(key_der).into() } else { PrivatePkcs8KeyDer::from(key_der).into() };
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ServerConfig::builder_with_provider(provider).with_protocol_versions(&[&rustls::version::TLS12, &rustls::version::TLS13]).map_err(|e| e.to_string())?.with_no_client_auth().with_single_cert(vec![cert], key).map_err(|e| e.to_string())?;
    Ok(TlsAcceptor::from(Arc::new(cfg)))
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

pub fn setup_page(app_url: &str, fingerprint: &str) -> String {
    let (url, fp) = (escape_html(app_url), escape_html(fingerprint));
    format!(
        r#"<!doctype html><html lang="zh-Hant"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Agent Workbench · iPhone 設定</title>
<style>
:root{{color-scheme:light dark;--bg:#f5f5f7;--card:#fff;--fg:#1d1d1f;--mut:#6e6e73;--acc:#0a84ff}}
@media (prefers-color-scheme:dark){{:root{{--bg:#000;--card:#1c1c1e;--fg:#f5f5f7;--mut:#98989d}}}}
body{{margin:0;padding:24px 16px 48px;font:16px/1.5 -apple-system,system-ui,sans-serif;background:var(--bg);color:var(--fg)}}
h1{{font-size:24px;margin:8px 0 4px}}p{{color:var(--mut);margin:4px 0 16px}}
ol{{list-style:none;padding:0;margin:0;display:grid;gap:12px}}
li{{background:var(--card);border-radius:14px;padding:16px}}
b{{display:block;margin-bottom:4px}}.btn{{display:block;text-align:center;background:var(--acc);color:#fff;text-decoration:none;border-radius:12px;padding:12px;margin-top:10px;font-weight:600}}
code{{font-size:11px;word-break:break-all;color:var(--mut)}}
</style></head><body>
<h1>用 iPhone 遠端控制</h1>
<p>第一次使用需要讓 iPhone 信任這台電腦的本機憑證（只能用在區網位址）。</p>
<ol>
<li><b>1. 下載憑證</b>用 Safari 開啟此頁並點下方按鈕，選「允許」。<a class="btn" href="/ca.crt">下載描述檔</a></li>
<li><b>2. 安裝描述檔</b>設定 → 一般 → VPN 與裝置管理 → 選「Agent Workbench Local CA」→ 安裝。</li>
<li><b>3. 開啟完全信任</b>設定 → 一般 → 關於本機 → 憑證信任設定 → 打開「Agent Workbench Local CA」。</li>
<li><b>4. 開啟 App</b>用 Safari 開啟下方連結 → 分享 → 加入主畫面，再從主畫面開啟並輸入桌面上顯示的配對碼。<a class="btn" href="{url}">開啟 {url}</a></li>
</ol>
<p style="margin-top:20px">核對憑證指紋（SHA-256，需與桌面設定頁一致）：<br><code>{fp}</code></p>
</body></html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_hosts() {
        for ok in ["https://web.push.apple.com/x", "https://fcm.googleapis.com/x", "https://updates.push.services.mozilla.com/x", "https://db5.notify.windows.com/x"] {
            assert!(is_push_endpoint(ok), "{ok}");
        }
        for bad in ["http://web.push.apple.com/x", "https://evil.com/web.push.apple.com", "https://web.push.apple.com.evil.com/", "https://notify.windows.com.evil.com/", "https://evilpush.services.mozilla.com/", "nope", ""] {
            assert!(!is_push_endpoint(bad), "{bad}");
        }
    }

    #[test]
    fn only_expected_static_paths() {
        assert_eq!(static_rel("/").as_deref(), Some("remote.html"));
        assert_eq!(static_rel("/assets/x-1a.js").as_deref(), Some("assets/x-1a.js"));
        assert_eq!(static_rel("/remote-sw.js").as_deref(), Some("remote-sw.js"));
        for bad in ["/assets/..", "/assets/.", "/assets/a/b.js", "/assets/../secret.json", "/assets/%2e%2e.js", "/remote/..", "/index.js", "/settings.json"] {
            assert!(static_rel(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn status_parse() {
        let v = parse_status("## main...origin/main [ahead 2, behind 3]\0 M a.txt\0R  new.txt\0old.txt\0?? b.txt\0");
        assert_eq!(v["branch"], "main");
        assert_eq!((v["ahead"].as_u64(), v["behind"].as_u64()), (Some(2), Some(3)));
        let names: Vec<&str> = v["files"].as_array().unwrap().iter().map(|f| sv(f, "path")).collect();
        assert_eq!(names, ["a.txt", "new.txt", "b.txt"]);
        assert_eq!(parse_status("## No commits yet on dev\0")["branch"], "dev");
        assert_eq!(parse_status("## HEAD (no branch)\0")["branch"], "HEAD");
        assert_eq!(parse_status("## solo\0")["branch"], "solo");
    }

    #[test]
    fn page_escapes_what_came_from_a_header() {
        let page = setup_page("https://\"><script>alert(1)</script>:1/", "AA");
        assert!(!page.contains("<script>"));
    }

    #[test]
    fn audit_keeps_identifiers_only() {
        let a = cowork_audit("execWaive", &[json!("run"), json!("t1"), json!("a secret reason"), json!(true)], &json!({ "ok": true }), Some("d"));
        assert!(!a.to_string().contains("secret"));
        let b = cowork_audit("execMerge", &[json!("r"), json!({ "commit": "c", "waive": "text", "other": "x" })], &json!({ "ok": false, "code": "stale" }), Some("d"));
        assert_eq!(b["code"], "stale");
        assert_eq!(b["args"][1]["waived"], true);
        assert!(!b.to_string().contains("\"other\""));
    }
}
