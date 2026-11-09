// Signed updates. The public key is baked in at build time; a build without one has no backend at all, and the UI points at the
// releases page instead. An unsigned payload is never run.
use std::sync::Mutex;

use serde_json::{json, Value};
use tauri::{AppHandle, Url};
use tauri_plugin_updater::{Update, UpdaterExt};
use wb_core::updater::UpdateBackend;

pub const PUBLIC_KEY: Option<&str> = option_env!("WB_UPDATER_PUBKEY");

pub struct SignedUpdates {
    app: AppHandle,
    found: Mutex<Option<Update>>,
    payload: Mutex<Option<Vec<u8>>>,
    /// Runs before the installer ends the process
    before_install: Box<dyn Fn() + Send + Sync>,
}

impl SignedUpdates {
    pub fn new(app: AppHandle, before_install: Box<dyn Fn() + Send + Sync>) -> Self {
        Self { app, found: Mutex::default(), payload: Mutex::default(), before_install }
    }

    fn endpoint() -> Result<Url, String> {
        let repo = option_env!("WB_UPDATE_REPO").unwrap_or(wb_core::updater::UPSTREAM_REPO);
        Url::parse(&format!("https://github.com/{repo}/releases/latest/download/latest.json")).map_err(|e| e.to_string())
    }
}

impl UpdateBackend for SignedUpdates {
    fn available(&self) -> bool {
        PUBLIC_KEY.is_some()
    }

    fn check(&self) -> Result<Option<Value>, String> {
        let updater = self.app.updater_builder().endpoints(vec![Self::endpoint()?]).map_err(|e| e.to_string())?.build().map_err(|e| e.to_string())?;
        let update = tauri::async_runtime::block_on(updater.check()).map_err(|e| e.to_string())?;
        let info = update.as_ref().map(|u| json!({ "version": u.version, "releaseDate": u.date.map(|d| d.to_string()), "releaseNotes": u.body, "downloadUrl": u.download_url.as_str() }));
        *self.found.lock().unwrap() = update;
        *self.payload.lock().unwrap() = None;
        Ok(info)
    }

    fn download(&self, progress: &(dyn Fn(u64, Option<u64>) + Send + Sync)) -> Result<(), String> {
        let update = self.found.lock().unwrap().clone().ok_or("no update to download")?;
        let done = std::sync::atomic::AtomicU64::new(0);
        // The plugin verifies the signature here; a bad one is an error and nothing is kept
        let bytes = tauri::async_runtime::block_on(update.download(
            |chunk, total| {
                let so_far = done.fetch_add(chunk as u64, std::sync::atomic::Ordering::SeqCst) + chunk as u64;
                progress(so_far, total);
            },
            || {},
        ))
        .map_err(|e| e.to_string())?;
        *self.payload.lock().unwrap() = Some(bytes);
        Ok(())
    }

    fn install(&self) -> Result<(), String> {
        let update = self.found.lock().unwrap().clone().ok_or("no update to install")?;
        let bytes = self.payload.lock().unwrap().take().ok_or("nothing downloaded")?;
        (self.before_install)();
        // On Windows this starts the installer and ends this process
        update.install(bytes).map_err(|e| e.to_string())
    }
}
