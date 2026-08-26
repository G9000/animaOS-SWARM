//! `anima-schedule` is the host-agnostic scheduling engine for proactive
//! personal-assistant agents.
//!
//! The crate contains pure, deterministic logic only: no async runtime, no
//! wall-clock access, no I/O beyond explicit file loading. A host (e.g. the
//! Rust daemon) drives a [`Scheduler`] from its own event loop by calling
//! [`Scheduler::due_jobs`] with the current time in milliseconds since the
//! unix epoch.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

const MINUTE_MS: i64 = 60_000;
const DAY_MS: i64 = 86_400_000;
const MAX_TZ_OFFSET_MINUTES: i32 = 14 * 60;

/// When a job fires.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum ScheduleTrigger {
    /// Fire every `intervalSecs` seconds.
    #[serde(rename_all = "camelCase")]
    Every { interval_secs: u64 },
    /// Fire once per day at a local wall-clock time.
    #[serde(rename_all = "camelCase")]
    DailyAt {
        hour: u8,
        minute: u8,
        tz_offset_minutes: i32,
    },
}

/// A proactive job: run `agent_name` with `prompt` when `trigger` is due.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledJob {
    pub name: String,
    pub agent_name: String,
    pub prompt: String,
    pub trigger: ScheduleTrigger,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

/// Per-job runtime state: when the job last fired.
#[derive(Clone, Debug, Default)]
struct FireState {
    /// Last fire time in millis since the unix epoch (`Every` trigger).
    last_fired_ms: Option<u64>,
    /// Local calendar day (days since the unix epoch, in the job's timezone)
    /// for which the job already fired (`DailyAt` trigger).
    last_fired_day: Option<i64>,
}

/// A deterministic, tick-driven scheduler for [`ScheduledJob`]s.
#[derive(Debug)]
pub struct Scheduler {
    jobs: Vec<ScheduledJob>,
    state: Vec<FireState>,
}

impl Scheduler {
    /// Create a scheduler from a set of jobs.
    ///
    /// Validation rejects:
    /// - empty `name`, `agent_name`, or `prompt`
    /// - `Every` triggers with `interval_secs == 0`
    /// - `DailyAt` triggers with `hour >= 24`, `minute >= 60`, or
    ///   `|tz_offset_minutes| > 14 * 60`
    /// - duplicate job names
    pub fn new(jobs: Vec<ScheduledJob>) -> Result<Self, String> {
        let mut seen = HashSet::new();
        for job in &jobs {
            if job.name.trim().is_empty() {
                return Err("job name must not be empty".to_string());
            }
            if job.agent_name.trim().is_empty() {
                return Err(format!("job '{}': agent_name must not be empty", job.name));
            }
            if job.prompt.trim().is_empty() {
                return Err(format!("job '{}': prompt must not be empty", job.name));
            }
            if !seen.insert(job.name.clone()) {
                return Err(format!("duplicate job name '{}'", job.name));
            }
            match &job.trigger {
                ScheduleTrigger::Every { interval_secs } => {
                    if *interval_secs == 0 {
                        return Err(format!(
                            "job '{}': interval_secs must be greater than 0",
                            job.name
                        ));
                    }
                }
                ScheduleTrigger::DailyAt {
                    hour,
                    minute,
                    tz_offset_minutes,
                } => {
                    if *hour >= 24 {
                        return Err(format!("job '{}': hour must be < 24", job.name));
                    }
                    if *minute >= 60 {
                        return Err(format!("job '{}': minute must be < 60", job.name));
                    }
                    if tz_offset_minutes.abs() > MAX_TZ_OFFSET_MINUTES {
                        return Err(format!(
                            "job '{}': tz_offset_minutes must be within +/-{}",
                            job.name, MAX_TZ_OFFSET_MINUTES
                        ));
                    }
                }
            }
        }
        let state = vec![FireState::default(); jobs.len()];
        Ok(Self { jobs, state })
    }

    /// All registered jobs, in registration order.
    pub fn jobs(&self) -> &[ScheduledJob] {
        &self.jobs
    }

