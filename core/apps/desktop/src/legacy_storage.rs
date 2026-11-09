// Layout and preferences from the Electron build. They live in Chromium's LevelDB under the old file:// origin.
use std::path::Path;

use serde_json::{json, Map, Value};

const ORIGIN_PREFIX: &[u8] = b"_file://\0\x01";

fn decode(value: &[u8]) -> Option<String> {
    match value.split_first()? {
        (0, rest) => {
            let units: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            String::from_utf16(&units).ok()
        }
        (1, rest) => Some(rest.iter().map(|b| *b as char).collect()),
        _ => None,
    }
}

fn wanted(key: &str) -> bool {
    key.starts_with("wb-") || key.starts_with("agent-workbench:")
}

fn read_db(dir: &Path) -> Result<Map<String, Value>, String> {
    // A copy: this never locks or writes to what the old app might still be using
    let tmp = std::env::temp_dir().join(format!("wb-legacy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    for e in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
        if e.file_type().map(|t| t.is_file()).unwrap_or(false) && e.file_name() != "LOCK" {
            let _ = std::fs::copy(e.path(), tmp.join(e.file_name()));
        }
    }
    let mut out = Map::new();
    let result = (|| -> Result<(), String> {
        let mut db = rusty_leveldb::DB::open(&tmp, rusty_leveldb::Options::default()).map_err(|e| e.to_string())?;
        let mut it = db.new_iter().map_err(|e| e.to_string())?;
        use rusty_leveldb::LdbIterator;
        while let Some((k, v)) = it.next() {
            let Some(name) = k.strip_prefix(ORIGIN_PREFIX) else { continue };
            let Ok(name) = std::str::from_utf8(name) else { continue };
            if wanted(name) {
                if let Some(text) = decode(&v) {
                    out.insert(name.to_string(), json!(text));
                }
            }
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result.map(|_| out)
}

pub fn read(data_dir: &Path) -> Value {
    let dir = data_dir.join("Local Storage").join("leveldb");
    if !dir.exists() {
        return json!({});
    }
    match read_db(&dir) {
        Ok(m) => Value::Object(m),
        Err(e) => {
            eprintln!("[legacy storage] {e}");
            json!({})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_both_encodings() {
        assert_eq!(decode(&[1, b'a', b'b']).unwrap(), "ab");
        let mut v = vec![0u8];
        v.extend("中".encode_utf16().flat_map(|u| u.to_le_bytes()));
        assert_eq!(decode(&v).unwrap(), "中");
        assert!(decode(&[]).is_none());
    }

    #[test]
    fn only_our_keys() {
        assert!(wanted("wb-layout"));
        assert!(wanted("agent-workbench:dashboard-x"));
        assert!(!wanted("something-else"));
    }
}
