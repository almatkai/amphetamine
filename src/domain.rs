//! Native keep-awake replacement for the installed Raycast Amphetamine extension
//! (gstvds et al., MIT; installed `amphetamine/.sc-build/{amp-start,
//! quick-start,amp-end,remaining,amp-duration}.js`). Does not invoke Amphetamine.
//! A single process-wide actor owns the assertion, independent of UI sessions.
use serde_json::{json, Value};
use std::sync::{mpsc, OnceLock};
use std::time::{Duration, Instant};
use tezbar_extension_sdk::{CoreResult, ExtensionContext as CoreContext};

use tezbar_extension_sdk as v2_sdk;

const COMMANDS: &[&str] = &[
    "amp-start",
    "quick-start",
    "amp-end",
    "remaining",
    "amp-duration",
];
const SUBMIT: &str = "amphetamine:start-duration";
const SET_TYPE: &str = "amphetamine:set-session-type";
const MAX_MINUTES: u64 = 10 * 365 * 24 * 60;

pub fn commands() -> &'static [&'static str] {
    if cfg!(any(target_os = "macos", target_os = "windows")) {
        COMMANDS
    } else {
        &[]
    }
}

// This must be called by the app exit hook BEFORE the native core is torn down.
// Windows' assertion also disappears when the owning actor thread exits; macOS
// caffeinate additionally tracks our PID, so a crash cannot strand an assertion.
pub fn shutdown() {
    let _gate = ACTOR_GATE.lock().unwrap();
    SHUTTING_DOWN.store(true, Ordering::Release);
    if let Some(actor) = ACTOR.get() {
        let (tx, rx) = mpsc::sync_channel(1);
        if actor.send(Request::Shutdown(tx)).is_ok() {
            let _ = rx.recv_timeout(Duration::from_millis(500));
        }
    }
}