    /// Returns the enabled jobs due at `now_ms` and records them as fired.
    ///
    /// - `Every`: due when `now_ms - last_fired >= interval_secs * 1000`.
    ///   Never-fired jobs are due immediately on the first tick.
    /// - `DailyAt`: due when the local time (`now_ms` shifted by the timezone
    ///   offset) has reached or passed `hour:minute` and the job has not
    ///   already fired for that local calendar day.
    pub fn due_jobs(&mut self, now_ms: u64) -> Vec<ScheduledJob> {
        let mut due = Vec::new();
        for (idx, job) in self.jobs.iter().enumerate() {
            if !job.enabled {
                continue;
            }
            let fired = match &job.trigger {
                ScheduleTrigger::Every { interval_secs } => {
                    let interval_ms = interval_secs.saturating_mul(1000);
                    match self.state[idx].last_fired_ms {
                        None => true,
                        Some(last) => now_ms.saturating_sub(last) >= interval_ms,
                    }
                }
                ScheduleTrigger::DailyAt {
                    hour,
                    minute,
                    tz_offset_minutes,
                } => {
                    let local_ms = local_ms(now_ms, *tz_offset_minutes);
                    let day = local_ms.div_euclid(DAY_MS);
                    let reached = local_minutes_of_day(local_ms) >= u32::from(*hour) * 60 + u32::from(*minute);
                    reached && self.state[idx].last_fired_day != Some(day)
                }
            };
            if fired {
                match &job.trigger {
                    ScheduleTrigger::Every { .. } => {
                        self.state[idx].last_fired_ms = Some(now_ms);
                    }
                    ScheduleTrigger::DailyAt {
                        tz_offset_minutes, ..
                    } => {
                        let local_ms = local_ms(now_ms, *tz_offset_minutes);
                        self.state[idx].last_fired_day = Some(local_ms.div_euclid(DAY_MS));
                    }
                }
                due.push(job.clone());
            }
        }
        due
    }

    /// Millis until the next enabled job could be due (for sleep hints).
    ///
    /// Returns `None` when there are no enabled jobs. Returns `Some(0)` when a
    /// job is due at `now_ms` itself.
    pub fn next_due_in_ms(&self, now_ms: u64) -> Option<u64> {
        let mut best: Option<u64> = None;
        for (idx, job) in self.jobs.iter().enumerate() {
            if !job.enabled {
                continue;
            }
            let wait = match &job.trigger {
                ScheduleTrigger::Every { interval_secs } => {
                    let interval_ms = interval_secs.saturating_mul(1000);
                    match self.state[idx].last_fired_ms {
                        None => 0,
                        Some(last) => interval_ms.saturating_sub(now_ms.saturating_sub(last)),
                    }
                }
                ScheduleTrigger::DailyAt {
                    hour,
                    minute,
                    tz_offset_minutes,
                } => {
                    let local_ms = local_ms(now_ms, *tz_offset_minutes);
                    let day = local_ms.div_euclid(DAY_MS);
                    let target_min = i64::from(*hour) * 60 + i64::from(*minute);
                    let fired_today = self.state[idx].last_fired_day == Some(day);
                    let reached = local_minutes_of_day(local_ms) as i64 >= target_min;
                    let (fire_day, now_within_day_ms) = if !fired_today && reached {
                        // Due right now.
                        return Some(0);
                    } else if !fired_today {
                        // Later today.
                        (day, local_ms.rem_euclid(DAY_MS))
                    } else {
                        // Tomorrow.
                        (day + 1, local_ms.rem_euclid(DAY_MS))
                    };
                    let fire_local_ms = fire_day * DAY_MS + target_min * MINUTE_MS;
                    (fire_local_ms - now_within_day_ms).max(0) as u64
                }
            };
            best = Some(match best {
                None => wait,
                Some(current) => current.min(wait),
            });
        }
        best
    }
}

/// Shift a unix-epoch timestamp into a local wall-clock timeline.
fn local_ms(now_ms: u64, tz_offset_minutes: i32) -> i64 {
    now_ms as i64 + i64::from(tz_offset_minutes) * MINUTE_MS
}

