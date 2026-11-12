//! The whole backend behind one call, independent of any UI framework.
pub mod approval_hook;
pub mod bgtasks;
pub mod canvas;
pub mod conn;
pub mod core;
pub mod custom_agent;
pub mod dashboard;
pub mod ext;
pub mod files;
pub mod git_ui;
pub mod http;
pub mod manifest;
pub mod memsync;
pub mod paths;
pub mod remote;
pub mod pty;
pub mod session_scan;
pub mod settings;
pub mod updater;
pub mod usage;
pub mod util;
pub mod worktree;

use std::sync::Arc;

use serde_json::Value;

pub use crate::core::{AppInfo, Core, Host};

/// Long-lived feature state that needs the finished Core
pub struct Features {
    pub memsync: memsync::MemSync,
    pub updater: updater::Updater,
    pub remote: remote::RemoteApp,
    pub canvas: canvas::CanvasApp,
}

pub fn start_features(core: &Arc<Core>) {
    let _ = core.features.set(Features { memsync: memsync::MemSync::default(), updater: updater::Updater::new(&core.info.version, core.info.is_packaged), remote: remote::start(core), canvas: canvas::CanvasApp::default() });
    canvas::start(core);
    memsync::start_background_sync(core);
    updater::start_startup_check(core);
    remote::start_background(core);
}

/// Pure functions the check scripts call by name, through wb-ipc
pub fn test_call(name: &str, a: &[Value]) -> Result<Value, String> {
    let text = |i: usize| a.get(i).and_then(Value::as_str).unwrap_or("");
    let to_json = |d: usage::Days| serde_json::to_value(d).map_err(|e| e.to_string());
    match name {
        "claudeBgTasks" => Ok(Value::Array(bgtasks::claude_bg_tasks(text(0)))),
        "codexBgTasks" => Ok(Value::Array(bgtasks::codex_bg_tasks(text(0)))),
        "antigravityBgTasks" => Ok(Value::Array(bgtasks::antigravity_bg_tasks(text(0)))),
        "claudeDays" => to_json(usage::claude_days(text(0))),
        "codexDays" => to_json(usage::codex_days(text(0))),
        "antigravityDays" => to_json(usage::antigravity_days(text(0))),
        "sumDays" => {
            let days: usage::Days = serde_json::from_value(a.first().cloned().unwrap_or(Value::Null)).map_err(|e| e.to_string())?;
            serde_json::to_value(usage::sum_days(&days, text(1))).map_err(|e| e.to_string())
        }
        "dayKey" => Ok(Value::String(usage::day_key(a.first().unwrap_or(&Value::Null)))),
        "looksLikeApprovalPrompt" => Ok(Value::Bool(wb_pty::approval::looks_like_approval_prompt(text(0)))),
        "phoneCanvasOps" => Ok(serde_json::json!(remote::server::PHONE_CANVAS_OPS)),
        "phoneAuditedOps" => Ok(serde_json::json!(remote::server::PHONE_AUDITED_OPS)),
        "parseApprovalPrompt" => Ok(match wb_pty::approval::parse_approval_prompt(text(0)) {
            Some(p) => serde_json::json!({ "title": p.title, "options": p.options, "selected": p.selected, "canApprove": p.can_approve, "canReject": p.can_reject }),
            None => Value::Null,
        }),
        _ => remote::test_call(name, a),
    }
}

pub fn features_call(core: &Arc<Core>, window: u32, channel: &str, args: &[Value]) -> Result<Value, String> {
    match channel.split(':').next().unwrap_or("") {
        "memsync" => memsync::call(core, window, channel, args),
        "updater" => updater::call(core, channel, args),
        "remote" => remote::call(core, window, channel, args),
        "canvas" => canvas::call(core, window, channel, args),
        _ => Err(format!("No handler registered for '{channel}'")),
    }
}