pub async fn handle(
    ctx: &CoreContext,
    command: &str,
    operation: &str,
    payload: &Value,
    state: &mut Value,
) -> CoreResult {
    if !commands().contains(&command) {
        return Err(format!("Unsupported keep-awake command: {command}"));
    }
    match operation {
        "extension:run-command" => match command {
            "amp-duration" => Ok(form(state)),
            "amp-start" => {
                start_retained(ctx, None, false).await?;
                Ok(v2_sdk::no_view("New default keep-awake session started"))
            }
            "quick-start" => {
                let args = payload.get("argumentValues").unwrap_or(&Value::Null);
                if !args.is_object() && !args.is_null() {
                    return Err("Quick Start arguments must be an object".into());
                }
                let hours = argument(args, "hours")?;
                let minutes = argument(args, "minutes")?;
                let seconds = argument(args, "seconds")?;
                let total = hours
                    .checked_mul(3600)
                    .and_then(|n| minutes.checked_mul(60).and_then(|m| n.checked_add(m)))
                    .and_then(|n| n.checked_add(seconds))
                    .ok_or("Duration is too large")?;
                // Source extension rounds any seconds up to a whole minute.
                let rounded = total.checked_add(59).ok_or("Duration is too large")? / 60;
                let duration = minutes_duration(rounded)?;
                let display_sleep_allowed = payload
                    .get("preferences")
                    .and_then(|p| p.get("displaySleepAllowed"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                start_retained(ctx, duration, display_sleep_allowed).await?;
                let message = if rounded == 0 {
                    "Session started with no time limit".to_string()
                } else {
                    format!(
                        "Session started for {}h {}m{}",
                        rounded / 60,
                        rounded % 60,
                        if total % 60 != 0 { " (rounded up)" } else { "" }
                    )
                };
                Ok(v2_sdk::no_view(&message))
            }
            "amp-end" => match end_retained(ctx).await? {
                Reply::Ended(true) => Ok(v2_sdk::no_view("Ended current session.")),
                Reply::Ended(false) => Err("No keep-awake session is running.".into()),
                _ => Err("Unexpected keep-awake end response".into()),
            },
            "remaining" => {
                let status = send_async(Request::Status).await?;
                let message = match status {
                    Reply::Status(None) => "There is no session running.".to_string(),
                    Reply::Status(Some(None)) => {
                        "Your current session has infinite duration.".to_string()
                    }
                    Reply::Status(Some(Some(seconds))) => {
                        format!("Remaining time: {}", format_remaining(seconds))
                    }
                    _ => return Err("Unexpected keep-awake status".into()),
                };
                Ok(v2_sdk::no_view(&message))
            }
            _ => unreachable!(),
        },
        "extension:invoke-action" if command == "amp-duration" => {
            let action = payload
                .get("actionId")
                .and_then(Value::as_str)
                .unwrap_or("");
            if action == SET_TYPE {
                let next = payload
                    .get("formValues")
                    .and_then(|v| v.get("value"))
                    .and_then(Value::as_str)
                    .ok_or("Session type must be text")?;
                if !matches!(next, "duration" | "time") {
                    return Err("Invalid session type".into());
                }
                *state = json!({"sessionType":next});
                return Ok(form(state));
            }
            if action != SUBMIT {
                return Err("Action is no longer available in this session.".into());
            }
            let values = payload.get("formValues").ok_or("Missing session form")?;
            let kind = values
                .get("sessionType")
                .and_then(Value::as_str)
                .unwrap_or("duration");
            let duration = match kind {
                "duration" => {
                    let raw = values
                        .get("duration")
                        .and_then(Value::as_str)
                        .unwrap_or("30");
                    if raw.trim().is_empty() {
                        None
                    } else {
                        let unit = values
                            .get("interval")
                            .and_then(Value::as_str)
                            .unwrap_or("minutes");
                        if !matches!(unit, "minutes" | "hours") {
                            return Err("Invalid duration unit".into());
                        }
                        let number: f64 =
                            raw.trim().parse().map_err(|_| "The duration is invalid")?;
                        if !number.is_finite() || number < 0.0 {
                            return Err("Duration must be a non-negative finite number".into());
                        }
                        let minutes = number * if unit == "hours" { 60.0 } else { 1.0 };
                        if minutes > MAX_MINUTES as f64 {
                            return Err("Duration exceeds the 10-year safety limit".into());
                        }
                        // Unlike the original AppleScript's fractional duration, the
                        // native assertion rounds up so it never expires early.
                        minutes_duration(minutes.ceil() as u64)?
                    }
                }
                "time" => {
                    let date = values
                        .get("targetDate")
                        .and_then(Value::as_str)
                        .ok_or("Choose a future date.")?;
                    let time = values
                        .get("targetTime")
                        .and_then(Value::as_str)
                        .ok_or("Enter a time (HH:MM).")?;
                    if !valid_time(time) {
                        return Err("Enter a valid time in HH:MM format.".into());
                    }
                    until_duration(&format!("{date}T{time}"), chrono::Local::now())?
                }
                _ => return Err("Invalid session type".into()),
            };
            start_retained(ctx, duration, false).await?;
            Ok(v2_sdk::no_view(if duration.is_some() {
                "Timed keep-awake session started"
            } else {
                "Keep-awake session started with no time limit"
            }))
        }
        "extension:refresh-session" if command == "amp-duration" => Ok(form(state)),
        "extension:refresh-session" | "extension:dispose-session" => Ok(v2_sdk::unchanged()),
        _ => Err(format!("Unsupported keep-awake operation: {operation}")),
    }
}

fn form(state: &Value) -> Value {
    let kind = state
        .get("sessionType")
        .and_then(Value::as_str)
        .unwrap_or("duration");
    let mut children = vec![json!({"type":"Form.Dropdown","props":{
    "id":"sessionType","title":"Session Type","value":kind,"actionId":SET_TYPE},
    "children":[
        {"type":"Form.Dropdown.Item","props":{"value":"duration","title":"For Duration"}},
        {"type":"Form.Dropdown.Item","props":{"value":"time","title":"Until Time"}}
    ]})];
    if kind == "time" {
        // Current Tezbar Form.DatePicker renders a date-only HTML input, so
        // compose it with a separate HH:MM field rather than silently losing time.
        children.push(json!({"type":"Form.DatePicker","props":{
            "id":"targetDate","title":"Until Date",
            "info":"Choose a future date."
        }}));
        children.push(json!({"type":"Form.TextField","props":{
            "id":"targetTime","title":"Until Time (HH:MM)",
            "placeholder":"18:00","info":"Local 24-hour time."
        }}));
    } else {
        children.push(json!({"type":"Form.TextField","props":{
            "id":"duration","title":"Duration","value":"30",
            "info":"Enter 0 or leave empty for no time limit."
        }}));
        children.push(json!({"type":"Form.Dropdown","props":{"id":"interval","title":"Unit","value":"minutes"},
            "children":[
                {"type":"Form.Dropdown.Item","props":{"value":"minutes","title":"minutes"}},
                {"type":"Form.Dropdown.Item","props":{"value":"hours","title":"hours"}}
            ]}));
    }
    v2_sdk::view(
        "Configure Session",
        json!({"type":"Form","children":children}),
        vec![json!({"id":SUBMIT,"title":"Start Session","kind":"submit-form","style":"default"})],
    )
}

fn argument(args: &Value, name: &str) -> Result<u64, String> {
    let value = match args.get(name) {
        None | Some(Value::Null) => "",
        Some(Value::String(s)) => s.trim(),
        _ => return Err(format!("{name}: enter whole numbers only")),
    };
    if value.is_empty() {
        return Ok(0);
    }
    if !value.bytes().all(|c| c.is_ascii_digit()) {
        return Err(format!("{name}: enter whole numbers only"));
    }
    value
        .parse()
        .map_err(|_| format!("{name}: duration is too large"))
}

fn minutes_duration(minutes: u64) -> Result<Option<Duration>, String> {
    if minutes > MAX_MINUTES {
        return Err("Duration exceeds the 10-year safety limit".into());
    }
    Ok((minutes != 0).then(|| Duration::from_secs(minutes * 60)))
}

fn valid_time(time: &str) -> bool {
    use chrono::Timelike;
    let Some((hours, minutes)) = time.split_once(':') else {
        return false;
    };
    hours.len() == 2
        && minutes.len() == 2
        && hours
            .bytes()
            .chain(minutes.bytes())
            .all(|b| b.is_ascii_digit())
        && chrono::NaiveTime::from_hms_opt(
            hours.parse().unwrap_or(24),
            minutes.parse().unwrap_or(60),
            0,
        )
        .is_some_and(|t| t.hour() < 24)
}

fn until_duration(
    target: &str,
    now: chrono::DateTime<chrono::Local>,
) -> Result<Option<Duration>, String> {
    use chrono::{Local, NaiveDateTime, TimeZone};
    let instant = chrono::DateTime::parse_from_rfc3339(target)
        .map(|d| d.with_timezone(&Local))
        .map_err(|e| e.to_string())
        .or_else(|_| {
            NaiveDateTime::parse_from_str(target, "%Y-%m-%dT%H:%M:%S")
                .or_else(|_| NaiveDateTime::parse_from_str(target, "%Y-%m-%dT%H:%M"))
                .map_err(|_| "Invalid date/time".to_string())
                .and_then(|d| {
                    Local
                        .from_local_datetime(&d)
                        .single()
                        .ok_or("Ambiguous or nonexistent local time".to_string())
                })
        })
        .map_err(|_| "Choose a valid future date and time.")?;
    let difference = instant.signed_duration_since(now).num_milliseconds();
    if difference <= 0 {
        return Err("Choose a future date and time.".into());
    }
    let minutes = ((difference as u64) / 60_000) + u64::from(difference % 60_000 != 0);
    minutes_duration(minutes)
}

fn format_remaining(seconds: u64) -> String {
    if seconds >= 3600 {
        format!("{:02}:{:02}h", seconds / 3600, (seconds % 3600) / 60)
    } else {
        format!("{:02}:{:02}min", seconds / 60, seconds % 60)
    }
}

// Reply is delivered on a separate rendezvous channel so the mutex never
// spans an await, and timed sessions run independently of the UI session.
type ReplyChannel = mpsc::SyncSender<Result<Reply, String>>;
enum Request {
    Start(Option<Duration>, bool, ReplyChannel),
    End(ReplyChannel),
    Status(ReplyChannel),
    Shutdown(mpsc::SyncSender<()>),
}
enum Reply {
    Started,
    Ended(bool),
    Status(Option<Option<u64>>), // None: inactive; Some(None): unlimited
}
static ACTOR: OnceLock<mpsc::SyncSender<Request>> = OnceLock::new();

fn actor() -> &'static mpsc::SyncSender<Request> {
    ACTOR.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel(32);
        std::thread::Builder::new()
            .name("tezbar-keep-awake".into())
            .spawn(move || run_actor(rx))
            .expect("unable to create keep-awake actor");
        tx
    })
}

