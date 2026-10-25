//! In-app probe re-run schedule.
//!
//! This is one thread in the desktop process. It is not an operating-system
//! cron job and not a background daemon. Process exit stops it: the thread
//! is not left running after the app quits.
//!
//! Nothing is armed until the user enables it. Enabling does not send.
//! The first tick is [`PROBE_SCHEDULE_INTERVAL_MS`] later (24 hours,
//! provisional / 暂定). A due check runs at most one tick. If the clock
//! jumps across several intervals, the missed ones are not caught up; the
//! next tick is one interval after that single run. Each tick is performed
//! by the caller, which must use the same `probe_run` / `run_once` gates
//! (consent, monthly budget, same-day already-run, host bind, key variable
//! name, provisional charge, store writes) on this thread, off the async
//! runtime. A budget stop, a missing consent, or a binding that can never
//! send clears the schedule so later checks do not send.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_value_core::constants::RUN_DAY_UTC_OFFSET_SECONDS;
use agent_value_core::runday::RunDay;
use serde::Deserialize;

/// Provisional in-app re-run interval: once per day.
///
/// 24 hours, in milliseconds. 暂定. Not an observation-period threshold
/// and not an operating-system cron period.
pub const PROBE_SCHEDULE_INTERVAL_MS: i64 = 24 * 60 * 60 * 1000;

/// Shown when the monthly budget stops the schedule. Further ticks do not send.
pub const SCHEDULE_BUDGET_STOP_LABEL: &str = "已超过月预算，已停止后续定时探测，不会继续重试。";

/// Shown when a tick or an enable finds no stored acceptance.
pub const SCHEDULE_NO_CONSENT_LABEL: &str = "尚未同意，不会定时探测。";

/// Shown when a binding can never send (arm gone, host bind, protocol).
/// The sentence does not repeat a URL, a host, or a token.
pub const SCHEDULE_HALT_LABEL: &str = "定时已停止。这次绑定无法发送，不会继续重试。";

const MS_PER_DAY: i64 = 86_400_000;

/// Arm, key variable name, and base URL for a later tick.
///
/// `key_env` is the variable name, never the token. The tick still host-binds
/// the base to the selected arm before any socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleBinding {
    pub arm_id: String,
    pub key_env: String,
    pub base_url: String,
}

/// What the caller's one run decided. The schedule does not send HTTP itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickClass {
    /// The gates ran. Keep the next interval that was already stored.
    Continue,
    /// Monthly budget stop. Do not call the runner again.
    Budget,
    /// Consent missing. Do not call the runner again.
    Consent,
    /// The binding cannot send. Do not call the runner again.
    Halt,
}

/// Whether this check called the runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TickReport {
    pub ran: bool,
    pub class: Option<TickClass>,
}

/// Fields the probe panel shows. The next-planned sentence is read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleSnapshot {
    pub active: bool,
    pub next_planned_label: Option<String>,
    pub status_label: Option<String>,
    pub hint: String,
}

/// What the webview sends to enable or stop the schedule.
#[derive(Debug, Deserialize)]
pub struct ScheduleInput {
    pub enabled: bool,
    pub arm_id: String,
    pub key_env: String,
    pub base_url: String,
}

struct Inner {
    active: bool,
    next_at_ms: Option<i64>,
    binding: Option<ScheduleBinding>,
    status_label: Option<String>,
    shutdown: bool,
}

impl Inner {
    fn idle() -> Self {
        Self {
            active: false,
            next_at_ms: None,
            binding: None,
            status_label: None,
            shutdown: false,
        }
    }

    fn disarm(&mut self) {
        self.active = false;
        self.next_at_ms = None;
        self.binding = None;
    }
}

/// Process-local schedule. Dropping the process stops every wait.
pub struct ProbeSchedule {
    inner: Mutex<Inner>,
    cv: Condvar,
}

