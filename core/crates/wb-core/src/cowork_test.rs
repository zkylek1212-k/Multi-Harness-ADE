// What the Cowork test scripts call, by the names they use. Lets the same scenarios run against this implementation (wb-ipc).
use std::path::Path;

use serde_json::{json, Map, Value};
use wb_git as g;

use crate::cowork::{self, cerr, cerr_m, Service, StartOptions, CR};
use crate::cowork_exec as exec;
use crate::cowork_shared as sh;
use crate::util::{sv, uuid};

fn check(c: sh::Check) -> Value {
    match c {
        Ok(v) => json!({ "ok": true, "value": v }),
        Err(errors) => json!({ "ok": false, "errors": errors }),
    }
}

fn parsed(p: sh::Parsed) -> Value {
    let mut o = Map::new();
    o.insert("ok".into(), json!(p.value.is_some()));
    match p.value {
        Some(v) => {
            o.insert("value".into(), v);
        }
        None => {
            o.insert("error".into(), json!(p.error));
        }
    }
    o.insert("raw".into(), json!(p.raw));
    if let Some(u) = p.usage {
        o.insert("usage".into(), u);
    }
    if let Some(m) = p.model {
        o.insert("model".into(), json!(m));
    }
    Value::Object(o)
}

fn tool_from(v: &Value) -> sh::ToolEvent {
    sh::ToolEvent { id: v.get("id").and_then(Value::as_str).map(str::to_string), name: sv(v, "name").to_string(), fingerprint: sv(v, "fingerprint").to_string(), failed: v.get("failed").and_then(Value::as_bool), file_change: v.get("fileChange") == Some(&json!(true)) }
}

fn event_json(e: sh::ExecEvent) -> Value {
    let mut o = Map::new();
    if !e.tools.is_empty() {
        let rows: Vec<Value> = e
            .tools
            .iter()
            .map(|t| {
                let mut r = Map::new();
                if let Some(i) = &t.id {
                    r.insert("id".into(), json!(i));
                }
                r.insert("name".into(), json!(t.name));
                r.insert("fingerprint".into(), json!(t.fingerprint));
                if let Some(f) = t.failed {
                    r.insert("failed".into(), json!(f));
                }
                if t.file_change {
                    r.insert("fileChange".into(), json!(true));
                }
                Value::Object(r)
            })
            .collect();
        o.insert("tools".into(), json!(rows));
    }
    if !e.tool_results.is_empty() {
        o.insert("toolResults".into(), json!(e.tool_results.iter().map(|(id, failed)| json!({ "id": id, "failed": failed })).collect::<Vec<_>>()));
    }
    for (k, v) in [("sessionId", e.session_id), ("progress", e.progress), ("text", e.text), ("textDelta", e.text_delta), ("model", e.model)] {
        if let Some(v) = v {
            o.insert(k.into(), json!(v));
        }
    }
    if let Some((ok, text, err)) = e.final_ {
        let mut f = json!({ "ok": ok, "text": text });
        if let Some(x) = err {
            f["error"] = json!(x);
        }
        o.insert("final".into(), f);
    }
    if let Some(u) = e.usage {
        o.insert("usage".into(), u);
    }
    Value::Object(o)
}