async fn send_start(duration: Option<Duration>, display_sleep_allowed: bool) -> Result<(), String> {
    match tokio::task::spawn_blocking(move || {
        let _gate = ACTOR_GATE.lock().unwrap();
        if SHUTTING_DOWN.load(Ordering::Acquire) {
            return Err("Keep-awake process is shutting down".to_string());
        }
        let (tx, rx) = mpsc::sync_channel(1);
        actor()
            .send(Request::Start(duration, display_sleep_allowed, tx))
            .map_err(|_| "Keep-awake actor stopped".to_string())?;
        rx.recv_timeout(Duration::from_secs(2))
            .map_err(|_| "Keep-awake actor disconnected".to_string())?
    })
    .await
    .map_err(|e| e.to_string())??
    {
        Reply::Started => Ok(()),
        _ => Err("Unexpected keep-awake start response".into()),
    }
}

async fn send_async(request: fn(ReplyChannel) -> Request) -> Result<Reply, String> {
    tokio::task::spawn_blocking(move || send(request))
        .await
        .map_err(|e| e.to_string())?
}

fn send(request: fn(ReplyChannel) -> Request) -> Result<Reply, String> {
    let (tx, rx) = mpsc::sync_channel(1);
    actor()
        .send(request(tx))
        .map_err(|_| "Keep-awake actor stopped")?;
    rx.recv_timeout(Duration::from_secs(2))
        .map_err(|_| "Keep-awake actor disconnected".to_string())?
}

