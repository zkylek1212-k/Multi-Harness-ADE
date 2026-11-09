// Update status for the UI. The host does the signed download and install; this tracks state, asks GitHub as a fallback,
// and for a fork build also reports the upstream project's version (a notice only, never installed).
use std::cmp::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use regex::Regex;
use serde_json::{json, Value};

use crate::core::Core;
use crate::util::{exists, now_ms, sv};

pub const UPSTREAM_REPO: &str = "zkylek1212-k/Multi-Harness-ADE";

fn version_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$").unwrap())
}

#[derive(PartialEq, Eq)]
enum Pre {
    Num(u64),
    Str(String),
}

fn parse(v: &str) -> Option<([u64; 3], Vec<Pre>)> {
    let m = version_re().captures(v.trim())?;
    let n = |i: usize| m[i].parse::<u64>().ok();
    let pre = m.get(4).map(|p| p.as_str().split('.').map(|x| x.parse().map(Pre::Num).unwrap_or_else(|_| Pre::Str(x.to_string()))).collect()).unwrap_or_default();
    Some(([n(1)?, n(2)?, n(3)?], pre))
}

/// Semver precedence; a version that does not parse compares equal
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let (Some(x), Some(y)) = (parse(a), parse(b)) else { return Ordering::Equal };
    if x.0 != y.0 {
        return x.0.cmp(&y.0);
    }
    // No prerelease ranks above the same version with one (1.0.0 > 1.0.0-fork.1)
    if x.1.is_empty() || y.1.is_empty() {
        return match (x.1.is_empty(), y.1.is_empty()) {
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            _ => Ordering::Equal,
        };
    }
    for i in 0..x.1.len().max(y.1.len()) {
        match (x.1.get(i), y.1.get(i)) {
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(p), Some(q)) if p == q => continue,
            (Some(Pre::Num(p)), Some(Pre::Num(q))) => return p.cmp(q),
            (Some(Pre::Num(_)), _) => return Ordering::Less,
            (_, Some(Pre::Num(_))) => return Ordering::Greater,
            (Some(Pre::Str(p)), Some(Pre::Str(q))) => return p.cmp(q),
        }
    }
    Ordering::Equal
}

pub fn is_valid_version(v: &str) -> bool {
    parse(v).is_some()
}

fn upstream_base_of(version: &str) -> Option<String> {
    Regex::new(r"^v?(\d+\.\d+\.\d+)-fork\.\d+$").unwrap().captures(version.trim()).map(|m| m[1].to_string())
}

fn is_repo_slug(s: &str) -> bool {
    Regex::new(r"^[A-Za-z0-9-]{1,39}/[A-Za-z0-9._-]{1,100}$").unwrap().is_match(s)
}

fn releases_url(repo: &str) -> String {
    format!("https://github.com/{repo}/releases")
}

/// The newest of the last releases, compared by hand: with fork prereleases, /latest is not always the largest
fn fetch_latest(repo: &str) -> Option<Value> {
    let res = crate::http::get(&format!("https://api.github.com/repos/{repo}/releases?per_page=20"), &[("Accept", "application/vnd.github+json")]).ok()?;
    if res.status != 200 {
        return None;
    }
    let list: Value = serde_json::from_slice(&res.body).ok()?;
    let mut best: Option<&Value> = None;
    for r in list.as_array()? {
        let v = sv(r, "tag_name").trim_start_matches('v');
        if r.get("draft") == Some(&json!(true)) || !is_valid_version(v) {
            continue;
        }
        if best.map(|b| compare_versions(sv(b, "tag_name").trim_start_matches('v'), v) == Ordering::Less).unwrap_or(true) {
            best = Some(r);
        }
    }
    let b = best?;
    let version = sv(b, "tag_name").trim_start_matches('v');
    let name = Some(sv(b, "name")).filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| format!("Version {version}"));
    let url = Some(sv(b, "html_url")).filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| releases_url(repo));
    Some(json!({ "version": version, "releaseName": name, "releaseNotes": sv(b, "body"), "releaseDate": b.get("published_at"), "downloadUrl": url }))
}

fn format_error(msg: &str) -> String {
    if msg.contains("latest.json") || msg.contains("404") {
        return "Online update manifest is not yet published for this release.".into();
    }
    let first = msg.lines().next().unwrap_or("");
    if first.encode_utf16().count() > 120 {
        format!("{}...", crate::util::js_prefix(first, 120))
    } else {
        first.to_string()
    }
}