/// Minutes elapsed since local midnight for a local-timeline timestamp.
fn local_minutes_of_day(local_ms: i64) -> u32 {
    (local_ms.rem_euclid(DAY_MS) / MINUTE_MS) as u32
}

/// Load jobs from a JSON file.
///
/// Accepts either a bare array of [`ScheduledJob`] or an object of the form
/// `{ "jobs": [...] }`. A missing or unreadable file is an error; malformed
/// JSON or a wrong shape is an [`std::io::ErrorKind::InvalidData`] error with
/// the file path and parse context attached.
pub fn load_jobs_file(path: &Path) -> std::io::Result<Vec<ScheduledJob>> {
    let text = std::fs::read_to_string(path)?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| invalid_data(path, format!("malformed JSON: {e}")))?;
    let jobs_value = if value.is_object() {
        value
            .get("jobs")
            .cloned()
            .ok_or_else(|| invalid_data(path, "object form must contain a \"jobs\" array".to_string()))?
    } else {
        value
    };
    serde_json::from_value(jobs_value)
        .map_err(|e| invalid_data(path, format!("invalid job definition: {e}")))
}

fn invalid_data(path: &Path, context: String) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("{}: {context}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const HOUR_MS: u64 = 3_600_000;

    fn every_job(name: &str, interval_secs: u64) -> ScheduledJob {
        ScheduledJob {
            name: name.to_string(),
            agent_name: "assistant".to_string(),
            prompt: "check in".to_string(),
            trigger: ScheduleTrigger::Every { interval_secs },
            enabled: true,
        }
    }

    fn daily_job(name: &str, hour: u8, minute: u8, tz_offset_minutes: i32) -> ScheduledJob {
        ScheduledJob {
            name: name.to_string(),
            agent_name: "assistant".to_string(),
            prompt: "good morning".to_string(),
            trigger: ScheduleTrigger::DailyAt {
                hour,
                minute,
                tz_offset_minutes,
            },
            enabled: true,
        }
    }

    fn due_names(scheduler: &mut Scheduler, now_ms: u64) -> Vec<String> {
        scheduler
            .due_jobs(now_ms)
            .into_iter()
            .map(|j| j.name)
            .collect()
    }

    // ---------- Every trigger ----------

    #[test]
    fn every_is_due_immediately_on_first_tick() {
        let mut s = Scheduler::new(vec![every_job("a", 60)]).unwrap();
        assert_eq!(due_names(&mut s, 1_000_000), vec!["a"]);
    }

    #[test]
    fn every_waits_for_full_interval_then_fires_again() {
        let mut s = Scheduler::new(vec![every_job("a", 60)]).unwrap();
        assert_eq!(due_names(&mut s, 0), vec!["a"]);
        assert!(due_names(&mut s, 30_000).is_empty());
        assert!(due_names(&mut s, 59_999).is_empty());
        assert_eq!(due_names(&mut s, 60_000), vec!["a"]);
        assert_eq!(due_names(&mut s, 120_000), vec!["a"]);
    }

    #[test]
    fn every_fires_once_even_after_long_gap() {
        let mut s = Scheduler::new(vec![every_job("a", 60)]).unwrap();
        assert_eq!(due_names(&mut s, 0), vec!["a"]);
        // Ten intervals later: exactly one fire, not ten.
        assert_eq!(due_names(&mut s, 600_000), vec!["a"]);
        assert!(due_names(&mut s, 600_001).is_empty());
    }

    // ---------- DailyAt trigger ----------

    #[test]
    fn daily_at_fires_at_target_time() {
        // UTC (offset 0), target 09:30.
        let mut s = Scheduler::new(vec![daily_job("a", 9, 30, 0)]).unwrap();
        let t_0900 = 9 * HOUR_MS;
        let t_0930 = t_0900 + 30 * 60_000;
        assert!(due_names(&mut s, t_0900).is_empty());
        assert!(due_names(&mut s, t_0930 - 1).is_empty());
        assert_eq!(due_names(&mut s, t_0930), vec!["a"]);
    }

    #[test]
    fn daily_at_fires_late_if_tick_passes_target() {
        let mut s = Scheduler::new(vec![daily_job("a", 9, 30, 0)]).unwrap();
        // First tick of the day is already past the target: still due.
        assert_eq!(due_names(&mut s, 12 * HOUR_MS), vec!["a"]);
    }

    #[test]
    fn daily_at_does_not_fire_twice_same_day() {
        let mut s = Scheduler::new(vec![daily_job("a", 9, 30, 0)]).unwrap();
        assert_eq!(due_names(&mut s, 10 * HOUR_MS), vec!["a"]);
        assert!(due_names(&mut s, 11 * HOUR_MS).is_empty());
        assert!(due_names(&mut s, 23 * HOUR_MS).is_empty());
    }

    #[test]
    fn daily_at_fires_again_after_day_rollover() {
        let mut s = Scheduler::new(vec![daily_job("a", 9, 30, 0)]).unwrap();
        assert_eq!(due_names(&mut s, 10 * HOUR_MS), vec!["a"]);
        // Before target on day 1: not due.
        assert!(due_names(&mut s, DAY_MS as u64 + 9 * HOUR_MS).is_empty());
        // At target on day 1: due again.
        assert_eq!(
            due_names(&mut s, DAY_MS as u64 + 9 * HOUR_MS + 30 * 60_000),
            vec!["a"]
        );
    }

    #[test]
    fn daily_at_with_negative_tz_offset() {
        // UTC-5: local 09:00 == 14:00 UTC.
        let mut s = Scheduler::new(vec![daily_job("a", 9, 0, -300)]).unwrap();
        let t_1300_utc = 13 * HOUR_MS;
        let t_1400_utc = 14 * HOUR_MS;
        assert!(due_names(&mut s, t_1300_utc).is_empty());
        assert_eq!(due_names(&mut s, t_1400_utc), vec!["a"]);
        // Same local day still: not due at 23:00 UTC (18:00 local).
        assert!(due_names(&mut s, 23 * HOUR_MS).is_empty());
        // Next local day (next 14:00 UTC): due again.
        assert_eq!(due_names(&mut s, DAY_MS as u64 + t_1400_utc), vec!["a"]);
    }

    #[test]
    fn daily_at_with_positive_tz_offset() {
        // UTC+14 (Kiritimati): local 06:00 on day 1 == 16:00 UTC on day 0.
        let mut s = Scheduler::new(vec![daily_job("a", 6, 0, 840)]).unwrap();
        let t_1500_utc = 15 * HOUR_MS;
        let t_1600_utc = 16 * HOUR_MS;
        assert!(due_names(&mut s, t_1500_utc).is_empty());
        assert_eq!(due_names(&mut s, t_1600_utc), vec!["a"]);
    }

    #[test]
    fn daily_at_small_now_with_negative_offset_does_not_panic() {
        // now_ms near 0 with a negative offset pushes local time before the
        // epoch; euclidean division must keep day math sane. Local time is
        // 10:00 on local day -1, so a 09:00 job is (correctly) due.
        let mut s = Scheduler::new(vec![daily_job("a", 9, 0, -840)]).unwrap();
        assert_eq!(due_names(&mut s, 0), vec!["a"]);
        // Later that same local day: not due again.
        assert!(due_names(&mut s, 2 * HOUR_MS).is_empty());
    }

    // ---------- Disabled jobs ----------

    #[test]
    fn disabled_jobs_never_fire() {
        let mut disabled = every_job("off", 60);
        disabled.enabled = false;
        let mut s = Scheduler::new(vec![disabled, every_job("on", 60)]).unwrap();
        assert_eq!(due_names(&mut s, 0), vec!["on"]);
        assert_eq!(due_names(&mut s, 600_000), vec!["on"]);
    }

    #[test]
    fn disabled_daily_job_never_fires() {
        let mut disabled = daily_job("off", 0, 0, 0);
        disabled.enabled = false;
        let mut s = Scheduler::new(vec![disabled]).unwrap();
        assert!(due_names(&mut s, 12 * HOUR_MS).is_empty());
        assert!(due_names(&mut s, 2 * DAY_MS as u64).is_empty());
    }

    // ---------- next_due_in_ms ----------

    #[test]
    fn next_due_none_with_no_enabled_jobs() {
        let s = Scheduler::new(vec![]).unwrap();
        assert_eq!(s.next_due_in_ms(0), None);

        let mut disabled = every_job("off", 60);
        disabled.enabled = false;
        let s = Scheduler::new(vec![disabled]).unwrap();
        assert_eq!(s.next_due_in_ms(0), None);
    }

    #[test]
    fn next_due_for_every_trigger() {
        let mut s = Scheduler::new(vec![every_job("a", 60)]).unwrap();
        assert_eq!(s.next_due_in_ms(0), Some(0)); // never fired
        s.due_jobs(10_000);
        assert_eq!(s.next_due_in_ms(10_000), Some(60_000));
        assert_eq!(s.next_due_in_ms(40_000), Some(30_000));
        assert_eq!(s.next_due_in_ms(70_000), Some(0)); // already due
    }

    #[test]
    fn next_due_for_daily_at() {
        let mut s = Scheduler::new(vec![daily_job("a", 9, 30, 0)]).unwrap();
        // 08:00 UTC -> 90 minutes until 09:30.
        assert_eq!(s.next_due_in_ms(8 * HOUR_MS), Some(90 * 60_000));
        // Past target, not yet fired -> due now.
        assert_eq!(s.next_due_in_ms(10 * HOUR_MS), Some(0));
        s.due_jobs(10 * HOUR_MS);
        // Fired today -> next is tomorrow 09:30 (23.5 h away).
        assert_eq!(s.next_due_in_ms(10 * HOUR_MS), Some(86_400_000 - 30 * 60_000));
    }

    #[test]
    fn next_due_is_min_across_jobs() {
        let mut s = Scheduler::new(vec![every_job("a", 60), daily_job("b", 9, 30, 0)]).unwrap();
        s.due_jobs(8 * HOUR_MS); // fires "a" (every), not "b"
        // "a" next at +60s; "b" next in 90 min.
        assert_eq!(s.next_due_in_ms(8 * HOUR_MS), Some(60_000));
    }

    // ---------- Validation ----------

    #[test]
    fn validation_rejects_empty_fields() {
        let mut job = every_job("a", 60);
        job.name = "  ".to_string();
        assert!(Scheduler::new(vec![job]).is_err());

        let mut job = every_job("a", 60);
        job.agent_name = String::new();
        assert!(Scheduler::new(vec![job]).is_err());

        let mut job = every_job("a", 60);
        job.prompt = String::new();
        assert!(Scheduler::new(vec![job]).is_err());
    }

    #[test]
    fn validation_rejects_zero_interval() {
        let err = Scheduler::new(vec![every_job("a", 0)]).unwrap_err();
        assert!(err.contains("interval_secs"), "unexpected error: {err}");
    }

    #[test]
    fn validation_rejects_bad_daily_at() {
        assert!(Scheduler::new(vec![daily_job("a", 24, 0, 0)]).is_err());
        assert!(Scheduler::new(vec![daily_job("a", 0, 60, 0)]).is_err());
        assert!(Scheduler::new(vec![daily_job("a", 0, 0, 841)]).is_err());
        assert!(Scheduler::new(vec![daily_job("a", 0, 0, -841)]).is_err());
        // Boundary values are accepted.
        assert!(Scheduler::new(vec![daily_job("a", 23, 59, 840)]).is_ok());
        assert!(Scheduler::new(vec![daily_job("a", 0, 0, -840)]).is_ok());
    }

    #[test]
    fn validation_rejects_duplicate_names() {
        let err = Scheduler::new(vec![every_job("a", 60), daily_job("a", 9, 0, 0)]).unwrap_err();
        assert!(err.contains("duplicate"), "unexpected error: {err}");
    }

    // ---------- serde ----------

    #[test]
    fn serde_camel_case_round_trip() {
        let json = r#"{"type":"every","intervalSecs":60}"#;
        let trigger: ScheduleTrigger = serde_json::from_str(json).unwrap();
        assert_eq!(trigger, ScheduleTrigger::Every { interval_secs: 60 });
        assert_eq!(serde_json::to_string(&trigger).unwrap(), json);

        let json = r#"{"type":"dailyAt","hour":9,"minute":30,"tzOffsetMinutes":-300}"#;
        let trigger: ScheduleTrigger = serde_json::from_str(json).unwrap();
        assert_eq!(
            trigger,
            ScheduleTrigger::DailyAt {
                hour: 9,
                minute: 30,
                tz_offset_minutes: -300
            }
        );
        assert_eq!(serde_json::to_string(&trigger).unwrap(), json);
    }

    #[test]
    fn serde_job_round_trip_with_default_enabled() {
        let json = r#"{
            "name": "standup",
            "agentName": "assistant",
            "prompt": "summarize my day",
            "trigger": {"type": "dailyAt", "hour": 9, "minute": 0, "tzOffsetMinutes": 60}
        }"#;
        let job: ScheduledJob = serde_json::from_str(json).unwrap();
        assert!(job.enabled, "enabled must default to true");
        let serialized = serde_json::to_value(&job).unwrap();
        assert_eq!(serialized["enabled"], serde_json::json!(true));
        assert_eq!(serialized["agentName"], serde_json::json!("assistant"));
        assert!(serialized.get("agent_name").is_none());
        let back: ScheduledJob = serde_json::from_value(serialized).unwrap();
        assert_eq!(back, job);
    }

    // ---------- load_jobs_file ----------

    fn write_temp(name: &str, contents: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("anima-schedule-test-{name}"));
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        path
    }

    #[test]
    fn load_jobs_file_bare_array() {
        let path = write_temp(
            "bare.json",
            r#"[
                {"name":"a","agentName":"x","prompt":"p","trigger":{"type":"every","intervalSecs":60}},
                {"name":"b","agentName":"x","prompt":"p","trigger":{"type":"dailyAt","hour":9,"minute":0,"tzOffsetMinutes":0},"enabled":false}
            ]"#,
        );
        let jobs = load_jobs_file(&path).unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].name, "a");
        assert!(jobs[0].enabled);
        assert!(!jobs[1].enabled);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn load_jobs_file_object_form() {
        let path = write_temp(
            "object.json",
            r#"{"jobs":[{"name":"a","agentName":"x","prompt":"p","trigger":{"type":"every","intervalSecs":60}}]}"#,
        );
        let jobs = load_jobs_file(&path).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(
            jobs[0].trigger,
            ScheduleTrigger::Every { interval_secs: 60 }
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn load_jobs_file_missing_file_is_error() {
        let path = std::env::temp_dir().join("anima-schedule-test-does-not-exist.json");
        let err = load_jobs_file(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn load_jobs_file_malformed_json_has_context() {
        let path = write_temp("malformed.json", "{ not json !!");
        let err = load_jobs_file(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        let msg = err.to_string();
        assert!(msg.contains("malformed JSON"), "missing context: {msg}");
        assert!(msg.contains("malformed.json"), "missing path: {msg}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn load_jobs_file_object_without_jobs_key_is_error() {
        let path = write_temp("no-jobs-key.json", r#"{"other": []}"#);
        let err = load_jobs_file(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn load_jobs_file_invalid_job_shape_is_error() {
        let path = write_temp("bad-job.json", r#"[{"name":"a"}]"#);
        let err = load_jobs_file(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("invalid job definition"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn loaded_jobs_feed_scheduler() {
        let path = write_temp(
            "roundtrip.json",
            r#"{"jobs":[{"name":"a","agentName":"x","prompt":"p","trigger":{"type":"every","intervalSecs":60}}]}"#,
        );
        let jobs = load_jobs_file(&path).unwrap();
        let mut s = Scheduler::new(jobs).unwrap();
        assert_eq!(due_names(&mut s, 0), vec!["a"]);
        std::fs::remove_file(&path).ok();
    }
}