trait Assertion {
    fn start(&mut self, allow_display_sleep: bool) -> Result<(), String>;
    fn stop(&mut self);
    fn alive(&mut self) -> bool;
}

struct Session {
    deadline: Option<Instant>,
}

fn run_actor(rx: mpsc::Receiver<Request>) {
    run_actor_with(rx, PlatformAssertion::default());
}

fn run_actor_with(rx: mpsc::Receiver<Request>, mut assertion: impl Assertion) {
    let mut session: Option<Session> = None;
    loop {
        let now = Instant::now();
        expire(&mut assertion, &mut session, now);
        let request = if let Some(deadline) = session.as_ref().and_then(|s| s.deadline) {
            match rx.recv_timeout(deadline.saturating_duration_since(now)) {
                Ok(request) => request,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(request) => request,
                Err(_) => break,
            }
        };
        expire(&mut assertion, &mut session, Instant::now());
        match request {
            Request::Start(duration, allowed, reply) => {
                let result = Instant::now()
                    .checked_add(duration.unwrap_or_default())
                    .ok_or_else(|| "Duration cannot be represented by this system".to_string())
                    .and_then(|deadline| assertion.start(allowed).map(|_| deadline))
                    .map(|deadline| {
                        session = Some(Session {
                            deadline: duration.map(|_| deadline),
                        });
                        Reply::Started
                    });
                let _ = reply.send(result);
            }
            Request::End(reply) => {
                let had_session = session.take().is_some();
                if had_session {
                    assertion.stop();
                }
                let _ = reply.send(Ok(Reply::Ended(had_session)));
            }
            Request::Status(reply) => {
                if session.is_some() && !assertion.alive() {
                    assertion.stop();
                    session = None;
                }
                let status = session.as_ref().map(|s| {
                    s.deadline.map(|deadline| {
                        // Source formats whole seconds; ceil avoids falsely showing zero.
                        let left = deadline.saturating_duration_since(Instant::now());
                        left.as_secs() + u64::from(left.subsec_nanos() != 0)
                    })
                });
                let _ = reply.send(Ok(Reply::Status(status)));
            }
            Request::Shutdown(reply) => {
                assertion.stop();
                let _ = reply.send(());
                return;
            }
        }
    }
    assertion.stop();
}