/// What the host can do with a signed update. Off when a build has no signing key: then the release page is the only way.
pub trait UpdateBackend: Send + Sync {
    fn available(&self) -> bool;
    fn check(&self) -> Result<Option<Value>, String>;
    fn download(&self, progress: &(dyn Fn(u64, Option<u64>) + Send + Sync)) -> Result<(), String>;
    /// Runs the installer and ends the app
    fn install(&self) -> Result<(), String>;
}

pub struct Updater {
    status: Mutex<Value>,
    repo: String,
    is_fork: bool,
    backend: Mutex<Option<Arc<dyn UpdateBackend>>>,
}

impl Updater {
    pub fn new(core_version: &str, is_packaged: bool) -> Self {
        // A fork build bakes these in at compile time. An official build has none.
        let slug = |v: Option<&'static str>| v.filter(|s| is_repo_slug(s)).unwrap_or(UPSTREAM_REPO);
        let repo = slug(option_env!("WB_UPDATE_REPO")).to_string();
        let upstream = slug(option_env!("WB_UPSTREAM_REPO")).to_string();
        let declared = option_env!("WB_UPSTREAM_VERSION").filter(|v| is_valid_version(v)).map(str::to_string);
        let is_fork = repo.to_lowercase() != upstream.to_lowercase();
        let mut status = json!({
            "currentVersion": core_version, "isPackaged": is_packaged, "isInstalled": false, "checking": false,
            "updateAvailable": false, "updateDownloaded": false, "isDownloading": false, "channelRepo": repo,
        });
        if is_fork {
            status["upstream"] = json!({ "repo": upstream, "baseVersion": upstream_base_of(core_version).or(declared).unwrap_or_default(), "newer": false });
        }
        Self { status: Mutex::new(status), repo, is_fork, backend: Mutex::new(None) }
    }

    pub fn set_backend(&self, b: Arc<dyn UpdateBackend>) {
        *self.backend.lock().unwrap() = Some(b);
    }

    fn signed(&self) -> Option<Arc<dyn UpdateBackend>> {
        self.backend.lock().unwrap().clone().filter(|b| b.available())
    }

    fn patch(&self, core: &Core, f: impl FnOnce(&mut Value)) {
        let snapshot = {
            let mut s = self.status.lock().unwrap();
            f(&mut s);
            s.clone()
        };
        core.host.send_all("updater:statusChange", json!([snapshot]));
    }

    fn snapshot(&self) -> Value {
        self.status.lock().unwrap().clone()
    }
}

/// An installed copy, not a portable folder: installing over a portable one would leave its shortcut on the old version
fn is_installed(core: &Core) -> bool {
    // ponytail: "uninstall.exe" is the Tauri installer's name, the other is the old Electron one's
    if !core.info.is_packaged {
        return false;
    }
    let dir = crate::util::dirname(&core.info.exe_path);
    ["uninstall.exe", "Uninstall Agent Workbench.exe"].iter().any(|n| exists(&format!("{dir}\\{n}")))
}

fn check_upstream(core: &Core, u: &Updater) {
    let Some(up) = u.snapshot().get("upstream").cloned() else { return };
    let latest = fetch_latest(sv(&up, "repo"));
    u.patch(core, |s| {
        let mut next = up.clone();
        next["checkedAt"] = json!(now_ms() as u64);
        match latest {
            None => next["error"] = json!("Could not reach the upstream releases."),
            Some(l) => {
                next.as_object_mut().unwrap().shift_remove("error");
                next["latestVersion"] = l["version"].clone();
                next["latestReleaseDate"] = l["releaseDate"].clone();
                next["latestUrl"] = l["downloadUrl"].clone();
                let base = sv(&up, "baseVersion");
                next["newer"] = json!(!base.is_empty() && compare_versions(base, sv(&l, "version")) == Ordering::Less);
            }
        }
        s["upstream"] = next;
    });
}

fn perform_check(core: &Arc<Core>) -> Value {
    let u = &core.features().updater;
    if u.is_fork {
        let c = core.clone();
        std::thread::spawn(move || check_upstream(&c, &c.features().updater));
    }
    let installed = is_installed(core);
    u.patch(core, |s| {
        s["checking"] = json!(true);
        s.as_object_mut().unwrap().shift_remove("error");
        s["isInstalled"] = json!(installed);
    });
    // Signed path first, for an installed copy
    if core.info.is_packaged && installed {
        if let Some(b) = u.signed() {
            match b.check() {
                Ok(found) => {
                    u.patch(core, |s| {
                        s["checking"] = json!(false);
                        s["updateAvailable"] = json!(found.is_some());
                        if let Some(info) = found {
                            s["updateInfo"] = info;
                        }
                    });
                    return u.snapshot();
                }
                Err(e) => eprintln!("[updater] signed check failed, asking GitHub: {e}"),
            }
        }
    }
    // Portable copy, no signing key, or a missing manifest: the releases API
    let found = fetch_latest(&u.repo);
    u.patch(core, |s| {
        s["checking"] = json!(false);
        if let Some(rel) = found {
            s.as_object_mut().unwrap().shift_remove("error");
            let newer = compare_versions(sv(s, "currentVersion"), sv(&rel, "version")) == Ordering::Less;
            s["updateAvailable"] = json!(newer);
            if newer {
                s["updateInfo"] = rel;
            }
        } else if s.get("error").is_none() {
            s["updateAvailable"] = json!(false);
        }
    });
    u.snapshot()
}