impl ProbeSchedule {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::idle()),
            cv: Condvar::new(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.lock().active
    }

    pub fn snapshot(&self) -> ScheduleSnapshot {
        let guard = self.lock();
        let next_planned_label = if guard.active {
            Some(match guard.next_at_ms {
                Some(next) => next_planned_label(next),
                None => fallback_planned_label(),
            })
        } else {
            None
        };
        ScheduleSnapshot {
            active: guard.active,
            next_planned_label,
            status_label: guard.status_label.clone(),
            hint: schedule_hint(),
        }
    }

    /// Arms the schedule. Does not run a probe. The first tick is one
    /// interval after `now_ms`. A later enable resets that time and does
    /// not send.
    pub fn enable(&self, now_ms: i64, binding: ScheduleBinding) {
        let mut guard = self.lock();
        if guard.shutdown {
            return;
        }
        let Some(next) = now_ms.checked_add(PROBE_SCHEDULE_INTERVAL_MS) else {
            guard.disarm();
            guard.status_label = Some("定时未能排上。".to_string());
            self.cv.notify_all();
            return;
        };
        guard.active = true;
        guard.next_at_ms = Some(next);
        guard.binding = Some(binding);
        guard.status_label = None;
        self.cv.notify_all();
    }

    /// User stop. Does not send. Clears the next-planned time.
    pub fn disable(&self) {
        let mut guard = self.lock();
        guard.disarm();
        guard.status_label = None;
        self.cv.notify_all();
    }

    /// Records a refusal. Does not arm and does not send.
    pub fn note(&self, label: &str) {
        let mut guard = self.lock();
        guard.disarm();
        guard.status_label = Some(label.to_string());
        self.cv.notify_all();
    }

    /// Clears the schedule after a monthly budget stop. Does not send.
    pub fn stop_for_budget(&self) {
        let mut guard = self.lock();
        guard.disarm();
        guard.status_label = Some(SCHEDULE_BUDGET_STOP_LABEL.to_string());
        self.cv.notify_all();
    }

    /// Drops a stored status sentence when `drop_label` says that gate no
    /// longer matches. Does not arm or disarm. A stopped schedule stays off.
    pub fn clear_status_if(&self, drop_label: impl Fn(&str) -> bool) {
        let mut guard = self.lock();
        let stale = guard.status_label.as_deref().is_some_and(drop_label);
        if stale {
            guard.status_label = None;
        }
    }

    /// Wakes the worker so process exit does not leave it waiting.
    pub fn shutdown(&self) {
        let mut guard = self.lock();
        guard.shutdown = true;
        guard.disarm();
        self.cv.notify_all();
    }

    /// One check at `now_ms`.
    ///
    /// The runner is called only when the schedule is active, consent is
    /// accepted, and `now_ms` is at or after the planned time. Several
    /// missed intervals still call it once. The next planned time becomes
    /// `now_ms` plus one interval, so a second check at the same `now_ms`
    /// does not run.
    pub fn tick(
        &self,
        now_ms: i64,
        consent: bool,
        run: impl FnOnce(&ScheduleBinding) -> TickClass,
    ) -> TickReport {
        let binding = {
            let mut guard = self.lock();
            if guard.shutdown || !guard.active {
                return TickReport {
                    ran: false,
                    class: None,
                };
            }
            let Some(next) = guard.next_at_ms else {
                return TickReport {
                    ran: false,
                    class: None,
                };
            };
            if now_ms < next {
                return TickReport {
                    ran: false,
                    class: None,
                };
            }
            if !consent {
                guard.disarm();
                guard.status_label = Some(SCHEDULE_NO_CONSENT_LABEL.to_string());
                self.cv.notify_all();
                return TickReport {
                    ran: false,
                    class: Some(TickClass::Consent),
                };
            }
            let Some(binding) = guard.binding.clone() else {
                guard.disarm();
                return TickReport {
                    ran: false,
                    class: None,
                };
            };
            match now_ms.checked_add(PROBE_SCHEDULE_INTERVAL_MS) {
                Some(next_at) => guard.next_at_ms = Some(next_at),
                None => {
                    guard.disarm();
                    guard.status_label = Some("定时已停止。".to_string());
                    return TickReport {
                        ran: false,
                        class: None,
                    };
                }
            }
            binding
        };
        let class = run(&binding);
        if let Some(label) = disarm_label(class) {
            let mut guard = self.lock();
            guard.disarm();
            guard.status_label = Some(label.to_string());
            self.cv.notify_all();
        }
        TickReport {
            ran: true,
            class: Some(class),
        }
    }

    /// Blocks until a tick is due. `Some(now)` is the clock used for that
    /// tick. `None` means the app is shutting down.
    ///
    /// The lock is not held after this returns, so the caller can run a
    /// blocking probe on this same thread.
    pub fn park_until_due(&self) -> Option<i64> {
        let mut guard = self.lock();
        loop {
            if guard.shutdown {
                return None;
            }
            let Some(now) = system_now_ms() else {
                let (next_guard, _) = self
                    .cv
                    .wait_timeout(guard, Duration::from_secs(1))
                    .unwrap_or_else(|err| err.into_inner());
                guard = next_guard;
                continue;
            };
            if guard.active {
                if let Some(next) = guard.next_at_ms {
                    if now >= next {
                        return Some(now);
                    }
                    let wait_ms = next.saturating_sub(now).min(PROBE_SCHEDULE_INTERVAL_MS);
                    let dur = Duration::from_millis(u64::try_from(wait_ms).unwrap_or(0));
                    let (next_guard, _) = self
                        .cv
                        .wait_timeout(guard, dur)
                        .unwrap_or_else(|err| err.into_inner());
                    guard = next_guard;
                    continue;
                }
            }
            guard = self.cv.wait(guard).unwrap_or_else(|err| err.into_inner());
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|err| err.into_inner())
    }
}