fn expire(assertion: &mut impl Assertion, session: &mut Option<Session>, now: Instant) {
    if session
        .as_ref()
        .and_then(|s| s.deadline)
        .is_some_and(|d| now >= d)
    {
        assertion.stop();
        *session = None;
    }
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct PlatformAssertion(Option<std::process::Child>);

#[cfg(target_os = "macos")]
impl Assertion for PlatformAssertion {
    fn start(&mut self, allow_display_sleep: bool) -> Result<(), String> {
        let mut command = std::process::Command::new("/usr/bin/caffeinate");
        command.arg("-i");
        if !allow_display_sleep {
            command.arg("-d");
        }
        // -w causes caffeinate to release its assertion even when Tezbar
        // crashes. Timed expiry is separately enforced by this actor.
        command.arg("-w").arg(std::process::id().to_string());
        let mut child = command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("Could not start caffeinate: {e}"))?;
        match child.try_wait() {
            Ok(Some(status)) => return Err(format!("caffeinate exited before starting: {status}")),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("Could not verify caffeinate process: {error}"));
            }
            Ok(None) => {}
        }
        self.stop();
        self.0 = Some(child);
        Ok(())
    }
    fn stop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    fn alive(&mut self) -> bool {
        self.0
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
    }
}

#[cfg(target_os = "windows")]
#[derive(Default)]
struct PlatformAssertion(bool);