pub fn call(core: &Arc<Core>, channel: &str, args: &[Value]) -> Result<Value, String> {
    let u = &core.features().updater;
    match channel {
        "updater:getStatus" => {
            let installed = is_installed(core);
            u.status.lock().unwrap()["isInstalled"] = json!(installed);
            Ok(u.snapshot())
        }
        "updater:check" => Ok(perform_check(core)),
        "updater:download" => {
            let fallback = || u.snapshot().pointer("/updateInfo/downloadUrl").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| releases_url(&u.repo));
            // No verified path: the user downloads it themselves. An unsigned payload is never run.
            let backend = if is_installed(core) { u.signed() } else { None };
            let Some(b) = backend else {
                core.host.open_url(&fallback());
                return Ok(json!(true));
            };
            u.patch(core, |s| s["isDownloading"] = json!(true));
            let core2 = core.clone();
            let progress = move |done: u64, total: Option<u64>| {
                core2.features().updater.patch(&core2, |s| {
                    s["isDownloading"] = json!(true);
                    let total = total.unwrap_or(0);
                    let percent = if total > 0 { (done as f64 / total as f64 * 1000.0).round() / 10.0 } else { 0.0 };
                    s["downloadProgress"] = json!({ "percent": percent, "bytesPerSecond": 0, "transferred": done, "total": total });
                });
            };
            match b.download(&progress) {
                Ok(()) => {
                    u.patch(core, |s| {
                        s["isDownloading"] = json!(false);
                        s["updateDownloaded"] = json!(true);
                    });
                    Ok(json!(true))
                }
                Err(e) => {
                    u.patch(core, |s| {
                        s["isDownloading"] = json!(false);
                        s["error"] = json!(format_error(&e));
                    });
                    Ok(json!(false))
                }
            }
        }
        "updater:install" => {
            if u.snapshot()["updateDownloaded"] == json!(true) {
                if let Some(b) = u.signed() {
                    b.install()?;
                }
            }
            Ok(Value::Null)
        }
        "updater:openRelease" => {
            let fallback = releases_url(&u.repo);
            let custom = args.first().and_then(Value::as_str).filter(|x| !x.is_empty()).map(str::to_string);
            let url = custom.or_else(|| u.snapshot().pointer("/updateInfo/downloadUrl").and_then(Value::as_str).map(str::to_string)).unwrap_or_else(|| fallback.clone());
            // http(s) only: the OS would act on file: or a custom scheme
            let lower = url.to_lowercase();
            core.host.open_url(if lower.starts_with("http://") || lower.starts_with("https://") { &url } else { &fallback });
            Ok(Value::Null)
        }
        _ => Err(format!("No handler registered for '{channel}'")),
    }
}

/// Five seconds after start, unless the user turned it off. It only ever notifies; downloading is the user's call.
pub fn start_startup_check(core: &Arc<Core>) {
    if !core.info.is_packaged {
        return;
    }
    let weak = Arc::downgrade(core);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(5));
        let Some(core) = weak.upgrade() else { return };
        if core.load_settings().get("autoCheckUpdates") != Some(&json!(false)) {
            perform_check(&core);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_precedence() {
        assert_eq!(compare_versions("1.0.0", "1.0.1"), Ordering::Less);
        assert_eq!(compare_versions("1.0.0", "1.0.0-fork.1"), Ordering::Greater);
        assert_eq!(compare_versions("0.1.41-fork.2", "0.1.41-fork.10"), Ordering::Less);
        assert_eq!(compare_versions("1.0.0-a", "1.0.0-a.1"), Ordering::Less);
        assert_eq!(compare_versions("1.0.0-1", "1.0.0-a"), Ordering::Less);
        assert_eq!(compare_versions("bad", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("v1.2.3", "1.2.3"), Ordering::Equal);
        assert_eq!(upstream_base_of("0.1.41-fork.2").as_deref(), Some("0.1.41"));
        assert!(is_repo_slug("a/b") && !is_repo_slug("a/b/c") && !is_repo_slug("https://x"));
    }
}