impl Default for ProbeSchedule {
    fn default() -> Self {
        Self::new()
    }
}

/// Starts the schedule thread. The closure runs on that thread, not on the
/// Tauri async runtime, so a blocking HTTP client is the same shape as
/// `spawn_blocking`. The thread waits until a tick is due; it does not send
/// by itself. [`retain_worker`] keeps the handle for the process lifetime.
/// [`ProbeSchedule::shutdown`] ends the wait on app exit.
pub fn spawn_worker(
    schedule: Arc<ProbeSchedule>,
    mut on_due: impl FnMut(i64) + Send + 'static,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("probe-schedule".into())
        .spawn(move || {
            while let Some(now) = schedule.park_until_due() {
                on_due(now);
            }
        })
}

/// Holds the worker handle so it is not dropped (which would detach it) and
/// so the process can shut the thread down through [`ProbeSchedule::shutdown`].
pub fn retain_worker(handle: JoinHandle<()>) {
    static HELD: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);
    *HELD.lock().unwrap_or_else(|err| err.into_inner()) = Some(handle);
}

pub fn system_now_ms() -> Option<i64> {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(elapsed.as_millis()).ok()
}

/// Provisional interval sentence. The webview shows this text; it does not
/// choose the interval.
pub fn schedule_hint() -> String {
    let hours = PROBE_SCHEDULE_INTERVAL_MS / 3_600_000;
    format!(
        "定时重跑间隔暂定为 {hours} 小时。启用后第一次在下一间隔，错过的间隔不补跑。退出应用即停止，不是系统定时任务。"
    )
}

/// Read-only next fire time in Beijing. The hour and minute are not editable.
pub fn next_planned_label(next_at_ms: i64) -> String {
    let hours = PROBE_SCHEDULE_INTERVAL_MS / 3_600_000;
    match beijing_clock(next_at_ms) {
        Some((day, hh, mm)) => format!(
            "下次计划：{day} {hh:02}:{mm:02}（北京时间）。间隔暂定为 {hours} 小时，错过不补跑。"
        ),
        None => fallback_planned_label(),
    }
}

fn disarm_label(class: TickClass) -> Option<&'static str> {
    match class {
        TickClass::Continue => None,
        TickClass::Budget => Some(SCHEDULE_BUDGET_STOP_LABEL),
        TickClass::Consent => Some(SCHEDULE_NO_CONSENT_LABEL),
        TickClass::Halt => Some(SCHEDULE_HALT_LABEL),
    }
}

fn fallback_planned_label() -> String {
    let hours = PROBE_SCHEDULE_INTERVAL_MS / 3_600_000;
    format!("下次计划：已排期。间隔暂定为 {hours} 小时，错过不补跑。")
}

