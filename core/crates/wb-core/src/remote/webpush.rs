// Web Push sender: RFC 8030 delivery, RFC 8291 aes128gcm encryption, RFC 8292 VAPID.
// The payload is encrypted to the phone's key first, so the push service only ever sees ciphertext.
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use ring::{aead, agreement, hmac};
use serde_json::{json, Value};
use url::Url;

use crate::util::sv;

#[derive(Clone)]
pub struct VapidKeys {
    /// Uncompressed P-256 public key (65 bytes) as base64url, what a browser takes as applicationServerKey
    pub public_key: String,
    /// PKCS#8 PEM
    pub private_key_pem: String,
}

fn b64u(b: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(b)
}

fn unb64u(s: &str) -> Result<Vec<u8>, String> {
    URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')).map_err(|e| e.to_string())
}

pub fn generate_vapid_keys() -> Result<VapidKeys, String> {
    let rng = SystemRandom::new();
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).map_err(|e| e.to_string())?;
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng).map_err(|e| e.to_string())?;
    let b64 = STANDARD.encode(pkcs8.as_ref());
    let lines: Vec<&str> = b64.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap()).collect();
    Ok(VapidKeys { public_key: b64u(pair.public_key().as_ref()), private_key_pem: format!("-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n", lines.join("\n")) })
}

fn hmac256(key: &[u8], data: &[u8]) -> Vec<u8> {
    hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), data).as_ref().to_vec()
}

/// RFC 8291: the complete body, header included. One record, plaintext then 0x02 as the last-record delimiter.
pub fn encrypt_payload(sub: &Value, plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let ua_public = unb64u(sv(&sub["keys"], "p256dh"))?;
    let auth_secret = unb64u(sv(&sub["keys"], "auth"))?;
    let rng = SystemRandom::new();
    let private = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).map_err(|e| e.to_string())?;
    let as_public = private.compute_public_key().map_err(|e| e.to_string())?.as_ref().to_vec();
    let ecdh = agreement::agree_ephemeral(private, &agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, &ua_public), |s| s.to_vec()).map_err(|_| "bad subscription key".to_string())?;
    let mut salt = [0u8; 16];
    rng.fill(&mut salt).map_err(|e| e.to_string())?;

    let prk_key = hmac256(&auth_secret, &ecdh);
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(&ua_public);
    key_info.extend_from_slice(&as_public);
    key_info.push(1);
    let ikm = hmac256(&prk_key, &key_info);
    let prk = hmac256(&salt, &ikm);
    let cek = hmac256(&prk, b"Content-Encoding: aes128gcm\0\x01");
    let nonce = hmac256(&prk, b"Content-Encoding: nonce\0\x01");

    let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &cek[..16]).map_err(|e| e.to_string())?);
    let mut body = plaintext.to_vec();
    body.push(2);
    key.seal_in_place_append_tag(aead::Nonce::try_assume_unique_for_key(&nonce[..12]).map_err(|e| e.to_string())?, aead::Aad::empty(), &mut body).map_err(|e| e.to_string())?;

    let mut out = salt.to_vec();
    out.extend_from_slice(&4096u32.to_be_bytes());
    out.push(as_public.len() as u8);
    out.extend_from_slice(&as_public);
    out.extend_from_slice(&body);
    Ok(out)
}

pub fn vapid_auth_header(endpoint: &str, keys: &VapidKeys, subject: &str, now_sec: i64) -> Result<String, String> {
    let aud = Url::parse(endpoint).map_err(|e| e.to_string())?.origin().ascii_serialization();
    let header = b64u(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = b64u(json!({ "aud": aud, "exp": now_sec + 12 * 3600, "sub": subject }).to_string().as_bytes());
    let der = super::certs::cert_der(&keys.private_key_pem)?;
    let rng = SystemRandom::new();
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der, &rng).map_err(|e| e.to_string())?;
    let sig = pair.sign(&rng, format!("{header}.{claims}").as_bytes()).map_err(|e| e.to_string())?;
    Ok(format!("vapid t={header}.{claims}.{}, k={}", b64u(sig.as_ref()), keys.public_key))
}

pub struct PushResult {
    pub status: u16,
    /// 404 or 410: the subscription is gone, so the caller should drop it
    pub gone: bool,
}

/// Blocks. Failure to deliver is an ordinary result; nothing here takes the bridge down.
pub fn send_push(sub: &Value, payload: &Value, keys: &VapidKeys, subject: &str) -> PushResult {
    let failed = PushResult { status: 0, gone: false };
    let endpoint = sv(sub, "endpoint");
    let Ok(body) = encrypt_payload(sub, payload.to_string().as_bytes()) else { return failed };
    let Ok(auth) = vapid_auth_header(endpoint, keys, subject, chrono::Utc::now().timestamp()) else { return failed };
    let headers = [("Content-Type", "application/octet-stream"), ("Content-Encoding", "aes128gcm"), ("TTL", "300"), ("Urgency", "high"), ("Authorization", auth.as_str())];
    match crate::http::post_bytes_no_redirect(endpoint, &headers, &body, std::time::Duration::from_secs(15)) {
        Ok(status) => PushResult { status, gone: status == 404 || status == 410 },
        Err(_) => failed,
    }
}
