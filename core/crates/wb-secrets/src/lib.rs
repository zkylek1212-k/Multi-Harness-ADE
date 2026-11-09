//! OS-backed encryption. Windows only for now: the Linux app stays on Electron (DEC-011).
//! The blob is tied to the current Windows user, the same protection level safeStorage gives.

#[cfg(windows)]
mod imp {
    use std::io;
    use windows_sys::Win32::Foundation::{LocalFree, HLOCAL};
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
    }

    fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        unsafe { LocalFree(out.pbData as HLOCAL) };
        v
    }

    pub fn protect(plain: &[u8]) -> io::Result<Vec<u8>> {
        let input = blob(plain);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = unsafe { CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(take(out))
    }

    pub fn unprotect(sealed: &[u8]) -> io::Result<Vec<u8>> {
        let input = blob(sealed);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = unsafe { CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(take(out))
    }
}

#[cfg(not(windows))]
mod imp {
    use std::io;
    pub fn protect(_: &[u8]) -> io::Result<Vec<u8>> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "only Windows is supported"))
    }
    pub fn unprotect(_: &[u8]) -> io::Result<Vec<u8>> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "only Windows is supported"))
    }
}

pub use imp::{protect, unprotect};

/// Read a value Electron's safeStorage wrote: "v10" + 12-byte nonce + AES-256-GCM, with the key DPAPI-sealed in
/// Local State. Lets an install that never ran the migration keep its secrets after Electron is gone.
pub fn safe_storage_decrypt(local_state: &std::path::Path, sealed: &[u8]) -> std::io::Result<Vec<u8>> {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Key, Nonce};
    use base64::{engine::general_purpose::STANDARD, Engine};
    let bad = |m: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, m.to_string());
    let body = sealed.strip_prefix(b"v10").filter(|b| b.len() > 12 + 16).ok_or_else(|| bad("not a safeStorage value"))?;
    let state: serde_json::Value = serde_json::from_slice(&std::fs::read(local_state)?).map_err(|_| bad("Local State is not JSON"))?;
    let key_b64 = state.pointer("/os_crypt/encrypted_key").and_then(|v| v.as_str()).ok_or_else(|| bad("no os_crypt key"))?;
    let sealed_key = STANDARD.decode(key_b64).map_err(|_| bad("bad os_crypt key"))?;
    let key = unprotect(sealed_key.strip_prefix(b"DPAPI").ok_or_else(|| bad("bad os_crypt key"))?)?;
    if key.len() != 32 {
        return Err(bad("bad os_crypt key"));
    }
    let (nonce, ct) = body.split_at(12);
    Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key)).decrypt(Nonce::from_slice(nonce), ct).map_err(|_| bad("value does not decrypt"))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn round_trip_including_empty_and_binary() {
        for plain in [&b"-----BEGIN PRIVATE KEY-----\nabc\n"[..], b"", &[0u8, 255, 1, 2][..], "中文".as_bytes()] {
            let sealed = protect(plain).unwrap();
            assert_ne!(sealed, plain);
            assert_eq!(unprotect(&sealed).unwrap(), plain);
        }
    }

    #[test]
    fn reads_a_safe_storage_value() {
        use aes_gcm::aead::{Aead, KeyInit};
        use aes_gcm::{Aes256Gcm, Key, Nonce};
        use base64::{engine::general_purpose::STANDARD, Engine};
        let key = [7u8; 32];
        let mut sealed_key = b"DPAPI".to_vec();
        sealed_key.extend(protect(&key).unwrap());
        let dir = std::env::temp_dir().join(format!("wb-ss-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = dir.join("Local State");
        std::fs::write(&state, serde_json::json!({ "os_crypt": { "encrypted_key": STANDARD.encode(sealed_key) } }).to_string()).unwrap();
        let nonce = [9u8; 12];
        let ct = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key)).encrypt(Nonce::from_slice(&nonce), &b"token"[..]).unwrap();
        let mut value = b"v10".to_vec();
        value.extend_from_slice(&nonce);
        value.extend(ct);
        assert_eq!(safe_storage_decrypt(&state, &value).unwrap(), b"token");
        *value.last_mut().unwrap() ^= 1;
        assert!(safe_storage_decrypt(&state, &value).is_err());
        assert!(safe_storage_decrypt(&state, b"plain").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tampered_blob_is_rejected() {
        let mut sealed = protect(b"secret").unwrap();
        let mid = sealed.len() / 2;
        sealed[mid] ^= 0xff;
        assert!(unprotect(&sealed).is_err());
        assert!(unprotect(b"not a blob").is_err());
    }
}
