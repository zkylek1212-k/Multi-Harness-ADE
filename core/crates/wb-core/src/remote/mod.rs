// Remote control's IPC: the settings page's switch, pairing codes and device list. The server is in server.rs.
pub mod certs;
pub mod files;
pub mod preview;
pub mod server;
pub mod store;
pub mod webpush;

use std::sync::Arc;

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use rand::RngCore;
use serde_json::{json, Value};

use crate::core::Core;
use crate::util::{js_prefix, sv};

pub struct RemoteApp {
    pub store: Arc<store::Store>,
    pub bridge: Arc<server::Bridge>,
}

pub fn start(core: &Arc<Core>) -> RemoteApp {
    RemoteApp { store: Arc::new(store::Store::new(&core.data_dir)), bridge: server::Bridge::new(core) }
}

/// The app starts regardless of whether the bridge can
pub fn start_background(core: &Arc<Core>) {
    let core = core.clone();
    std::thread::spawn(move || apply_enabled(&core));
}

pub(crate) fn random_bytes(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b
}

pub(crate) fn random_hex(n: usize) -> String {
    random_bytes(n).iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn random_b64u(n: usize) -> String {
    b64u(&random_bytes(n))
}

pub(crate) fn b64u(b: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(b)
}

/// Called with every Cowork change
pub fn on_cowork_update(core: &Core, run: &Value) {
    if let Some(f) = core.features.get() {
        f.remote.bridge.on_cowork(run);
    }
}

pub fn on_settings_changed(core: &Core) {
    if let Some(f) = core.features.get() {
        f.remote.bridge.schedule_state();
    }
}

pub fn shutdown(core: &Core) {
    if let Some(f) = core.features.get() {
        f.remote.bridge.stop();
    }
}

fn status(core: &Core) -> Value {
    let app = &core.features().remote;
    let cfg = app.store.load_config();
    let s = app.bridge.status();
    let primary = s.addresses.first();
    json!({
        "enabled": cfg.enabled, "running": s.running, "port": cfg.port, "addresses": s.addresses, "allowMesh": cfg.allow_mesh,
        "meshDetected": s.mesh_detected, "meshNeedsNewCa": s.mesh_needs_new_ca,
        "appUrl": primary.map(|p| format!("https://{p}:{}/", cfg.port)), "setupUrl": primary.map(|p| format!("http://{p}:{}/", cfg.port + 1)),
        "caFingerprint": s.ca_fingerprint, "connectedDevices": s.connected_devices, "error": s.error
    })
}

pub(crate) fn broadcast_status(core: &Core) {
    core.host.send_all("remote:status", json!([status(core)]));
}

fn apply_enabled(core: &Arc<Core>) {
    let app = &core.features().remote;
    let cfg = app.store.load_config();
    if cfg.enabled {
        if let Err(e) = app.bridge.start(cfg.port, cfg.allow_mesh) {
            eprintln!("[Remote] failed to start bridge: {e}");
        }
    } else {
        app.bridge.stop();
    }
}

pub fn qr_data_url(text: &str) -> Result<String, String> {
    let code = qrcode::QrCode::new(js_prefix(text, 500).as_bytes()).map_err(|e| e.to_string())?;
    let svg = code.render::<qrcode::render::svg::Color>().min_dimensions(360, 360).build();
    Ok(format!("data:image/svg+xml;base64,{}", STANDARD.encode(svg)))
}

pub fn call(core: &Arc<Core>, window: u32, channel: &str, args: &[Value]) -> Result<Value, String> {
    let app = &core.features().remote;
    let flag = args.first().is_some_and(crate::settings::truthy);
    match channel {
        "remote:status" => Ok(status(core)),
        "remote:vibeStatus" => {
            if let Some(s) = args.first() {
                app.bridge.report_vibe_status(window, s);
            }
            Ok(Value::Null)
        }
        "remote:setEnabled" => {
            app.store.save_config(Some(flag), None, None);
            app.store.audit(if flag { "bridge.enable" } else { "bridge.disable" }, json!({}));
            apply_enabled(core);
            Ok(status(core))
        }
        "remote:setPort" => {
            app.store.save_config(None, args.first(), None);
            apply_enabled(core);
            Ok(status(core))
        }
        "remote:setAllowMesh" => {
            app.store.save_config(None, None, Some(flag));
            app.store.audit(if flag { "mesh.enable" } else { "mesh.disable" }, json!({}));
            apply_enabled(core);
            Ok(status(core))
        }
        "remote:qr" => qr_data_url(args.first().and_then(Value::as_str).unwrap_or("")).map(Value::String),
        "remote:createPairing" => {
            if !app.bridge.running() {
                return Ok(Value::Null);
            }
            let s = app.bridge.status();
            let requested = args.first().and_then(Value::as_str).unwrap_or("");
            let address = if s.addresses.iter().any(|a| a == requested) { Some(requested.to_string()) } else { s.addresses.first().cloned() };
            let Some(address) = address else { return Ok(Value::Null) };
            let (code, expires_at) = app.bridge.create_pairing_code();
            let url = format!("https://{address}:{}/#pair={code}", app.store.load_config().port);
            Ok(json!({ "code": code, "expiresAt": expires_at as u64, "pairUrl": url, "pairQr": qr_data_url(&url).ok() }))
        }
        "remote:devices" => {
            let online = app.bridge.status().connected_device_ids;
            Ok(json!(app.store.load_devices().iter().map(|d| json!({ "id": d["id"], "name": d["name"], "createdAt": d["createdAt"], "lastSeenAt": d["lastSeenAt"], "pushEnabled": d.get("push").is_some(), "online": online.iter().any(|i| i == sv(d, "id")) })).collect::<Vec<_>>()))
        }
        "remote:revokeDevice" => {
            app.bridge.revoke_device(args.first().and_then(Value::as_str).unwrap_or(""));
            Ok(json!(true))
        }
        // A new CA and no devices: for a lost phone or a suspected leak
        "remote:resetTrust" => {
            app.bridge.stop();
            app.store.reset_all();
            app.store.audit("trust.reset", json!({}));
            apply_enabled(core);
            Ok(status(core))
        }
        _ => Err(format!("No handler registered for '{channel}'")),
    }
}

/// What the check scripts drive directly
pub fn test_call(name: &str, a: &[Value]) -> Result<Value, String> {
    let at = |i: usize| a.get(i).unwrap_or(&Value::Null);
    let s = |i: usize| at(i).as_str().unwrap_or("").to_string();
    let strs = |i: usize| -> Vec<String> { at(i).as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect() };
    let pair = |v: &Value| certs::CertPair { cert_pem: sv(v, "certPem").to_string(), key_pem: sv(v, "keyPem").to_string() };
    let pair_json = |p: certs::CertPair| json!({ "certPem": p.cert_pem, "keyPem": p.key_pem });
    match name {
        "isPrivateIPv4" => Ok(json!(certs::is_private_ipv4(&s(0)))),
        "isMeshConnection" => Ok(json!(certs::is_mesh_connection(&s(0), &s(1), &strs(2)))),
        "isAllowedOrigin" => Ok(json!(certs::is_allowed_origin(at(0).as_str(), &s(1), at(2) == &json!(true)))),
        "createCa" => certs::create_ca(&s(0), at(1) == &json!(true)).map(pair_json),
        "issueServerCert" => certs::issue_server_cert(&pair(at(0)), &strs(1), &strs(2)).map(pair_json),
        "certCovers" => Ok(json!(certs::cert_covers(&s(0), &strs(1), &strs(2)))),
        "caPermitsMesh" => Ok(json!(certs::ca_permits_mesh(&s(0)))),
        "fingerprint256" => Ok(json!(certs::fingerprint256(&s(0)))),
        "generateVapidKeys" => webpush::generate_vapid_keys().map(|k| json!({ "publicKey": k.public_key, "privateKeyPem": k.private_key_pem })),
        "encryptPayload" => webpush::encrypt_payload(at(0), &STANDARD.decode(s(1)).map_err(|e| e.to_string())?).map(|b| json!(STANDARD.encode(b))),
        "vapidAuthHeader" => webpush::vapid_auth_header(&s(0), &webpush::VapidKeys { public_key: sv(at(1), "publicKey").to_string(), private_key_pem: sv(at(1), "privateKeyPem").to_string() }, &s(2), at(3).as_i64().unwrap_or_else(|| chrono::Utc::now().timestamp())).map(Value::String),
        "workspacePath" => files::workspace_path(&s(0), &s(1)).map(|p| json!(crate::util::p(&p))),
        "listWorkspaceFiles" => files::list_workspace_files(&s(0), &s(1)).map(Value::Array),
        "readWorkspaceText" => files::read_workspace_text(&s(0), &s(1)).map(Value::String),
        "isPushEndpoint" => Ok(json!(server::is_push_endpoint(&s(0)))),
        _ => Err(format!("unknown remote call {name}")),
    }
}