fn beijing_clock(unix_ms: i64) -> Option<(String, u32, u32)> {
    let day = RunDay::from_unix_ms(unix_ms).ok()?;
    let offset_ms = i64::from(RUN_DAY_UTC_OFFSET_SECONDS).checked_mul(1000)?;
    let local = unix_ms.checked_add(offset_ms)?;
    let ms_of_day = local.rem_euclid(MS_PER_DAY);
    let minutes = u32::try_from(ms_of_day / 60_000).ok()?;
    let hh = minutes / 60;
    let mm = minutes % 60;
    if hh >= 24 {
        return None;
    }
    Some((day.to_string(), hh, mm))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use super::*;

    fn binding() -> ScheduleBinding {
        ScheduleBinding {
            arm_id: "arm".into(),
            key_env: "RELAY_API_KEY".into(),
            base_url: "https://relay.example".into(),
        }
    }

    #[test]
    fn the_interval_is_one_provisional_day() {
        assert_eq!(PROBE_SCHEDULE_INTERVAL_MS, 86_400_000);
        let hint = schedule_hint();
        assert!(hint.contains("暂定"));
        assert!(hint.contains("24"));
        assert!(hint.contains("不是系统定时任务"));
        assert!(!hint.contains("已校准"));
        assert_eq!(
            next_planned_label(86_400_000),
            "下次计划：1970-01-02 08:00（北京时间）。间隔暂定为 24 小时，错过不补跑。"
        );
    }

    #[test]
    fn enabling_does_not_run_and_the_first_tick_is_one_interval_later() {
        let schedule = ProbeSchedule::new();
        let calls = AtomicUsize::new(0);
        schedule.enable(0, binding());
        let snap = schedule.snapshot();
        assert!(snap.active);
        assert_eq!(
            snap.next_planned_label.as_deref(),
            Some("下次计划：1970-01-02 08:00（北京时间）。间隔暂定为 24 小时，错过不补跑。")
        );
        let early = schedule.tick(PROBE_SCHEDULE_INTERVAL_MS - 1, true, |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            TickClass::Continue
        });
        assert!(!early.ran);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let due = schedule.tick(PROBE_SCHEDULE_INTERVAL_MS, true, |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            TickClass::Continue
        });
        assert!(due.ran);
        assert_eq!(due.class, Some(TickClass::Continue));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_clock_jump_runs_one_tick_and_does_not_catch_up() {
        let schedule = ProbeSchedule::new();
        let calls = AtomicUsize::new(0);
        schedule.enable(0, binding());
        let far = PROBE_SCHEDULE_INTERVAL_MS.saturating_mul(3);
        let first = schedule.tick(far, true, |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            TickClass::Continue
        });
        assert!(first.ran);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let same = schedule.tick(far, true, |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            TickClass::Continue
        });
        assert!(!same.ran);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let next = schedule.tick(far + PROBE_SCHEDULE_INTERVAL_MS, true, |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            TickClass::Continue
        });
        assert!(next.ran);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(schedule.snapshot().active);
    }

    #[test]
    fn no_consent_does_not_run_the_tick() {
        let schedule = ProbeSchedule::new();
        schedule.enable(0, binding());
        let far = PROBE_SCHEDULE_INTERVAL_MS.saturating_mul(5);
        let blocked = schedule.tick(far, false, |_| panic!("runner called without consent"));
        assert!(!blocked.ran);
        assert_eq!(blocked.class, Some(TickClass::Consent));
        assert!(!schedule.snapshot().active);
        assert_eq!(
            schedule.snapshot().status_label.as_deref(),
            Some(SCHEDULE_NO_CONSENT_LABEL)
        );
        let later = schedule.tick(far, true, |_| panic!("disarmed schedule ran"));
        assert!(!later.ran);
    }

    #[test]
    fn a_budget_stop_runs_one_tick_and_then_stops() {
        let schedule = ProbeSchedule::new();
        let calls = AtomicUsize::new(0);
        schedule.enable(0, binding());
        let due = PROBE_SCHEDULE_INTERVAL_MS;
        let stopped = schedule.tick(due, true, |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            TickClass::Budget
        });
        assert!(stopped.ran);
        assert_eq!(stopped.class, Some(TickClass::Budget));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let snap = schedule.snapshot();
        assert!(!snap.active);
        assert!(snap.next_planned_label.is_none());
        assert_eq!(
            snap.status_label.as_deref(),
            Some(SCHEDULE_BUDGET_STOP_LABEL)
        );
        let later = schedule.tick(due.saturating_mul(10), true, |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            TickClass::Budget
        });
        assert!(!later.ran);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_halt_disarms_and_does_not_run_again() {
        let schedule = ProbeSchedule::new();
        let calls = AtomicUsize::new(0);
        schedule.enable(0, binding());
        let due = PROBE_SCHEDULE_INTERVAL_MS;
        let halted = schedule.tick(due, true, |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            TickClass::Halt
        });
        assert!(halted.ran);
        assert_eq!(halted.class, Some(TickClass::Halt));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let snap = schedule.snapshot();
        assert!(!snap.active);
        assert!(snap.next_planned_label.is_none());
        assert_eq!(snap.status_label.as_deref(), Some(SCHEDULE_HALT_LABEL));
        let later = schedule.tick(due.saturating_mul(8), true, |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            TickClass::Halt
        });
        assert!(!later.ran);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_worker_does_not_run_before_a_tick_is_due_and_stops_on_shutdown() {
        let schedule = Arc::new(ProbeSchedule::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let handle = spawn_worker(Arc::clone(&schedule), move |_now| {
            counted.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        let now = system_now_ms().unwrap();
        schedule.enable(now, binding());
        thread::sleep(Duration::from_millis(80));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(schedule.snapshot().active);
        schedule.shutdown();
        handle.join().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(!schedule.snapshot().active);
    }
}
