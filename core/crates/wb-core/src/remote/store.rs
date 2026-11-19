// Remote control's local state under userData/remote/, in the same files the Electron app wrote:
//   config.json   switch and port
//   devices.json  paired devices (only a token's SHA-256)
//   secrets.json  CA, server and VAPID private keys, sealed with the OS key
//   audit.log     pairing, connections, terminals opened and closed; never what was typed
//   guests.json   colleagues invited into one channel (cowork-agui-implement.md §13): only a token's SHA-256
//   invites.json  their one-time invite codes, also hashed
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use base64::Engine;
use rand::RngCore;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::conn::{protect_b64, unprotect_b64};
use crate::util::{iso, js_len, js_prefix, now_ms, sv, uuid};

pub const DEFAULT_PORT: u16 = 47600;
/// An invite link lasts a week; a channel takes ten colleagues (§13.2)
pub const INVITE_TTL_MS: f64 = 7.0 * 24.0 * 3_600_000.0;
pub const MAX_GUESTS: usize = 10;

#[derive(Clone, Debug)]
pub struct Config {
    pub enabled: bool,
    pub port: u16,
    pub allow_mesh: bool,
}

#[derive(Clone)]
pub struct StoredCerts {
    pub ca_cert: String,
    pub ca_key: String,
    pub server_cert: Option<String>,
    pub server_key: Option<String>,
}

pub struct Store {
    dir: PathBuf,
    local_state: PathBuf,
    migrated: AtomicBool,
    /// devices.json changes as read, edit, write. Without this a phone disconnecting while another is revoked brings that one back.
    devices: Mutex<()>,
}

impl Store {
    pub fn new(data_dir: &std::path::Path) -> Self {
        Self { dir: data_dir.join("remote"), local_state: data_dir.join("Local State"), migrated: AtomicBool::new(false), devices: Mutex::new(()) }
    }

    fn read(&self, name: &str) -> Option<Value> {
        serde_json::from_slice(&fs::read(self.dir.join(name)).ok()?).ok()
    }