#[cfg(target_os = "windows")]
impl Assertion for PlatformAssertion {
    fn start(&mut self, allow_display_sleep: bool) -> Result<(), String> {
        const ES_CONTINUOUS: u32 = 0x8000_0000;
        const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
        const ES_DISPLAY_REQUIRED: u32 = 0x0000_0002;
        #[link(name = "kernel32")]
        extern "system" {
            fn SetThreadExecutionState(flags: u32) -> u32;
        }
        let flags = ES_CONTINUOUS
            | ES_SYSTEM_REQUIRED
            | if allow_display_sleep {
                0
            } else {
                ES_DISPLAY_REQUIRED
            };
        if unsafe { SetThreadExecutionState(flags) } == 0 {
            return Err(format!(
                "Windows execution-state error: {}",
                std::io::Error::last_os_error()
            ));
        }
        self.0 = true;
        Ok(())
    }
    fn stop(&mut self) {
        if self.0 {
            #[link(name = "kernel32")]
            extern "system" {
                fn SetThreadExecutionState(flags: u32) -> u32;
            }
            unsafe {
                SetThreadExecutionState(0x8000_0000);
            }
            self.0 = false;
        }
    }
    fn alive(&mut self) -> bool {
        self.0
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[derive(Default)]
struct PlatformAssertion;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl Assertion for PlatformAssertion {
    fn start(&mut self, _: bool) -> Result<(), String> {
        Err("Keep-awake requires macOS or Windows".into())
    }
    fn stop(&mut self) {}
    fn alive(&mut self) -> bool {
        false
    }
}

// The process context survives view disposal. The OS actor remains thread-affine;
// the background lease is non-durable and is never restored after host restart.
use std::sync::atomic::{AtomicBool, Ordering};
static PROCESS: std::sync::Mutex<Option<CoreContext>> = std::sync::Mutex::new(None);
static LEASED: AtomicBool = AtomicBool::new(false);
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);
static ACTOR_GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
static CONTROL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
const LEASE: &str = "keep-awake";

pub fn initialize(ctx: &CoreContext) -> Result<(), String> {
    {
        let mut process = PROCESS.lock().unwrap();
        if process.is_some() {
            return Err("Keep-awake process already initialized".into());
        }
        *process = Some(ctx.clone());
    }
    let background = ctx.clone();
    ctx.spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if !LEASED.load(Ordering::Acquire) {
                continue;
            }
            let _guard = CONTROL.lock().await;
            if !LEASED.load(Ordering::Acquire) {
                continue;
            }
            let active = matches!(
                send_async(Request::Status).await,
                Ok(Reply::Status(Some(_)))
            );
            if !active {
                let _ = release(&background).await;
            }
        }
    })
}
async fn release(ctx: &CoreContext) -> Result<(), String> {
    if LEASED.load(Ordering::Acquire) {
        ctx.host("host.background.release", json!({"key":LEASE}))
            .await?;
        LEASED.store(false, Ordering::Release);
    }
    Ok(())
}
async fn start_retained(
    _ctx: &CoreContext,
    duration: Option<Duration>,
    display: bool,
) -> Result<(), String> {
    let background = PROCESS
        .lock()
        .unwrap()
        .clone()
        .ok_or("Keep-awake process not initialized")?;
    let owner = background.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    // Process-owned transaction: disposing the submitting form cannot strand a
    // half-retained lease or interrupt the actor handoff after acknowledgement.
    owner.spawn(async move {
        let result = async {
            let _guard = CONTROL.lock().await;
            if !LEASED.load(Ordering::Acquire) {
                background
                    .host(
                        "host.background.retain",
                        json!({"key":LEASE,"durable":false}),
                    )
                    .await?;
                LEASED.store(true, Ordering::Release);
            }
            if let Err(error) = send_start(duration, display).await {
                // A failed replacement can leave the previous assertion alive.
                if !matches!(
                    send_async(Request::Status).await,
                    Ok(Reply::Status(Some(_)))
                ) {
                    let _ = release(&background).await;
                }
                return Err(error);
            }
            Ok(())
        }
        .await;
        let _ = tx.send(result);
    })?;
    rx.await
        .map_err(|_| "Keep-awake background transaction cancelled".to_string())?
}
async fn end_retained(ctx: &CoreContext) -> Result<Reply, String> {
    let _guard = CONTROL.lock().await;
    let reply = send_async(Request::End).await?;
    release(ctx).await?;
    Ok(reply)
}
pub async fn shutdown_extension(ctx: &CoreContext) -> CoreResult {
    // Stop assertions first even on pipe EOF, when host RPC is already closed.
    tokio::task::spawn_blocking(shutdown)
        .await
        .map_err(|e| e.to_string())?;
    let _ = release(ctx).await;
    PROCESS.lock().unwrap().take();
    Ok(json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct FakeAssertion(Arc<Mutex<Vec<&'static str>>>);
    impl Assertion for FakeAssertion {
        fn start(&mut self, _: bool) -> Result<(), String> {
            self.0.lock().unwrap().push("start");
            Ok(())
        }
        fn stop(&mut self) {
            self.0.lock().unwrap().push("stop");
        }
        fn alive(&mut self) -> bool {
            true
        }
    }
    #[test]
    fn duration_validation_matches_quick_start_and_rejects_overflow() {
        assert_eq!(
            argument(&json!({"hours":" 2 ","seconds":"7"}), "hours").unwrap(),
            2
        );
        assert_eq!(argument(&json!({}), "minutes").unwrap(), 0);
        assert!(argument(&json!({"hours":"1.5"}), "hours").is_err());
        assert!(argument(&json!({"seconds":"-5"}), "seconds").is_err());
        assert_eq!(minutes_duration(0).unwrap(), None);
        assert!(minutes_duration(MAX_MINUTES + 1).is_err());
    }
    #[test]
    fn future_time_is_rounded_up_and_past_time_rejected() {
        let now = chrono::Local::now();
        let future = (now + chrono::Duration::seconds(61)).to_rfc3339();
        assert_eq!(
            until_duration(&future, now).unwrap(),
            Some(Duration::from_secs(120))
        );
        assert!(until_duration(&now.to_rfc3339(), now).is_err());
        assert!(until_duration("2026-15-60", now).is_err());
    }
    #[test]
    fn expiry_releases_assertion_with_fake_clock_and_driver() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut driver = FakeAssertion(log.clone());
        driver.start(false).unwrap();
        let now = Instant::now();
        let mut session = Some(Session {
            deadline: now.checked_add(Duration::from_secs(60)),
        });
        expire(&mut driver, &mut session, now + Duration::from_secs(59));
        assert!(session.is_some());
        expire(&mut driver, &mut session, now + Duration::from_secs(60));
        assert!(session.is_none());
        assert_eq!(*log.lock().unwrap(), vec!["start", "stop"]);
    }
    #[test]
    fn form_actions_are_namespaced_and_time_field_conditional() {
        let duration = form(&json!({}));
        assert_eq!(duration["actions"][0]["id"], SUBMIT);
        assert_eq!(
            duration["root"]["children"][0]["props"]["actionId"],
            SET_TYPE
        );
        let time = form(&json!({"sessionType":"time"}));
        assert_eq!(time["root"]["children"][1]["type"], "Form.DatePicker");
        assert_eq!(time["root"]["children"][2]["type"], "Form.TextField");
        assert!(valid_time("23:59"));
        assert!(!valid_time("24:00"));
    }
    #[tokio::test]
    async fn actor_and_nondurable_lease_survive_view_close_and_release_on_expiry_end_shutdown() {
        use tezbar_extension_sdk::{ExtensionPaths, TestContext};
        let dir = tempfile::tempdir().unwrap();
        let fixture = TestContext::new(
            ExtensionPaths {
                package_dir: dir.path().join("package"),
                data_dir: dir.path().join("data"),
                temp_dir: dir.path().join("tmp"),
            },
            &["background"],
        );
        let log = Arc::new(Mutex::new(Vec::new()));
        let driver = FakeAssertion(log.clone());
        let (tx, rx) = mpsc::sync_channel(32);
        assert!(ACTOR.set(tx).is_ok());
        let worker = std::thread::spawn(move || run_actor_with(rx, driver));
        initialize(&fixture.context).unwrap();
        fixture
            .run(start_retained(
                &fixture.context,
                Some(Duration::from_millis(40)),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(fixture.calls()[0]["method"], "host.background.retain");
        assert_eq!(fixture.calls()[0]["params"]["durable"], false);
        fixture
            .run(tokio::time::sleep(Duration::from_millis(250)))
            .await;
        assert!(!LEASED.load(Ordering::Acquire));
        assert_eq!(
            fixture.calls().last().unwrap()["method"],
            "host.background.release"
        );
        fixture
            .run(start_retained(&fixture.context, None, false))
            .await
            .unwrap();
        // Disposing a view does not end the explicitly process-owned assertion.
        handle(
            &fixture.context,
            "amp-duration",
            "extension:dispose-session",
            &json!({}),
            &mut json!({}),
        )
        .await
        .unwrap();
        assert!(matches!(
            send_async(Request::Status).await.unwrap(),
            Reply::Status(Some(None))
        ));
        assert!(matches!(
            fixture.run(end_retained(&fixture.context)).await.unwrap(),
            Reply::Ended(true)
        ));
        assert!(!LEASED.load(Ordering::Acquire));
        fixture
            .run(start_retained(&fixture.context, None, false))
            .await
            .unwrap();
        fixture
            .run(shutdown_extension(&fixture.context))
            .await
            .unwrap();
        worker.join().unwrap();
        assert!(!LEASED.load(Ordering::Acquire));
        assert_eq!(
            *log.lock().unwrap(),
            vec!["start", "stop", "start", "stop", "start", "stop"]
        );
    }

    #[test]
    fn assertion_actor_keeps_all_os_calls_on_one_thread() {
        struct Driver(Arc<Mutex<Vec<(std::thread::ThreadId, bool)>>>);
        impl Assertion for Driver {
            fn start(&mut self, display: bool) -> Result<(), String> {
                self.0
                    .lock()
                    .unwrap()
                    .push((std::thread::current().id(), display));
                Ok(())
            }
            fn stop(&mut self) {
                self.0
                    .lock()
                    .unwrap()
                    .push((std::thread::current().id(), false));
            }
            fn alive(&mut self) -> bool {
                true
            }
        }
        let log = Arc::new(Mutex::new(Vec::new()));
        let record = log.clone();
        let (tx, rx) = mpsc::sync_channel(4);
        let worker = std::thread::spawn(move || run_actor_with(rx, Driver(record)));
        let (reply, result) = mpsc::sync_channel(1);
        tx.send(Request::Start(None, true, reply)).unwrap();
        assert!(matches!(result.recv().unwrap().unwrap(), Reply::Started));
        let (reply, result) = mpsc::sync_channel(1);
        tx.send(Request::Shutdown(reply)).unwrap();
        result.recv().unwrap();
        worker.join().unwrap();
        let seen = log.lock().unwrap();
        assert!(seen[0].1);
        assert!(seen.iter().all(|entry| entry.0 == seen[0].0));
        assert_ne!(seen[0].0, std::thread::current().id());
    }

    #[test]
    fn remaining_format_matches_source() {
        assert_eq!(format_remaining(3661), "01:01h");
        assert_eq!(format_remaining(9), "00:09min");
    }
}