/// Pure functions. None means undefined in the caller's terms.
pub fn shared_call(name: &str, a: &[Value]) -> Result<Option<Value>, String> {
    let at = |i: usize| a.get(i).unwrap_or(&Value::Null);
    let strs = |i: usize| sh::strs(at(i));
    let some = |v: Value| Ok(Some(v));
    let ctx = at(1);
    let assignable = sh::strs(&ctx["assignable"]);
    let max = ctx["maxTasks"].as_u64().unwrap_or(0) as usize;
    match name {
        "normalizeScopePath" => some(match sh::normalize_scope_path(at(0).as_str().unwrap_or("")) {
            Ok(p) => json!({ "ok": true, "path": p }),
            Err(r) => json!({ "ok": false, "reason": r }),
        }),
        "checkTasks" => some(check(sh::check_tasks(a.first(), &sh::TaskCtx { assignable: &assignable, max_tasks: max, require_reason: ctx["requireReason"] == json!(true) }))),
        "checkR1" => some(check(sh::check_r1(at(0), &assignable, max))),
        "checkR2" => some(check(sh::check_r2(at(0)))),
        "checkResolution" => some(check(sh::check_resolution(at(0), &assignable, max, &sh::strs(&ctx["requiredIssues"])))),
        "checkDiscussion" => some(check(sh::check_discussion(at(0), at(1).as_array().map(Vec::as_slice).unwrap_or(&[])))),
        "checkDiscussionSummary" => some(check(sh::check_discussion_summary(at(0)))),
        "checkExecReview" => some(check(sh::check_exec_review(at(0)))),
        "currentBoard" => Ok(sh::current_board(at(0)).cloned()),
        "filesOutsideScope" => some(json!(sh::files_outside_scope(&strs(0), &strs(1)))),
        "touchesDependencies" => some(json!(sh::touches_dependencies(&strs(0)))),
        "parseExecResult" => Ok(sh::parse_exec_result(at(0).as_str().unwrap_or(""))),
        "stripResultBlock" => some(json!(sh::strip_result_block(at(0).as_str().unwrap_or("")))),
        "parseClaudeOutput" => some(parsed(sh::parse_claude_output(at(0).as_str().unwrap_or("")))),
        "parseAgyOutput" => some(parsed(sh::parse_agy_output(at(0).as_str().unwrap_or("")))),
        "parseCodexOutput" => some(parsed(sh::parse_codex_output(at(0).as_str().unwrap_or(""), at(1).as_str()))),
        "parseClaudeModelCatalog" => sh::parse_claude_model_catalog(at(0).as_str().unwrap_or(""), at(1).as_str().unwrap_or("")).map(Some),
        "groupAgyModels" => {
            let list: Vec<(String, String)> = at(0).as_array().into_iter().flatten().map(|x| (sv(x, "id").to_string(), sv(x, "label").to_string())).collect();
            some(json!(sh::group_agy_models(&list)))
        }
        "plannerInvocation" => {
            let o = at(1);
            let inv = sh::planner_invocation(at(0).as_str().unwrap_or(""), &sh::PlannerOpts { cwd: sv(o, "cwd"), schema: &o["schema"], schema_file: sv(o, "schemaFile"), out_file: sv(o, "outFile"), windows_sandbox: o["windowsSandbox"].as_str(), model: o["model"].as_str(), effort: o["effort"].as_str(), discussion: o["discussion"] == json!(true) });
            let mut out = json!({ "args": inv.args, "ndjson": inv.stdin_ndjson, "guard": inv.guard_agy });
            if let Some(x) = inv.schema_file {
                out["schemaFile"] = json!(x);
            }
            if let Some(x) = inv.out_file {
                out["outFile"] = json!(x);
            }
            if let Some(x) = inv.min_timeout_sec {
                out["minTimeoutSec"] = json!(x);
            }
            some(out)
        }
        "execInvocation" => {
            let o = at(1);
            let inv = sh::exec_invocation(at(0).as_str().unwrap_or(""), sv(o, "cwd"), o["resumeId"].as_str(), o["model"].as_str(), o["effort"].as_str(), o["bypass"] == json!(true), sv(o, "outFile"));
            let mut out = json!({ "args": inv.args, "ndjson": inv.stdin_ndjson });
            if let Some(x) = inv.out_file {
                out["outFile"] = json!(x);
            }
            some(out)
        }
        "agyStdin" => some(json!(sh::agy_stdin(at(0).as_str().unwrap_or("")))),
        "agyToolGuard" => some(sh::agy_tool_guard(at(0).as_str().unwrap_or("")).map(Value::String).unwrap_or(Value::Null)),
        "agyIsolationSettings" => some(sh::agy_isolation_settings(at(0).as_str().unwrap_or(""))),
        "toolNote" => some(json!(sh::tool_note(at(0).as_str().unwrap_or("")))),
        "withToolNote" => some(json!(sh::with_tool_note(at(0).as_str().unwrap_or(""), at(1).as_str().unwrap_or("")))),
        "sanitizeCoworkSettings" => some(sh::sanitize_cowork_settings(a.first())),
        "sanitizeModelChoice" => some(sh::sanitize_model_choice(a.first())),
        "sanitizeModelChoices" => some(sh::sanitize_model_choices(a.first())),
        "schemaFor" => some(sh::schema_for(at(0).as_str().unwrap_or(""), &strs(1))),
        "taskDispatchText" => some(json!(sh::task_dispatch_text(at(0), at(1)))),
        "buildDiscussionPrompt" => some(json!(sh::build_discussion_prompt(at(0), at(1).as_str().unwrap_or("")))),
        "buildDiscussionSummaryPrompt" => some(json!(sh::build_discussion_summary_prompt(at(0)))),
        "buildDiscussionConclusionPrompt" => some(json!(sh::build_discussion_conclusion_prompt(at(0)))),
        "discussionProjectPrompt" => some(json!(sh::discussion_project_prompt(at(0)))),
        "discussionNeedsSummary" => some(json!(sh::discussion_needs_summary(at(0)))),
        "discussionTranscript" => some(json!(sh::discussion_transcript(at(0).as_array().map(Vec::as_slice).unwrap_or(&[])))),
        "buildR1Prompt" => some(json!(sh::build_r1_prompt(&at(0)["run"], &sh::strs(&at(0)["notes"])))),
        "buildR2Prompt" => some(json!(sh::build_r2_prompt(&at(0)["run"], sv(at(0), "reviewer"), &at(0)["r1"], &sh::strs(&at(0)["notes"])))),
        "buildR34Prompt" => some(json!(sh::build_r34_prompt(&at(0)["run"], &at(0)["r1"], &at(0)["reviews"], at(0)["issues"].as_array().map(Vec::as_slice).unwrap_or(&[]), &sh::strs(&at(0)["notes"])))),
        "buildRevisePrompt" => some(json!(sh::build_revise_prompt(&at(0)["run"], &at(0)["board"], sv(at(0), "feedback"), &sh::strs(&at(0)["notes"])))),
        "buildRepairPrompt" => some(json!(sh::build_repair_prompt(at(0).as_str().unwrap_or(""), at(1).as_str().unwrap_or(""), &strs(2)))),
        "buildExecPrompt" => {
            let o = at(2);
            some(json!(sh::build_exec_prompt(at(0), at(1), sv(o, "branch"), o["deps"].as_array().map(Vec::as_slice).unwrap_or(&[]), o["depsLinked"].as_bool(), o["gates"] == json!(true))))
        }
        "buildExecReviewPrompt" => {
            let o = at(2);
            some(json!(sh::build_exec_review_prompt(at(0), at(1), sv(o, "diff"), o["truncated"] == json!(true), o["checks"].as_array().map(Vec::as_slice).unwrap_or(&[]), &sh::strs(&o["waivers"]))))
        }
        "collectIssues" => some(json!(sh::collect_issues(at(0).as_str().unwrap_or(""), at(1), at(2)))),
        "assignableAgents" => some(json!(sh::assignable_agents(at(0)))),
        "acceptanceGaps" => some(json!(sh::acceptance_gaps(at(0), at(1).as_object().map(|_| at(1))))),
        "validChecks" => some(json!(sh::valid_checks(a.first()))),
        "toolFingerprint" => some(json!(sh::tool_fingerprint(at(0).as_str().unwrap_or(""), a.get(1)))),
        "execApprovalContent" => some(json!(sh::exec_approval_content(at(0), at(1), sv(at(2), "mode"), at(2)["linkDeps"] == json!(true), at(2)["bypass"] == json!(true)))),
        "parseExecLine" => some(sh::parse_exec_line(at(0).as_str().unwrap_or(""), at(1).as_str().unwrap_or(""), at(2).as_str().unwrap_or("")).map(event_json).unwrap_or(Value::Null)),
        "maskSecrets" => some(json!(sh::mask_secrets(at(0).as_str().unwrap_or("")))),
        "hookPathsIn" => some(json!(sh::hook_paths_in(&strs(0), at(1).as_str()))),
        "mergeBlockers" => some(json!(sh::merge_blockers(a.first().filter(|v| !v.is_null())))),
        // A monitor has state: the caller replays its history and takes the answer to the last event
        "runawayFeed" => {
            let idle = at(1)["idleMs"].as_f64().unwrap_or(15.0 * 60_000.0);
            let mut m = sh::RunawayMonitor::with(at(0).as_f64().unwrap_or(0.0), at(1)["window"].as_u64().unwrap_or(20) as usize, at(1)["repeat"].as_u64().unwrap_or(8) as usize, idle);
            let mut last = Vec::new();
            for step in at(2).as_array().into_iter().flatten() {
                let tools: Vec<sh::ToolEvent> = step["ev"]["tools"].as_array().into_iter().flatten().map(tool_from).collect();
                let results: Vec<(String, bool)> = step["ev"]["toolResults"].as_array().into_iter().flatten().map(|r| (sv(r, "id").to_string(), r["failed"] == json!(true))).collect();
                last = m.feed(&tools, &results, step["now"].as_f64().unwrap_or(0.0));
            }
            some(json!(last.iter().map(|x| json!({ "signal": x.signal, "detail": x.detail, "canStop": x.can_stop })).collect::<Vec<_>>()))
        }
        // Git and path helpers
        "git" => g::git(Path::new(at(0).as_str().unwrap_or("")), &strs(1).iter().map(String::as_str).collect::<Vec<_>>()).map(|o| Some(json!(o))),
        "readBaseline" => g::read_baseline(Path::new(at(0).as_str().unwrap_or(""))).map(|b| Some(serde_json::to_value(b).unwrap())),
        "createSnapshot" => g::create_snapshot(Path::new(at(0).as_str().unwrap_or("")), Path::new(at(1).as_str().unwrap_or("")), at(2).as_str().unwrap_or("")).map(|_| None),
        "snapshotChanges" => g::snapshot_changes(Path::new(at(0).as_str().unwrap_or("")), at(1).as_str().unwrap_or("")).map(|c| Some(json!(c))),
        "snapshotDirFor" => some(json!(cowork::snapshot_dir_for(at(0).as_str().unwrap_or(""), at(1).as_str().unwrap_or("")))),
        "removeSnapshot" => cowork::remove_snapshot(at(0).as_str().unwrap_or(""), at(1).as_str().unwrap_or("")).map(|_| None),
        "scopeEscapes" => some(cowork::scope_escapes(at(0).as_str().unwrap_or(""), at(1).as_str().unwrap_or("")).map(Value::String).unwrap_or(Value::Null)),
        "diffGuard" => some(json!(cowork::diff_guard(at(0), at(1)))),
        "gitGuardState" => g::git_guard_state(Path::new(at(0).as_str().unwrap_or(""))).map(|s| Some(json!(s))),
        "exportBaselineHooks" => g::export_baseline_hooks(Path::new(at(0).as_str().unwrap_or("")), at(1).as_str().unwrap_or(""), Path::new(at(2).as_str().unwrap_or(""))).map(|s| Some(json!(s))),
        "patchId" => g::patch_id(Path::new(at(0).as_str().unwrap_or("")), at(1).as_str().unwrap_or(""), at(2).as_str().unwrap_or("")).map(|s| Some(json!(s))),
        "mergeTree" => some(serde_json::to_value(g::merge_tree(Path::new(at(0).as_str().unwrap_or("")), at(1).as_str().unwrap_or(""), at(2).as_str().unwrap_or(""))).unwrap()),
        "resolveCheckCommand" => some(match wb_proc::resolve_command(at(0).as_str().unwrap_or(""), Path::new(at(1).as_str().unwrap_or(""))) {
            wb_proc::Resolved::Path(p) => json!({ "resolved": p }),
            wb_proc::Resolved::Error(e) => json!({ "error": e }),
        }),
        "runCheck" => some(exec::run_check_with(0, &|_| {}, &strs(0), at(1).as_str().unwrap_or(""), &wb_proc::CancelToken::default(), at(2).as_f64(), at(3).as_str())),
        _ => Err(format!("unknown shared call {name}")),
    }
}