    fn write(&self, name: &str, value: &Value) -> Result<(), String> {
        fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let p = self.dir.join(name);
        let tmp = self.dir.join(format!("{name}.tmp"));
        fs::write(&tmp, serde_json::to_string_pretty(value).unwrap()).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &p).map_err(|e| e.to_string())
    }

    fn secrets(&self) -> Map<String, Value> {
        self.read("secrets.json").and_then(|v| v.as_object().cloned()).unwrap_or_default()
    }

    fn safe_decrypt(&self, b64: &str) -> Result<String, String> {
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| e.to_string())?;
        let plain = wb_secrets::safe_storage_decrypt(&self.local_state, &bytes).map_err(|e| e.to_string())?;
        String::from_utf8(plain).map_err(|e| e.to_string())
    }

    /// Reseals what Electron's safeStorage wrote. A failure keeps the old values and tries again next start; never a new CA.
    fn migrate_once(&self) {
        if self.migrated.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut s = self.secrets();
        let mut count = 0;
        let run = (|| -> Result<(), String> {
            for (_, v) in s.iter_mut() {
                if v.get("enc").and_then(Value::as_str) == Some("safe") {
                    let plain = self.safe_decrypt(sv(v, "data"))?;
                    *v = json!({ "enc": "dpapi", "data": protect_b64(&plain)? });
                    count += 1;
                }
            }
            Ok(())
        })();
        match run {
            Ok(()) if count > 0 => {
                if self.write("secrets.json", &Value::Object(s)).is_ok() {
                    self.audit("secrets.migrated", json!({ "count": count }));
                }
            }
            Ok(()) => {}
            Err(e) => self.audit("secrets.migrate.failed", json!({ "message": js_prefix(&e, 200) })),
        }
    }

    fn seal(&self, plain: &str) -> Result<Value, String> {
        Ok(json!({ "enc": "dpapi", "data": protect_b64(plain)? }))
    }

    /// Present but undecryptable is an error, not "no key": treated as missing it would create a new CA and every phone would lose trust
    fn unseal(&self, v: Option<&Value>) -> Result<Option<String>, String> {
        let Some(v) = v.filter(|v| v.is_object()) else { return Ok(None) };
        let data = sv(v, "data");
        let r = match sv(v, "enc") {
            "dpapi" => unprotect_b64(data),
            "safe" => self.safe_decrypt(data),
            _ => Ok(data.to_string()),
        };
        r.map(Some).map_err(|e| format!("Could not decrypt the remote control keys: {}", js_prefix(&e, 200)))
    }

    pub fn load_config(&self) -> Config {
        let c = self.read("config.json").unwrap_or(Value::Null);
        // The bridge uses port, port+1 (CA page) and port+2 (preview), so the top leaves room for three
        let port = c.get("port").and_then(Value::as_u64).filter(|p| *p > 1024 && *p < 65533).unwrap_or(DEFAULT_PORT as u64) as u16;
        Config { enabled: c.get("enabled").is_some_and(crate::settings::truthy), port, allow_mesh: c.get("allowMesh").is_some_and(crate::settings::truthy) }
    }

    pub fn save_config(&self, enabled: Option<bool>, port: Option<&Value>, allow_mesh: Option<bool>) {
        let cur = self.load_config();
        let next = json!({ "enabled": enabled.unwrap_or(cur.enabled), "port": port.cloned().unwrap_or(json!(cur.port)), "allowMesh": allow_mesh.unwrap_or(cur.allow_mesh) });
        let _ = self.write("config.json", &next);
    }

    pub fn load_devices(&self) -> Vec<Value> {
        self.read("devices.json").and_then(|v| v.as_array().cloned()).unwrap_or_default()
    }

    fn save_devices(&self, list: &[Value]) {
        let _ = self.write("devices.json", &json!(list));
    }

    pub fn find_device_by_token(&self, token: &str) -> Option<Value> {
        if token.is_empty() || js_len(token) > 200 {
            return None;
        }
        let want = hash_token(token);
        self.load_devices().into_iter().find(|d| ct_eq(sv(d, "tokenHash").as_bytes(), want.as_bytes()))
    }

    pub fn add_device(&self, name: &str) -> (Value, String) {
        let mut raw = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut raw);
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
        let short = js_prefix(name, 60);
        let now = now_ms() as u64;
        let device = json!({ "id": uuid(), "name": if short.is_empty() { "iPhone" } else { &short }, "tokenHash": hash_token(&token), "createdAt": now, "lastSeenAt": now });
        let _g = self.devices.lock().unwrap();
        let mut list = self.load_devices();
        list.push(device.clone());
        self.save_devices(&list);
        (device, token)
    }

    /// A null value removes the key
    pub fn update_device(&self, id: &str, key: &str, value: Value) {
        let _g = self.devices.lock().unwrap();
        let mut list = self.load_devices();
        for d in list.iter_mut().filter(|d| sv(d, "id") == id) {
            if let Some(o) = d.as_object_mut() {
                if value.is_null() {
                    o.shift_remove(key);
                } else {
                    o.insert(key.to_string(), value.clone());
                }
            }
        }
        self.save_devices(&list);
    }

    // ---------- guests (§13) ----------

    fn random_token() -> String {
        let mut raw = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut raw);
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
    }

    pub fn load_guests(&self) -> Vec<Value> {
        self.read("guests.json").and_then(|v| v.as_array().cloned()).unwrap_or_default()
    }

    /// A one-time code for this channel only. Expired codes are dropped on the way.
    pub fn add_invite(&self, channel: &str, repo: &str) -> Result<(String, f64), String> {
        let _g = self.devices.lock().unwrap();
        if self.load_guests().iter().filter(|g| sv(g, "channel") == channel).count() >= MAX_GUESTS {
            return Err(format!("A channel takes at most {MAX_GUESTS} colleagues"));
        }
        let code = Self::random_token();
        let expires = now_ms() + INVITE_TTL_MS;
        let mut list: Vec<Value> = self.read("invites.json").and_then(|v| v.as_array().cloned()).unwrap_or_default();
        list.retain(|i| i["expiresAt"].as_f64().unwrap_or(0.0) > now_ms());
        list.push(json!({ "codeHash": hash_token(&code), "channel": channel, "repo": repo, "expiresAt": expires }));
        self.write("invites.json", &json!(list))?;
        Ok((code, expires))
    }

    /// Spends an invite: a guest for its channel and the token their browser keeps. A used, expired or unknown code
    /// gives nothing.
    pub fn redeem_invite(&self, code: &str, name: &str) -> Option<(Value, String)> {
        if code.is_empty() || js_len(code) > 200 {
            return None;
        }
        let _g = self.devices.lock().unwrap();
        let want = hash_token(code);
        let mut list: Vec<Value> = self.read("invites.json").and_then(|v| v.as_array().cloned()).unwrap_or_default();
        let i = list.iter().position(|x| ct_eq(sv(x, "codeHash").as_bytes(), want.as_bytes()))?;
        let inv = list.remove(i);
        let _ = self.write("invites.json", &json!(list));
        let channel = sv(&inv, "channel").to_string();
        let mut guests = self.load_guests();
        if inv["expiresAt"].as_f64().unwrap_or(0.0) < now_ms() || guests.iter().filter(|g| sv(g, "channel") == channel).count() >= MAX_GUESTS {
            return None;
        }
        let token = Self::random_token();
        let now = now_ms() as u64;
        let clean: String = name.chars().filter(|c| !c.is_control() && *c != ':').collect();
        let name = js_prefix(clean.trim(), 40);
        let guest = json!({ "id": uuid(), "channel": channel, "repo": sv(&inv, "repo"), "name": if name.is_empty() { "Guest" } else { &name }, "tokenHash": hash_token(&token), "createdAt": now, "lastSeenAt": now });
        guests.push(guest.clone());
        let _ = self.write("guests.json", &json!(guests));
        Some((guest, token))
    }

    pub fn find_guest_by_token(&self, token: &str) -> Option<Value> {
        if token.is_empty() || js_len(token) > 200 {
            return None;
        }
        let want = hash_token(token);
        self.load_guests().into_iter().find(|g| ct_eq(sv(g, "tokenHash").as_bytes(), want.as_bytes()))
    }

    pub fn touch_guest(&self, id: &str) {
        let _g = self.devices.lock().unwrap();
        let mut list = self.load_guests();
        for g in list.iter_mut().filter(|g| sv(g, "id") == id) {
            g["lastSeenAt"] = json!(now_ms() as u64);
        }
        let _ = self.write("guests.json", &json!(list));
    }

    pub fn remove_guest(&self, id: &str) -> bool {
        let _g = self.devices.lock().unwrap();
        let list = self.load_guests();
        let kept: Vec<Value> = list.iter().filter(|g| sv(g, "id") != id).cloned().collect();
        let gone = kept.len() != list.len();
        let _ = self.write("guests.json", &json!(kept));
        gone
    }

    pub fn remove_device(&self, id: &str) {
        let _g = self.devices.lock().unwrap();
        let list: Vec<Value> = self.load_devices().into_iter().filter(|d| sv(d, "id") != id).collect();
        self.save_devices(&list);
    }

    /// Only the public part, which needs no key to read
    pub fn ca_cert(&self) -> Option<String> {
        self.secrets().get("caCert").and_then(Value::as_str).map(str::to_string)
    }

    pub fn load_certs(&self) -> Result<Option<StoredCerts>, String> {
        self.migrate_once();
        let s = self.secrets();
        let ca_key = self.unseal(s.get("caKey"))?;
        let (Some(ca_cert), Some(ca_key)) = (s.get("caCert").and_then(Value::as_str), ca_key.filter(|k| !k.is_empty())) else { return Ok(None) };
        // A server certificate can always be issued again: not reading its key only costs one more issue
        let server_key = self.unseal(s.get("serverKey")).ok().flatten().filter(|k| !k.is_empty());
        let server_cert = server_key.as_ref().and_then(|_| s.get("serverCert").and_then(Value::as_str)).map(str::to_string);
        Ok(Some(StoredCerts { ca_cert: ca_cert.to_string(), ca_key, server_cert, server_key }))
    }

    pub fn save_certs(&self, c: &StoredCerts) -> Result<(), String> {
        let mut s = self.secrets();
        s.insert("caCert".into(), json!(c.ca_cert));
        s.insert("caKey".into(), self.seal(&c.ca_key)?);
        s.shift_remove("serverCert");
        s.shift_remove("serverKey");
        if let (Some(cert), Some(key)) = (&c.server_cert, &c.server_key) {
            s.insert("serverCert".into(), json!(cert));
            s.insert("serverKey".into(), self.seal(key)?);
        }
        self.write("secrets.json", &Value::Object(s))
    }

    pub fn load_vapid(&self) -> Result<Option<super::webpush::VapidKeys>, String> {
        self.migrate_once();
        let s = self.secrets();
        let private_key_pem = self.unseal(s.get("vapidPrivate"))?;
        Ok(match (s.get("vapidPublic").and_then(Value::as_str), private_key_pem) {
            (Some(public_key), Some(private_key_pem)) if !private_key_pem.is_empty() => Some(super::webpush::VapidKeys { public_key: public_key.to_string(), private_key_pem }),
            _ => None,
        })
    }

    pub fn save_vapid(&self, v: &super::webpush::VapidKeys) -> Result<(), String> {
        let mut s = self.secrets();
        s.insert("vapidPublic".into(), json!(v.public_key));
        s.insert("vapidPrivate".into(), self.seal(&v.private_key_pem)?);
        self.write("secrets.json", &Value::Object(s))
    }

    /// Reset trust: drops the CA and every paired device. The old certificate on a phone is for its owner to remove.
    pub fn reset_all(&self) {
        for f in ["secrets.json", "devices.json", "guests.json", "invites.json"] {
            let _ = fs::remove_file(self.dir.join(f));
        }
    }

    /// Failing to log never affects the feature
    pub fn audit(&self, event: &str, detail: Value) {
        let _ = fs::create_dir_all(&self.dir);
        let p = self.dir.join("audit.log");
        if fs::metadata(&p).is_ok_and(|m| m.len() > 1024 * 1024) {
            let _ = fs::rename(&p, self.dir.join("audit.log.1"));
        }
        let mut row = json!({ "t": iso(now_ms()), "event": event });
        if let (Some(o), Some(d)) = (row.as_object_mut(), detail.as_object()) {
            for (k, v) in d {
                o.insert(k.clone(), v.clone());
            }
        }
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&p) {
            let _ = writeln!(f, "{row}");
        }
    }
}

