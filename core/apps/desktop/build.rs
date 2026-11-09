/// The memory plugin goes along as a resource, without its tests. tauri.conf.json has no way to exclude, hence this copy.
fn copy_plugin(from: &std::path::Path, to: &std::path::Path) {
    let _ = std::fs::create_dir_all(to);
    for e in std::fs::read_dir(from).expect("plugin folder").flatten() {
        let name = e.file_name();
        let t = to.join(&name);
        if e.path().is_dir() {
            if name != "tests" {
                copy_plugin(&e.path(), &t);
            }
        } else {
            std::fs::copy(e.path(), t).expect("copy plugin file");
        }
    }
}

fn main() {
    let plugin = std::path::Path::new("../../../plugins/shared-project-memory");
    println!("cargo:rerun-if-changed={}", plugin.display());
    let target = std::path::Path::new("resources/plugins/shared-project-memory");
    let _ = std::fs::remove_dir_all(target);
    copy_plugin(plugin, target);
    // Listing the commands puts them under capabilities, so a page in a child webview cannot reach them.
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
        "ipc",
        "ipc_send",
        "subscribe",
        "window_control",
        "open_external",
        "legacy_local_storage",
        "webview_create",
        "webview_close",
        "webview_navigate",
        "webview_bounds",
        "webview_action",
        // Only has a handler in debug and e2e builds
        "e2e_result",
    ])))
    .expect("tauri build");
}
