//! Routines (cowork-agui-implement.md §11): a Bot starts a new conversation on a schedule, or when a branch gets new
//! commits. Only while the app runs; a missed time is skipped, not made up; one run per routine at a time; nothing
//! is approved for it (its approvals wait in Needs you like any other); ten failures in a row switch it off.
//! The schedule math is pure and takes the clock as an argument, so tests use a fixed one.
use chrono::{DateTime, Datelike, Duration as Span, Local, NaiveTime, TimeZone};

use super::*;

pub const MIN_EVERY_MIN: u32 = 15;
pub const MAX_ENABLED: usize = 20;
const MAX_ROUTINES: usize = 100;
const FATIGUE: u32 = 10;
const GRACE_MAX_MS: f64 = 30.0 * 60_000.0;
const TICK: Duration = Duration::from_secs(30);
const MAX_PROMPT: usize = 16 * 1024;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Schedule {
    /// `time` is HH:MM, local
    Daily { time: String },
    /// `day` 0 = Monday
    Weekly { day: u8, time: String },
    Every { minutes: u32 },
    /// New commits on `branch` (its remote copy after a fetch when there is one), checked every 15 minutes
    Commit { branch: String },
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct LastRun {
    channel: String,
    at: f64,
    /// running | ok | failed | stopped
    status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    summary: String,
    /// The user saw the finding (dismissed it from Needs you)
    #[serde(default)]
    seen: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Routine {
    id: String,
    name: String,
    prompt: String,
    agent: String,
    /// The repository's root
    repo: String,
    /// report: read-only; change: may change code in its own worktree
    scope: String,
    schedule: Schedule,
    enabled: bool,
    #[serde(default)]
    next_at: f64,
    #[serde(default)]
    failures: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_run: Option<LastRun>,
    /// Commit routines: the head they last saw
    #[serde(default, skip_serializing_if = "Option::is_none")]
    seen_head: Option<String>,
}

// ---------- pure rules (unit tested) ----------

fn hm(time: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(time, "%H:%M").ok()
}

/// How long one period is, for the grace window
fn period_ms(s: &Schedule) -> f64 {
    match s {
        Schedule::Daily { .. } => 86_400_000.0,
        Schedule::Weekly { .. } => 7.0 * 86_400_000.0,
        Schedule::Every { minutes } => *minutes as f64 * 60_000.0,
        Schedule::Commit { .. } => MIN_EVERY_MIN as f64 * 60_000.0,
    }
}

/// The first time strictly after `after`
pub fn next_at<Tz: TimeZone>(s: &Schedule, after: &DateTime<Tz>) -> DateTime<Tz> {
    let tz = after.timezone();
    let at_day = |d: chrono::NaiveDate, t: NaiveTime| tz.from_local_datetime(&d.and_time(t)).earliest();
    match s {
        Schedule::Every { minutes } => after.clone() + Span::minutes((*minutes).max(MIN_EVERY_MIN) as i64),
        Schedule::Commit { .. } => after.clone() + Span::minutes(MIN_EVERY_MIN as i64),
        Schedule::Daily { time } | Schedule::Weekly { time, .. } => {
            let t = hm(time).unwrap_or_default();
            let want = match s {
                Schedule::Weekly { day, .. } => Some(*day as u32 % 7),
                _ => None,
            };
            let mut d = after.date_naive();
            for _ in 0..15 {
                if want.is_none_or(|w| d.weekday().num_days_from_monday() == w) {
                    // A local time that does not exist that day (a DST gap) moves to the next day
                    if let Some(x) = at_day(d, t).filter(|x| x > after) {
                        return x;
                    }
                }
                d = d.succ_opt().unwrap_or(d);
            }
            after.clone() + Span::days(1)
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Due {
    Wait,
    Run,
    /// Missed by more than the grace: not made up
    Skip,
}

/// A tenth of a period, at most 30 minutes
pub fn due(s: &Schedule, next: f64, now: f64) -> Due {
    let grace = (period_ms(s) * 0.1).min(GRACE_MAX_MS);
    if now < next {
        Due::Wait
    } else if now - next <= grace {
        Due::Run
    } else {
        Due::Skip
    }
}

/// After a run ended: success resets the count; the first failure and the one that switches it off are told
fn on_result(r: &mut Routine, ok: bool) -> Option<&'static str> {
    if ok {
        r.failures = 0;
        return None;
    }
    r.failures += 1;
    if r.failures >= FATIGUE {
        r.enabled = false;
        Some("off")
    } else if r.failures == 1 {
        Some("failed")
    } else {
        None
    }
}

fn valid(r: &Routine, chat_ok: impl Fn(&str) -> bool) -> Result<(), String> {
    if r.name.trim().is_empty() || r.name.chars().count() > 80 {
        return Err("A routine needs a name (up to 80 characters)".into());
    }
    if r.prompt.trim().is_empty() || r.prompt.len() > MAX_PROMPT {
        return Err("Say what the routine should do".into());
    }
    if !chat_ok(&r.agent) {
        return Err(format!("{} has no chat mode; a routine needs one", r.agent));
    }
    if !["report", "change"].contains(&r.scope.as_str()) {
        return Err("Scope is report or change".into());
    }
    match &r.schedule {
        Schedule::Every { minutes } if *minutes < MIN_EVERY_MIN || *minutes > 7 * 24 * 60 => Err(format!("At least every {MIN_EVERY_MIN} minutes")),
        Schedule::Daily { time } | Schedule::Weekly { time, .. } if hm(time).is_none() => Err("Time is HH:MM".into()),
        Schedule::Weekly { day, .. } if *day > 6 => Err("Day is 0 (Monday) to 6".into()),
        Schedule::Commit { branch } if branch.is_empty() || branch.len() > 200 || branch.starts_with('-') || branch.contains("..") || !branch.chars().all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c)) => Err("Not a branch name".into()),
        _ => Ok(()),
    }
}

// ---------- storage ----------

static LOCK: Mutex<()> = Mutex::new(());

fn file(core: &Core) -> PathBuf {
    core.data_dir.join("routines.json")
}

fn load(core: &Core) -> Vec<Routine> {
    let ok = |v: &Value| serde_json::from_value::<Vec<Routine>>(v.clone()).is_ok();
    read_json_with_fallback(&file(core), &ok).and_then(|(v, _)| serde_json::from_value(v).ok()).unwrap_or_default()
}

fn save(core: &Core, list: &[Routine]) -> R<()> {
    write_json_atomic(&file(core), &serde_json::to_value(list).unwrap()).map_err(|e| err("invalid", e))
}

/// Read-modify-write under one lock, so the tick and the UI never lose each other's changes
fn update<T>(core: &Core, f: impl FnOnce(&mut Vec<Routine>) -> R<T>) -> R<T> {
    let _g = LOCK.lock().unwrap();
    let mut list = load(core);
    let out = f(&mut list)?;
    save(core, &list)?;
    Ok(out)
}

fn now_local() -> DateTime<Local> {
    Local::now()
}

fn ms(d: &DateTime<Local>) -> f64 {
    d.timestamp_millis() as f64
}

fn from_ms(x: f64) -> DateTime<Local> {
    Local.timestamp_millis_opt(x as i64).single().unwrap_or_else(now_local)
}

// ---------- running ----------

fn preface(r: &Routine) -> String {
    let what = if r.scope == "report" {
        "Report only: read and investigate, do not change any file. End with your findings in a short summary."
    } else {
        "You may change code in this worktree and commit there; nothing is pushed. End with a short summary of what you changed."
    };
    format!("[Agent Workbench routine \"{}\", started on its schedule; nobody is watching, so anything that needs approval waits for the user.] {what}", r.name)
}

/// Opens the run's conversation and sends the first message. Off the tick thread: making a worktree takes a while.
fn start_run(core: &Arc<Core>, r: Routine, extra: String) {
    let core = core.clone();
    std::thread::spawn(move || {
        let result = (|| -> R<String> {
            let name = js_prefix(&format!("Routine · {}", r.name), 80);
            let ch = dispatch(&core, &r.repo, "channel.create", &json!({ "name": name }), Origin::Desktop)?["channel"]["id"].as_str().unwrap_or("").to_string();
            {
                let repo = repo_for(&core, &r.repo)?;
                let mut st = repo.st.lock().unwrap();
                if let Some(c) = st.channels.get_mut(&ch) {
                    c.routine = Some(r.id.clone());
                    repo.save(c)?;
                }
            }
            mark(&core, &r.id, |r| r.last_run = Some(LastRun { channel: ch.clone(), at: now_ms(), status: "running".into(), ..Default::default() }));
            dispatch(&core, &r.repo, "node.create", &json!({ "channelId": ch, "agent": r.agent, "workspace": "worktree", "mode": chat::MODE, "readOnly": r.scope == "report" }), Origin::Desktop)?;
            dispatch(&core, &r.repo, "chat.send", &json!({ "channelId": ch, "text": format!("{}\n\n{}{extra}", preface(&r), r.prompt) }), Origin::Desktop)?;
            Ok(ch)
        })();
        if let Err(e) = result {
            finish(&core, &r.id, None, false, &e.message);
        }
    });
}

fn mark(core: &Core, id: &str, f: impl FnOnce(&mut Routine)) {
    let _ = update(core, |list| {
        if let Some(r) = list.iter_mut().find(|r| r.id == id) {
            f(r);
        }
        Ok(())
    });
}

/// A run ended (its first turn finished or failed, or it could not start)
fn finish(core: &Arc<Core>, id: &str, ch: Option<&str>, ok: bool, summary: &str) {
    let told = update(core, |list| {
        let Some(r) = list.iter_mut().find(|r| r.id == id) else { return Ok(None) };
        if ch.is_some() && r.last_run.as_ref().map(|l| l.channel.as_str()) != ch {
            return Ok(None);
        }
        let channel = r.last_run.as_ref().map(|l| l.channel.clone()).unwrap_or_default();
        r.last_run = Some(LastRun { channel, at: now_ms(), status: if ok { "ok" } else { "failed" }.into(), summary: js_prefix(summary.trim(), 2000), seen: false });
        Ok(on_result(r, ok).map(|t| (t, r.name.clone())))
    })
    .ok()
    .flatten();
    if let Some((t, name)) = told {
        let what = if t == "off" { format!("failed {FATIGUE} times in a row and was switched off") } else { "failed".into() };
        crate::remote::on_chat_waiting(core, &format!("routine-{id}"), &format!("Routine {name}"), &what);
    }
    core.host.send_all(ROUTINES_EVENT, json!([]));
}

pub const ROUTINES_EVENT: &str = "canvas:routines";

/// From chat.rs: a turn of a routine's conversation ended. The first one is the run; the process then stops so it
/// costs nothing while the result waits (restart it to go on talking).
pub(super) fn on_turn_end(core: &Arc<Core>, repo: &Arc<Repo>, ch: &str, nd: &str, ok: bool, interrupted: bool) {
    let (id, summary) = {
        let mut st = repo.st.lock().unwrap();
        let Some(id) = st.channels.get(ch).and_then(|c| c.routine.clone()) else { return };
        let text = repo.transcripts(&mut st, ch).get(nd).and_then(|t| t.entries.iter().rev().find_map(|e| if let crate::agui::transcript::Entry::Text { text, .. } = e { Some(text.clone()) } else { None })).unwrap_or_default();
        (id, text)
    };
    let running = load(core).into_iter().any(|r| r.id == id && r.last_run.as_ref().is_some_and(|l| l.channel == ch && l.status == "running"));
    if !running {
        return;
    }
    if interrupted {
        mark(core, &id, |r| {
            if let Some(l) = &mut r.last_run {
                l.status = "stopped".into();
            }
        });
        core.host.send_all(ROUTINES_EVENT, json!([]));
    } else {
        finish(core, &id, Some(ch), ok, &summary);
    }
    let (core, repo, ch, nd) = (core.clone(), repo.clone(), ch.to_string(), nd.to_string());
    std::thread::spawn(move || {
        let _ = dispatch(&core, &p(&repo.root), "node.close", &json!({ "channelId": ch, "nodeId": nd }), Origin::Desktop);
    });
}

/// New commits on the branch since the routine last looked; the first look only records the head
fn new_commits(r: &Routine, branch: &str) -> (Option<String>, Option<String>) {
    let dir = Path::new(&r.repo);
    let _ = g::git(dir, &["fetch", "--quiet"]);
    let head = [format!("refs/remotes/origin/{branch}"), format!("refs/heads/{branch}")].iter().find_map(|rf| g::git(dir, &["rev-parse", "--verify", "--quiet", rf]).ok()).map(|s| s.trim().to_string());
    match (&r.seen_head, &head) {
        (Some(old), Some(new)) if old != new => (head.clone(), Some(format!("\n\nNew commits on {branch}: {}..{}", &old[..old.len().min(12)], &new[..new.len().min(12)]))),
        _ => (head, None),
    }
}

/// Every 30 seconds while the app runs
fn tick(core: &Arc<Core>) {
    let now = now_local();
    let mut runs: Vec<(Routine, String)> = Vec::new();
    let _ = update(core, |list| {
        for r in list.iter_mut().filter(|r| r.enabled) {
            if r.next_at == 0.0 {
                r.next_at = ms(&next_at(&r.schedule, &now));
                continue;
            }
            match due(&r.schedule, r.next_at, ms(&now)) {
                Due::Wait => continue,
                Due::Skip => {}
                Due::Run if r.last_run.as_ref().is_some_and(|l| l.status == "running") => {}
                Due::Run => runs.push((r.clone(), String::new())),
            }
            r.next_at = ms(&next_at(&r.schedule, &now));
        }
        Ok(())
    });
    for (r, extra) in runs {
        if let Schedule::Commit { branch } = &r.schedule {
            let (head, change) = new_commits(&r, branch);
            mark(core, &r.id, |x| x.seen_head = head);
            if let Some(change) = change {
                start_run(core, r, change);
            }
        } else {
            start_run(core, r, extra);
        }
    }
}

pub(super) fn start(core: &Arc<Core>) {
    let weak = Arc::downgrade(core);
    std::thread::spawn(move || loop {
        std::thread::sleep(TICK);
        let Some(core) = weak.upgrade() else { return };
        tick(&core);
    });
}

fn view(r: &Routine) -> Value {
    let mut v = serde_json::to_value(r).unwrap();
    v["nextAtText"] = json!(if r.enabled && r.next_at > 0.0 { from_ms(r.next_at).format("%Y-%m-%d %H:%M").to_string() } else { String::new() });
    v
}

pub(super) fn dispatch_op(core: &Arc<Core>, ws: &str, op: &str, req: &Value) -> R<Value> {
    let chat_ok = |a: &str| chat::protocol(core, a).is_some();
    let out = match op {
        "routine.list" => {
            let list = load(core);
            return Ok(json!(list.iter().map(view).collect::<Vec<_>>()));
        }
        "routine.save" => {
            let mut r: Routine = serde_json::from_value(req["routine"].clone()).map_err(|e| err("invalid", format!("Not a routine: {e}")))?;
            r.name = js_prefix(sanitize_text(&r.name, false).trim(), 80);
            r.prompt = sanitize_text(&r.prompt, true);
            valid(&r, chat_ok).map_err(|e| err("invalid", e))?;
            update(core, |list| {
                let old = list.iter().position(|x| x.id == r.id);
                if old.is_none() {
                    if list.len() >= MAX_ROUTINES {
                        return Err(err("invalid", "Too many routines"));
                    }
                    if ws.is_empty() {
                        return Err(err("invalid", "Open a folder first"));
                    }
                    r.id = short_id();
                    r.repo = p(&repo_for(core, ws)?.root);
                }
                if r.enabled && list.iter().filter(|x| x.enabled && x.id != r.id).count() >= MAX_ENABLED {
                    return Err(err("invalid", format!("At most {MAX_ENABLED} routines can be on at once")));
                }
                // The app keeps what only it knows; a changed schedule starts counting from now
                if let Some(o) = old.map(|i| list[i].clone()) {
                    r.repo = o.repo;
                    r.last_run = o.last_run;
                    r.seen_head = o.seen_head.filter(|_| o.schedule == r.schedule);
                    r.failures = if r.enabled && !o.enabled { 0 } else { o.failures };
                }
                r.next_at = if r.enabled { ms(&next_at(&r.schedule, &now_local())) } else { 0.0 };
                match old {
                    Some(i) => list[i] = r.clone(),
                    None => list.push(r.clone()),
                }
                Ok(view(&r))
            })?
        }
        "routine.delete" => {
            let id = s(req, "id");
            update(core, |list| {
                list.retain(|r| r.id != id);
                Ok(Value::Null)
            })?
        }
        "routine.runNow" => {
            let id = s(req, "id").to_string();
            let r = load(core).into_iter().find(|r| r.id == id).ok_or_else(|| err("not-found", "routine"))?;
            if r.last_run.as_ref().is_some_and(|l| l.status == "running") {
                return Err(err("busy", "This routine is already running"));
            }
            start_run(core, r, String::new());
            Value::Null
        }
        "routine.dismiss" => {
            let id = s(req, "id").to_string();
            mark(core, &id, |r| {
                if let Some(l) = &mut r.last_run {
                    l.seen = true;
                }
            });
            Value::Null
        }
        _ => return Err(err("invalid", "Unknown routine operation")),
    };
    core.host.send_all(ROUTINES_EVENT, json!([]));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn at(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    #[test]
    fn next_times() {
        let daily = Schedule::Daily { time: "08:30".into() };
        assert_eq!(next_at(&daily, &at("2026-11-19T07:00:00+08:00")), at("2026-11-19T08:30:00+08:00"));
        assert_eq!(next_at(&daily, &at("2026-11-19T08:30:00+08:00")), at("2026-11-20T08:30:00+08:00"), "strictly after");
        // 2026-11-19 is a Thursday (3); Monday 09:00 is four days on
        let weekly = Schedule::Weekly { day: 0, time: "09:00".into() };
        assert_eq!(next_at(&weekly, &at("2026-11-19T10:00:00+08:00")), at("2026-11-23T09:00:00+08:00"));
        assert_eq!(next_at(&Schedule::Every { minutes: 60 }, &at("2026-11-19T10:00:00+08:00")), at("2026-11-19T11:00:00+08:00"));
        assert_eq!(next_at(&Schedule::Every { minutes: 1 }, &at("2026-11-19T10:00:00+08:00")), at("2026-11-19T10:15:00+08:00"), "never under 15 minutes");
    }

    #[test]
    fn a_missed_time_is_skipped_not_made_up() {
        let daily = Schedule::Daily { time: "08:30".into() };
        let next = 1_000_000_000.0;
        assert_eq!(due(&daily, next, next - 1.0), Due::Wait);
        assert_eq!(due(&daily, next, next + 29.0 * 60_000.0), Due::Run, "within 30 minutes");
        assert_eq!(due(&daily, next, next + 31.0 * 60_000.0), Due::Skip, "the app was closed or asleep");
        let hourly = Schedule::Every { minutes: 60 };
        assert_eq!(due(&hourly, next, next + 5.0 * 60_000.0), Due::Run);
        assert_eq!(due(&hourly, next, next + 7.0 * 60_000.0), Due::Skip, "a tenth of the period");
    }

    #[test]
    fn ten_failures_switch_it_off() {
        let mut r = Routine { id: "r".into(), name: "n".into(), prompt: "p".into(), agent: "claude".into(), repo: ".".into(), scope: "report".into(), schedule: Schedule::Every { minutes: 60 }, enabled: true, next_at: 0.0, failures: 0, last_run: None, seen_head: None };
        assert_eq!(on_result(&mut r, false), Some("failed"), "the first failure is told");
        for _ in 2..FATIGUE {
            assert_eq!(on_result(&mut r, false), None);
        }
        assert_eq!(on_result(&mut r, false), Some("off"));
        assert!(!r.enabled);
        assert_eq!(on_result(&mut r, true), None);
        assert_eq!(r.failures, 0, "a success starts the count again");
    }

    #[test]
    fn routines_are_checked() {
        let ok = |a: &str| a == "claude";
        let mut r = Routine { id: "r".into(), name: "nightly".into(), prompt: "look".into(), agent: "claude".into(), repo: ".".into(), scope: "report".into(), schedule: Schedule::Every { minutes: 15 }, enabled: true, next_at: 0.0, failures: 0, last_run: None, seen_head: None };
        assert!(valid(&r, ok).is_ok());
        r.schedule = Schedule::Every { minutes: 14 };
        assert!(valid(&r, ok).is_err());
        r.schedule = Schedule::Daily { time: "25:00".into() };
        assert!(valid(&r, ok).is_err());
        r.schedule = Schedule::Commit { branch: "--upload-pack=x".into() };
        assert!(valid(&r, ok).is_err(), "a branch is never an option");
        r.schedule = Schedule::Commit { branch: "main".into() };
        r.agent = "cmd".into();
        assert!(valid(&r, ok).is_err(), "a routine needs a chat Bot");
    }
}