pub fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!("wb-remote-{}", uuid()));
        (Store::new(&dir), dir)
    }

    #[test]
    fn config_falls_back_on_a_bad_port() {
        let (s, dir) = store();
        assert_eq!(s.load_config().port, DEFAULT_PORT);
        for bad in [json!(80), json!(65533), json!("x"), json!(1.5)] {
            s.save_config(None, Some(&bad), None);
            assert_eq!(s.load_config().port, DEFAULT_PORT, "{bad}");
        }
        s.save_config(Some(true), Some(&json!(50000)), None);
        assert!(s.load_config().enabled && s.load_config().port == 50000);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn an_invite_is_used_once_and_only_its_hash_is_kept() {
        let (s, dir) = store();
        let (code, exp) = s.add_invite("ch1", "C:/r").unwrap();
        assert!(exp > now_ms());
        let raw = fs::read_to_string(dir.join("remote").join("invites.json")).unwrap();
        assert!(!raw.contains(&code), "the code itself is never written");
        assert!(s.redeem_invite("wrong", "Ann").is_none());
        let (g, token) = s.redeem_invite(&code, "Ann:\u{1b}x").unwrap();
        assert_eq!((sv(&g, "channel"), sv(&g, "name")), ("ch1", "Annx"), "the name loses colons and control characters");
        assert!(s.redeem_invite(&code, "Bob").is_none(), "one use");
        assert_eq!(sv(&s.find_guest_by_token(&token).unwrap(), "id"), sv(&g, "id"));
        assert!(!fs::read_to_string(dir.join("remote").join("guests.json")).unwrap().contains(&token));
        assert!(s.remove_guest(sv(&g, "id")));
        assert!(s.find_guest_by_token(&token).is_none(), "revoked at once");
        for _ in 0..MAX_GUESTS {
            let (c, _) = s.add_invite("ch2", "r").unwrap();
            s.redeem_invite(&c, "x").unwrap();
        }
        assert!(s.add_invite("ch2", "r").is_err(), "ten per channel");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn only_the_token_hash_is_stored() {
        let (s, dir) = store();
        let (d, token) = s.add_device("phone");
        let saved = fs::read_to_string(dir.join("remote/devices.json")).unwrap();
        assert!(!saved.contains(&token));
        assert_eq!(s.find_device_by_token(&token).unwrap()["id"], d["id"]);
        assert!(s.find_device_by_token("wrong").is_none() && s.find_device_by_token("").is_none());
        assert!(s.find_device_by_token(&"x".repeat(201)).is_none());
        s.update_device(sv(&d, "id"), "push", json!({ "endpoint": "e" }));
        assert!(s.load_devices()[0].get("push").is_some());
        s.update_device(sv(&d, "id"), "push", Value::Null);
        assert!(s.load_devices()[0].get("push").is_none());
        s.remove_device(sv(&d, "id"));
        assert!(s.find_device_by_token(&token).is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn an_unreadable_key_is_an_error_not_a_missing_ca() {
        let (s, dir) = store();
        fs::create_dir_all(dir.join("remote")).unwrap();
        fs::write(dir.join("remote/secrets.json"), json!({ "caCert": "c", "caKey": { "enc": "dpapi", "data": "AAAA" } }).to_string()).unwrap();
        assert!(s.load_certs().is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn keys_are_sealed_on_disk_and_round_trip() {
        let (s, dir) = store();
        s.save_certs(&StoredCerts { ca_cert: "CERT".into(), ca_key: "SECRET-KEY".into(), server_cert: Some("LEAF".into()), server_key: Some("LEAF-KEY".into()) }).unwrap();
        let raw = fs::read_to_string(dir.join("remote/secrets.json")).unwrap();
        assert!(!raw.contains("SECRET-KEY") && !raw.contains("LEAF-KEY"));
        let c = s.load_certs().unwrap().unwrap();
        assert_eq!((c.ca_key.as_str(), c.server_key.as_deref()), ("SECRET-KEY", Some("LEAF-KEY")));
        let _ = fs::remove_dir_all(dir);
    }
}