/// Method calls on a service, by the same names
pub fn service_call(svc: &Service, method: &str, a: &[Value]) -> CR<Value> {
    let at = |i: usize| a.get(i).unwrap_or(&Value::Null);
    let id = at(0).as_str().unwrap_or("").to_string();
    let s = |i: usize| at(i).as_str().unwrap_or("").to_string();
    let done = |r: CR<()>| r.map(|_| Value::Null);
    let run = || svc.must(&id);
    match method {
        "init" => {
            svc.init();
            Ok(json!(svc.all_runs().iter().map(|r| r.lock().unwrap().clone()).collect::<Vec<_>>()))
        }
        "shutdown" => {
            svc.shutdown();
            Ok(Value::Null)
        }
        "list" => svc.list(&id),
        "get" => Ok(svc.get(&id)),
        "start" => {
            let o = at(0);
            let opt = |k: &str| o.get(k).and_then(Value::as_str).map(str::to_string);
            svc.start(StartOptions {
                mode: opt("mode"),
                auto_effort: o.get("autoEffort") != Some(&json!(false)),
                summarizer: opt("summarizer"),
                summarize_each_round: o.get("summarizeEachRound") == Some(&json!(true)),
                workspace: sv(o, "workspace").to_string(),
                prompt: opt("prompt"),
                chair: sv(o, "chair").to_string(),
                participants: sh::strs(&o["participants"]),
                chair_executes: o.get("chairExecutes") == Some(&json!(true)),
                language: sv(o, "language").to_string(),
                limits: o["limits"].clone(),
                models: o.get("models").cloned(),
                skills: o["skills"].as_array().cloned().unwrap_or_default(),
                project_instructions: o.get("projectInstructions") != Some(&json!(false)),
            })
        }
        "cancel" => done(svc.cancel(&id)),
        "discuss" => done(svc.discuss(&id, &s(1))),
        "setSummarizer" => done(svc.set_summarizer(&id, &s(1))),
        "summarizeDiscussion" => done(svc.summarize_discussion(&id, at(1) == &json!(true))),
        "retry" => done(svc.retry(&id)),
        "dropFailedReviewers" => done(svc.drop_failed_reviewers(&id)),
        "addNote" => done(svc.add_note(&id, &s(1))),
        "feedback" => done(svc.feedback(&id, &s(1))),
        "editBoard" => svc.edit_board(&id, at(1).as_f64().unwrap_or(f64::NAN), at(2)),
        "dismissUnresolved" => done(svc.dismiss_unresolved(&id)),
        "approve" => done(svc.approve(&id, at(1).as_f64().unwrap_or(f64::NAN))),
        "raiseLimits" => done(svc.raise_limits(&id, at(1)["maxPlanningCalls"].as_f64().unwrap_or(0.0), at(1)["maxPlanningMinutes"].as_f64().unwrap_or(0.0))),
        "logDispatch" => done(svc.log_dispatch(&id, &s(1), &s(2))),
        "delete" => done(svc.delete(&id)),
        "execPreview" => {
            let o = at(1);
            if !matches!(sv(o, "mode"), "sequential" | "parallel") {
                return Err(cerr("not-allowed"));
            }
            let cell = run()?;
            let r = cell.lock().unwrap();
            exec::preview(svc, &r, sv(o, "mode"), o["linkDeps"] == json!(true), o["bypass"] == json!(true), o["worktreeRoot"].as_str())
        }
        "execStart" => {
            let o = at(1);
            if !matches!(sv(o, "mode"), "sequential" | "parallel") {
                return Err(cerr("not-allowed"));
            }
            done(svc.exec_start(&id, &exec::StartOpts { mode: sv(o, "mode").to_string(), link_deps: o["linkDeps"] == json!(true), bypass: o["bypass"] == json!(true), approval: o["approval"].as_str().map(str::to_string), review: o.get("review") != Some(&json!(false)), worktree_root: o["worktreeRoot"].as_str().map(str::to_string), runaway_stop: o["runawayStop"] == json!(true) }))
        }
        "execDiff" => exec::diff(&run()?),
        "execWaive" => done(exec::waive(svc, &run()?, &s(1), &s(2), at(3) == &json!(true))),
        "execMessage" => done(exec::message(svc, &run()?, &s(1), &s(2))),
        "execRetry" => done(exec::retry(svc, &run()?, &s(1))),
        "execAcceptScope" => done(exec::accept_scope(svc, &run()?, &s(1), at(2).as_str())),
        "execRevertScope" => done(exec::revert_scope(svc, &run()?, &s(1))),
        "execPause" => done(exec::pause(svc, &run()?)),
        "execResume" => done(exec::resume(svc, &run()?)),
        "execMerge" => done(exec::merge(svc, &run()?, a.get(1).filter(|x| x.is_object()))),
        "execCleanup" => done(exec::cleanup(svc, &run()?)),
        // The TS scenarios sometimes change a live run object directly. Here that is an explicit call.
        "testPatch" => {
            let cell = run()?;
            let mut r = cell.lock().unwrap();
            let path: Vec<String> = sh::strs(at(1));
            let mut slot = &mut *r;
            for key in &path {
                slot = if key == "-1" {
                    slot.as_array_mut().and_then(|x| x.last_mut()).ok_or_else(|| cerr("bad-path"))?
                } else {
                    &mut slot[key.as_str()]
                };
            }
            *slot = at(2).clone();
            svc.save(&mut r);
            Ok(Value::Null)
        }
        _ => Err(cerr_m("unknown-method", method)),
    }
}

/// A reference so wb-ipc can name fresh temp paths without its own dependency
pub fn temp_name() -> String {
    uuid()
}
