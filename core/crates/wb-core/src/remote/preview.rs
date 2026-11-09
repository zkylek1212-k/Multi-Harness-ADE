// Phone preview: a dev server a terminal printed, reached through an HTTPS reverse proxy on its own port.
// It has to be a proxy: the phone's localhost is the phone, dev servers bind 127.0.0.1, and an https page cannot frame http.
// It has to be its own origin: previewed pages use root-relative URLs that would otherwise collide with the app's own files.
//
// Access: private-network peers only (same guard as the bridge); first entry needs a one-time ticket issued over an authenticated
// WebSocket, exchanged for an HttpOnly cookie; the target must be a port a terminal really printed, and it is always 127.0.0.1.
use std::collections::HashMap;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::header::{HeaderValue, CONNECTION, COOKIE, HOST, UPGRADE};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use ring::hmac;
use tokio::net::TcpStream;

use super::server::{full, text, Body};
use super::store::ct_eq;
use crate::util::now_ms;

const TICKET_TTL_MS: f64 = 60_000.0;
const COOKIE_NAME: &str = "aw_preview";
const AUTH_PATH: &str = "/__aw";

fn cookie_of(req: &Request<Incoming>) -> Option<String> {
    for header in req.headers().get_all(COOKIE) {
        for part in header.to_str().unwrap_or("").split(';') {
            if let Some((k, v)) = part.split_once('=') {
                if k.trim() == COOKIE_NAME {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

pub struct Preview {
    port: AtomicU16,
    key: Mutex<hmac::Key>,
    tickets: Mutex<HashMap<String, (u16, f64)>>,
    allow_port: Box<dyn Fn(u16) -> bool + Send + Sync>,
}

impl Preview {
    /// allow_port: is this still a port some terminal printed?
    pub fn new(allow_port: Box<dyn Fn(u16) -> bool + Send + Sync>) -> Self {
        Self { port: AtomicU16::new(0), key: Mutex::new(new_key()), tickets: Mutex::default(), allow_port }
    }

    pub fn started(&self, port: u16) {
        *self.key.lock().unwrap() = new_key();
        self.tickets.lock().unwrap().clear();
        self.port.store(port, Ordering::SeqCst);
    }

    /// Every cookie and ticket issued so far stops working
    pub fn rotate(&self) {
        *self.key.lock().unwrap() = new_key();
        self.tickets.lock().unwrap().clear();
    }

    pub fn stopped(&self) {
        self.port.store(0, Ordering::SeqCst);
        self.tickets.lock().unwrap().clear();
    }

    /// A one-time ticket and the address to open, using the host the phone itself connected to
    pub fn issue_url(&self, host: &str, dev_port: u16) -> Option<String> {
        let port = self.port.load(Ordering::SeqCst);
        if port == 0 || !(self.allow_port)(dev_port) {
            return None;
        }
        let ticket = super::random_b64u(24);
        let now = now_ms();
        let mut t = self.tickets.lock().unwrap();
        t.retain(|_, (_, at)| *at >= now);
        t.insert(ticket.clone(), (dev_port, now + TICKET_TTL_MS));
        Some(format!("https://{host}:{port}{AUTH_PATH}?t={ticket}"))
    }

    fn sign(&self, port: u16) -> String {
        super::b64u(hmac::sign(&self.key.lock().unwrap(), port.to_string().as_bytes()).as_ref())
    }

    /// The cookie carries the port and its signature, so nothing has to be stored
    fn port_from_cookie(&self, req: &Request<Incoming>) -> Option<u16> {
        let raw = cookie_of(req)?;
        let (port, sig) = raw.rsplit_once('.')?;
        let port: u16 = port.parse().ok()?;
        (ct_eq(self.sign(port).as_bytes(), sig.as_bytes()) && (self.allow_port)(port)).then_some(port)
    }

    pub async fn handle(self: Arc<Self>, mut req: Request<Incoming>) -> Response<Body> {
        if req.uri().path() == AUTH_PATH {
            let ticket = url::form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes()).find(|(k, _)| k == "t").map(|(_, v)| v.into_owned()).unwrap_or_default();
            let grant = self.tickets.lock().unwrap().remove(&ticket);
            return match grant.filter(|(port, at)| *at >= now_ms() && (self.allow_port)(*port)) {
                Some((port, _)) => Response::builder().status(StatusCode::FOUND).header("Set-Cookie", format!("{COOKIE_NAME}={port}.{}; Path=/; Secure; HttpOnly; SameSite=Lax", self.sign(port))).header("Location", "/").header("Cache-Control", "no-store").body(full(Bytes::new())).unwrap(),
                None => text(StatusCode::FORBIDDEN, "preview link expired — reopen Preview in the app"),
            };
        }
        let Some(port) = self.port_from_cookie(&req) else { return text(StatusCode::FORBIDDEN, "no preview session") };

        let wants_upgrade = req.headers().contains_key(UPGRADE);
        let client_upgrade = wants_upgrade.then(|| hyper::upgrade::on(&mut req));
        let (parts, body) = req.into_parts();
        let mut out = Request::builder().method(parts.method).uri(parts.uri.path_and_query().map(|p| p.as_str()).unwrap_or("/")).version(hyper::Version::HTTP_11);
        for (k, v) in parts.headers.iter() {
            // Our cookie stays here. Without compression the proxy stays a plain pipe. Everything hop-by-hop is hyper's, except an upgrade.
            if k == COOKIE || k == HOST || k == "accept-encoding" || k == "keep-alive" || (k == CONNECTION && !wants_upgrade) {
                continue;
            }
            out = out.header(k, v);
        }
        let out = out.header(HOST, HeaderValue::from_str(&format!("127.0.0.1:{port}")).unwrap()).body(body).unwrap();

        let unreachable = || text(StatusCode::BAD_GATEWAY, "dev server not reachable");
        let Ok(tcp) = TcpStream::connect(("127.0.0.1", port)).await else { return unreachable() };
        let Ok((mut sender, conn)) = hyper::client::conn::http1::handshake(TokioIo::new(tcp)).await else { return unreachable() };
        tokio::spawn(conn.with_upgrades());
        let Ok(mut up) = sender.send_request(out).await else { return unreachable() };

        // HMR's WebSocket goes straight through, or Vite keeps reconnecting and covers the page with an error
        if up.status() == StatusCode::SWITCHING_PROTOCOLS {
            let Some(client_side) = client_upgrade else { return unreachable() };
            let server_side = hyper::upgrade::on(&mut up);
            tokio::spawn(async move {
                if let (Ok(a), Ok(b)) = (client_side.await, server_side.await) {
                    let (mut a, mut b) = (TokioIo::new(a), TokioIo::new(b));
                    let _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
                }
            });
            let (parts, _) = up.into_parts();
            return Response::from_parts(parts, full(Bytes::new()));
        }
        let (mut parts, body) = up.into_parts();
        // This page has to fit inside the phone app's iframe
        parts.headers.remove("x-frame-options");
        Response::from_parts(parts, body.map_err(|e| Box::new(e) as super::server::BoxError).boxed())
    }
}

fn new_key() -> hmac::Key {
    let mut b = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut b);
    hmac::Key::new(hmac::HMAC_SHA256, &b)
}
