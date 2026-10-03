//! When an automation fires (spec §9.1): the next fire time of any trigger,
//! moved into its active hours; the next few, for previews and the agent
//! minimum; and the validation every trigger passes, on creation and on
//! restore, one variant at a time.
#![allow(dead_code)] // M6 Task 5 uses every item.

use chrono::{Datelike, LocalResult, NaiveDateTime, TimeDelta, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use super::cron::parse_cron;
use super::ScheduleTrigger;

/// Window openings tried before a cron or daily trigger is declared never
/// to fire inside its active hours.
pub(crate) const MAX_WINDOW_HOPS: usize = 400;
/// Minutes a window opening inside a daylight-saving gap is moved forward,
/// at most, to the first wall time that exists.
pub(crate) const DST_GAP_SEARCH_MINUTES: i64 = 180;

pub(crate) const TIME_ZONE_INVALID: &str = "timeZone is invalid";
pub(crate) const INTERVAL_INVALID: &str = "intervalMs must be a positive whole number of seconds";
pub(crate) const DAILY_INVALID: &str = "daily trigger is invalid";
pub(crate) const ONCE_INVALID: &str = "atMs must be a positive time in milliseconds";
pub(crate) const ACTIVE_HOURS_TIME_INVALID: &str =
    "activeHours start and end must be HH:MM in 24-hour time";
pub(crate) const ACTIVE_HOURS_SAME_TIME: &str = "activeHours start and end must differ";
pub(crate) const ACTIVE_HOURS_DAYS_INVALID: &str =
    "activeHours days must list 1 to 7 different days from 0 (Sunday) to 6 (Saturday)";
pub(crate) const ACTIVE_HOURS_NOT_FOR_ONCE: &str =
    "activeHours does not apply to a one-time automation";
pub(crate) const ONCE_NOT_IN_FUTURE: &str = "atMs must be in the future";
pub(crate) const SCHEDULE_NEVER_RUNS: &str = "This schedule never runs";
pub(crate) const SCHEDULE_NEVER_IN_ACTIVE_HOURS: &str =
    "This schedule never runs inside its active hours";
/// The literal `schedules.rs` already answered for an overflowing time.
const TIMING_OVERFLOW: &str = "schedule timing overflow";

/// When an automation may fire (spec §9.1), as stored and sent: wall-clock
/// `start` and `end` (`HH:MM`), days numbered as JavaScript's `getDay()`
/// (0 is Sunday), and the zone they are read in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActiveHours {
    pub(crate) start: String,
    pub(crate) end: String,
    pub(crate) days: Vec<u8>,
    pub(crate) time_zone: String,
}

pub(crate) fn parse_time_zone(name: &str) -> Result<Tz, String> {
    name.trim()
        .parse::<Tz>()
        .map_err(|_| TIME_ZONE_INVALID.to_string())
}

/// Minutes after midnight of an `HH:MM` wall time.
pub(crate) fn parse_clock(text: &str) -> Option<u32> {
    let (hour, minute) = text.split_once(':')?;
    let digits = |part: &str| part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_digit());
    if !digits(hour) || !digits(minute) {
        return None;
    }
    let (hour, minute) = (hour.parse::<u32>().ok()?, minute.parse::<u32>().ok()?);
    (hour < 24 && minute < 60).then_some(hour * 60 + minute)
}

/// Parsed active hours.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveWindow {
    start: u32,
    end: u32,
    /// Bit `n` is day `n` (0 is Sunday).
    days: u8,
    time_zone: Tz,
}

impl ActiveWindow {
    pub(crate) fn parse(hours: &ActiveHours) -> Result<Self, String> {
        let (Some(start), Some(end)) = (parse_clock(&hours.start), parse_clock(&hours.end)) else {
            return Err(ACTIVE_HOURS_TIME_INVALID.into());
        };
        if start == end {
            return Err(ACTIVE_HOURS_SAME_TIME.into());
        }
        if hours.days.is_empty() || hours.days.len() > 7 {
            return Err(ACTIVE_HOURS_DAYS_INVALID.into());
        }
        let mut days = 0u8;
        for day in &hours.days {
            if *day > 6 || days & (1 << day) != 0 {
                return Err(ACTIVE_HOURS_DAYS_INVALID.into());
            }
            days |= 1 << day;
        }
        Ok(Self {
            start,
            end,
            days,
            time_zone: parse_time_zone(&hours.time_zone)?,
        })
    }

    fn local(&self, at_ms: u64) -> Option<NaiveDateTime> {
        let at = Utc
            .timestamp_millis_opt(i64::try_from(at_ms).ok()?)
            .single()?;
        Some(at.with_timezone(&self.time_zone).naive_local())
    }

    fn has_day(&self, day: u32) -> bool {
        self.days & (1 << day) != 0
    }

    /// Whether `at_ms` is inside the window. An overnight window
    /// (`start > end`) belongs to the day it starts.
    pub(crate) fn contains(&self, at_ms: u64) -> bool {
        let Some(local) = self.local(at_ms) else {
            return false;
        };
        let minute = local.hour() * 60 + local.minute();
        let day = local.weekday().num_days_from_sunday();
        if self.start < self.end {
            self.has_day(day) && (self.start..self.end).contains(&minute)
        } else {
            let yesterday = (day + 6) % 7;
            (self.has_day(day) && minute >= self.start)
                || (self.has_day(yesterday) && minute < self.end)
        }
    }

    /// `at_ms` when it is inside the window, else the window's next opening.
    pub(crate) fn next_open(&self, at_ms: u64) -> Option<u64> {
        if self.contains(at_ms) {
            return Some(at_ms);
        }
        let local = self.local(at_ms)?;
        let mut date = local.date();
        for _ in 0..=8 {
            if self.has_day(date.weekday().num_days_from_sunday()) {
                let wall = date.and_hms_opt(self.start / 60, self.start % 60, 0)?;
                if let Some(open) = self.opening(wall, at_ms) {
                    if open >= at_ms {
                        return Some(open);
                    }
                }
            }
            date = date.succ_opt()?;
        }
        None
    }

    /// The first instant at or after `at_ms` that the window opens at wall
    /// time `wall`: a repeated (fall-back) wall time counts both of its
    /// instants; inside a daylight-saving gap, the first wall minute after
    /// the gap. `None` when every instant is before `at_ms`.
    fn opening(&self, wall: NaiveDateTime, at_ms: u64) -> Option<u64> {
        (0..=DST_GAP_SEARCH_MINUTES)
            .find_map(|shift| {
                let shifted = wall.checked_add_signed(TimeDelta::minutes(shift))?;
                let instants = match self.time_zone.from_local_datetime(&shifted) {
                    LocalResult::Single(at) => [Some(at), None],
                    LocalResult::Ambiguous(first, second) => {
                        [Some(first.min(second)), Some(first.max(second))]
                    }
                    LocalResult::None => return None,
                };
                Some(
                    instants
                        .into_iter()
                        .flatten()
                        .filter_map(|at| u64::try_from(at.timestamp_millis()).ok())
                        .find(|open| *open >= at_ms),
                )
            })
            .flatten()
    }
}

/// `hours` checked, with its days in order: the stored form.
pub(crate) fn normalized_active_hours(hours: ActiveHours) -> Result<ActiveHours, String> {
    ActiveWindow::parse(&hours)?;
    let mut days = hours.days;
    days.sort_unstable();
    Ok(ActiveHours {
        start: hours.start,
        end: hours.end,
        days,
        time_zone: hours.time_zone.trim().to_string(),
    })
}

/// Every trigger variant by name, with no catch-all arm (spec §9.1): used on
/// creation, update, and restore.
pub(crate) fn validate_stored_trigger(trigger: &ScheduleTrigger) -> Result<(), String> {
    match trigger {
        ScheduleTrigger::Interval { interval_ms } => {
            if *interval_ms == 0 || interval_ms % 1_000 != 0 {
                return Err(INTERVAL_INVALID.into());
            }
        }
        ScheduleTrigger::Daily {
            hour,
            minute,
            time_zone,
        } => {
            if *hour > 23 || *minute > 59 {
                return Err(DAILY_INVALID.into());
            }
            parse_time_zone(time_zone)?;
        }
        ScheduleTrigger::Cron {
            expression,
            time_zone,
        } => {
            parse_cron(expression)?;
            parse_time_zone(time_zone)?;
        }
        ScheduleTrigger::Once { at_ms } => {
            if *at_ms == 0 {
                return Err(ONCE_INVALID.into());
            }
        }
    }
    Ok(())
}

/// `trigger` as stored: a time zone is kept trimmed, as validation read it.
pub(crate) fn normalized_trigger(trigger: ScheduleTrigger) -> ScheduleTrigger {
    match trigger {
        ScheduleTrigger::Daily {
            hour,
            minute,
            time_zone,
        } => ScheduleTrigger::Daily {
            hour,
            minute,
            time_zone: time_zone.trim().to_string(),
        },
        ScheduleTrigger::Cron {
            expression,
            time_zone,
        } => ScheduleTrigger::Cron {
            expression,
            time_zone: time_zone.trim().to_string(),
        },
        other @ (ScheduleTrigger::Interval { .. } | ScheduleTrigger::Once { .. }) => other,
    }
}

/// The next `hour:minute` in `time_zone` strictly after `after_ms`, by the
/// `daily` trigger's existing rule: a wall time a daylight-saving jump skips
/// moves to the next day, and a repeated one fires at its earlier instant.
fn next_daily_after(hour: u8, minute: u8, time_zone: &str, after_ms: u64) -> Result<u64, String> {
    let zone = parse_time_zone(time_zone)?;
    let after = Utc
        .timestamp_millis_opt(i64::try_from(after_ms).map_err(|_| TIMING_OVERFLOW.to_string())?)
        .single()
        .ok_or_else(|| TIMING_OVERFLOW.to_string())?;
    let local = after.with_timezone(&zone);
    for day_offset in 0..=2 {
        let date = local
            .date_naive()
            .checked_add_days(chrono::Days::new(day_offset))
            .ok_or_else(|| TIMING_OVERFLOW.to_string())?;
        let wall = date
            .and_hms_opt(u32::from(hour), u32::from(minute), 0)
            .ok_or_else(|| DAILY_INVALID.to_string())?;
        let candidate = match zone.from_local_datetime(&wall) {
            LocalResult::Single(at) => at,
            LocalResult::Ambiguous(first, second) => first.min(second),
            LocalResult::None => continue,
        };
        if let Ok(at_ms) = u64::try_from(candidate.timestamp_millis()) {
            if at_ms > after_ms {
                return Ok(at_ms);
            }
        }
    }
    Err(SCHEDULE_NEVER_RUNS.into())
}

/// The trigger's own next fire strictly after `after_ms`, ignoring windows.
///
/// `floor_ms` (at most `after_ms` in effect) is a cron's wall-clock floor.
fn raw_next(trigger: &ScheduleTrigger, after_ms: u64, floor_ms: u64) -> Result<u64, String> {
    match trigger {
        ScheduleTrigger::Interval { interval_ms } => after_ms
            .checked_add(*interval_ms)
            .ok_or_else(|| TIMING_OVERFLOW.to_string()),
        ScheduleTrigger::Daily {
            hour,
            minute,
            time_zone,
        } => next_daily_after(*hour, *minute, time_zone, after_ms),
        ScheduleTrigger::Cron {
            expression,
            time_zone,
        } => parse_cron(expression)?
            .next_after_floor(parse_time_zone(time_zone)?, after_ms, floor_ms)
            .ok_or_else(|| SCHEDULE_NEVER_RUNS.to_string()),
        ScheduleTrigger::Once { at_ms } => {
            if *at_ms > after_ms {
                Ok(*at_ms)
            } else {
                Err(ONCE_NOT_IN_FUTURE.into())
            }
        }
    }
}

/// The first fire of `trigger` after `from_ms` inside `window`: an interval
/// counts from `from_ms` and waits for the window to open; a cron or daily
/// fire outside the window is skipped for its next one inside.
pub(crate) fn next_fire_after(
    trigger: &ScheduleTrigger,
    window: Option<&ActiveWindow>,
    from_ms: u64,
) -> Result<u64, String> {
    next_fire_after_floor(trigger, window, from_ms, from_ms)
}

/// [`next_fire_after`] with a cron wall-clock floor (see
/// `CronSchedule::next_after_floor`); the fire is still after `from_ms`.
fn next_fire_after_floor(
    trigger: &ScheduleTrigger,
    window: Option<&ActiveWindow>,
    from_ms: u64,
    floor_ms: u64,
) -> Result<u64, String> {
    validate_stored_trigger(trigger)?;
    let Some(window) = window else {
        return raw_next(trigger, from_ms, floor_ms);
    };
    match trigger {
        ScheduleTrigger::Once { .. } => Err(ACTIVE_HOURS_NOT_FOR_ONCE.into()),
        ScheduleTrigger::Interval { .. } => window
            .next_open(raw_next(trigger, from_ms, floor_ms)?)
            .ok_or_else(|| SCHEDULE_NEVER_IN_ACTIVE_HOURS.to_string()),
        ScheduleTrigger::Daily { .. } | ScheduleTrigger::Cron { .. } => {
            let mut after = from_ms;
            for _ in 0..MAX_WINDOW_HOPS {
                let candidate = raw_next(trigger, after, floor_ms)?;
                if window.contains(candidate) {
                    return Ok(candidate);
                }
                let open = window
                    .next_open(candidate)
                    .ok_or_else(|| SCHEDULE_NEVER_IN_ACTIVE_HOURS.to_string())?;
                // The next fire at or after the opening.
                after = open.saturating_sub(1).max(candidate);
            }
            Err(SCHEDULE_NEVER_IN_ACTIVE_HOURS.into())
        }
    }
}

/// The due time once the occurrence due at `previous_due` was claimed at
/// `now_ms`: an interval keeps its cadence (missed steps are skipped, never
/// replayed) and waits for the window; cron and daily take their next fire
/// after `now_ms`; a `once` keeps its time (its claim turns it off).
pub(crate) fn next_fire_after_claim(
    trigger: &ScheduleTrigger,
    window: Option<&ActiveWindow>,
    previous_due: u64,
    now_ms: u64,
) -> Result<u64, String> {
    validate_stored_trigger(trigger)?;
    match trigger {
        ScheduleTrigger::Interval { interval_ms } => {
            let steps = now_ms.saturating_sub(previous_due) / interval_ms + 1;
            let stepped = interval_ms
                .checked_mul(steps)
                .and_then(|span| previous_due.checked_add(span))
                .ok_or_else(|| TIMING_OVERFLOW.to_string())?;
            match window {
                None => Ok(stepped),
                Some(window) => window
                    .next_open(stepped)
                    .ok_or_else(|| SCHEDULE_NEVER_IN_ACTIVE_HOURS.to_string()),
            }
        }
        ScheduleTrigger::Once { .. } => Ok(previous_due),
        ScheduleTrigger::Daily { .. } | ScheduleTrigger::Cron { .. } => {
            // The wall clock only moves forward from the fire just claimed,
            // even when the claim is late and the clock fell back meanwhile,
            // so a repeated hour is not fired twice.
            next_fire_after_floor(trigger, window, now_ms, previous_due)
        }
    }
}

/// The next `count` fires after `from_ms` (one for `once`), as the scheduler
/// would claim them on time: for previews and the agent minimum.
pub(crate) fn upcoming_fires(
    trigger: &ScheduleTrigger,
    window: Option<&ActiveWindow>,
    from_ms: u64,
    count: usize,
) -> Result<Vec<u64>, String> {
    let mut fires = Vec::with_capacity(count);
    if count == 0 {
        return Ok(fires);
    }
    let mut at = next_fire_after(trigger, window, from_ms)?;
    loop {
        fires.push(at);
        if fires.len() == count || matches!(trigger, ScheduleTrigger::Once { .. }) {
            return Ok(fires);
        }
        at = next_fire_after(trigger, window, at)?;
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> u64 {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .unwrap()
            .timestamp_millis() as u64
    }

    fn hours(start: &str, end: &str, days: &[u8], zone: &str) -> ActiveHours {
        ActiveHours {
            start: start.into(),
            end: end.into(),
            days: days.to_vec(),
            time_zone: zone.into(),
        }
    }

    fn window(start: &str, end: &str, days: &[u8], zone: &str) -> ActiveWindow {
        ActiveWindow::parse(&hours(start, end, days, zone)).unwrap()
    }

    const EVERY_DAY: &[u8] = &[0, 1, 2, 3, 4, 5, 6];
    const WEEKDAYS: &[u8] = &[1, 2, 3, 4, 5];
    const HALF_HOUR: ScheduleTrigger = ScheduleTrigger::Interval {
        interval_ms: 30 * 60_000,
    };

    fn cron(expression: &str) -> ScheduleTrigger {
        ScheduleTrigger::Cron {
            expression: expression.into(),
            time_zone: "UTC".into(),
        }
    }

    #[test]
    fn the_bounds_and_strings_are_the_specs() {
        assert_eq!(MAX_WINDOW_HOPS, 400);
        assert_eq!(DST_GAP_SEARCH_MINUTES, 180);
        assert_eq!(TIME_ZONE_INVALID, "timeZone is invalid");
        assert_eq!(
            INTERVAL_INVALID,
            "intervalMs must be a positive whole number of seconds"
        );
        assert_eq!(DAILY_INVALID, "daily trigger is invalid");
        assert_eq!(ONCE_INVALID, "atMs must be a positive time in milliseconds");
        assert_eq!(
            ACTIVE_HOURS_TIME_INVALID,
            "activeHours start and end must be HH:MM in 24-hour time"
        );
        assert_eq!(
            ACTIVE_HOURS_SAME_TIME,
            "activeHours start and end must differ"
        );
        assert_eq!(
            ACTIVE_HOURS_DAYS_INVALID,
            "activeHours days must list 1 to 7 different days from 0 (Sunday) to 6 (Saturday)"
        );
        assert_eq!(
            ACTIVE_HOURS_NOT_FOR_ONCE,
            "activeHours does not apply to a one-time automation"
        );
        assert_eq!(ONCE_NOT_IN_FUTURE, "atMs must be in the future");
        assert_eq!(SCHEDULE_NEVER_RUNS, "This schedule never runs");
        assert_eq!(
            SCHEDULE_NEVER_IN_ACTIVE_HOURS,
            "This schedule never runs inside its active hours"
        );
    }

    #[test]
    fn active_hours_are_validated_and_stored_with_their_days_in_order() {
        assert_eq!(
            normalized_active_hours(hours("08:00", "22:00", &[5, 1, 3], "UTC")),
            Ok(hours("08:00", "22:00", &[1, 3, 5], "UTC"))
        );
        for (bad, problem) in [
            (
                hours("8:00", "22:00", WEEKDAYS, "UTC"),
                ACTIVE_HOURS_TIME_INVALID,
            ),
            (
                hours("24:00", "22:00", WEEKDAYS, "UTC"),
                ACTIVE_HOURS_TIME_INVALID,
            ),
            (
                hours("08:60", "22:00", WEEKDAYS, "UTC"),
                ACTIVE_HOURS_TIME_INVALID,
            ),
            (
                hours("08:00", "2200", WEEKDAYS, "UTC"),
                ACTIVE_HOURS_TIME_INVALID,
            ),
            (
                hours("+8:00", "22:00", WEEKDAYS, "UTC"),
                ACTIVE_HOURS_TIME_INVALID,
            ),
            (
                hours("08:00", "08:00", WEEKDAYS, "UTC"),
                ACTIVE_HOURS_SAME_TIME,
            ),
            (
                hours("08:00", "22:00", &[], "UTC"),
                ACTIVE_HOURS_DAYS_INVALID,
            ),
            (
                hours("08:00", "22:00", &[7], "UTC"),
                ACTIVE_HOURS_DAYS_INVALID,
            ),
            (
                hours("08:00", "22:00", &[1, 1], "UTC"),
                ACTIVE_HOURS_DAYS_INVALID,
            ),
            (
                hours("08:00", "22:00", WEEKDAYS, "Mars/Base"),
                TIME_ZONE_INVALID,
            ),
            (hours("08:00", "22:00", WEEKDAYS, " "), TIME_ZONE_INVALID),
        ] {
            assert_eq!(
                normalized_active_hours(bad.clone()),
                Err(problem.to_string()),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_same_day_window_covers_start_up_to_end() {
        // 2026-01-05 is a Monday; 2026-01-10 a Saturday.
        let open = window("08:00", "22:00", WEEKDAYS, "UTC");
        assert!(open.contains(utc(2026, 1, 5, 8, 0)));
        assert!(open.contains(utc(2026, 1, 5, 21, 59)));
        assert!(!open.contains(utc(2026, 1, 5, 22, 0)));
        assert!(!open.contains(utc(2026, 1, 5, 7, 59)));
        assert!(!open.contains(utc(2026, 1, 10, 10, 0)));
    }

    #[test]
    fn an_overnight_window_belongs_to_its_start_day() {
        // Friday 22:00 to Saturday 06:00 only (2026-01-09 is a Friday).
        let night = window("22:00", "06:00", &[5], "UTC");
        assert!(night.contains(utc(2026, 1, 9, 23, 0)));
        assert!(night.contains(utc(2026, 1, 10, 5, 59)));
        assert!(
            !night.contains(utc(2026, 1, 10, 23, 0)),
            "Saturday night is not listed"
        );
        assert!(
            !night.contains(utc(2026, 1, 9, 5, 0)),
            "Thursday night is not listed"
        );
    }

    #[test]
    fn the_next_opening_is_now_inside_or_the_next_listed_day() {
        let open = window("08:00", "22:00", WEEKDAYS, "UTC");
        let inside = utc(2026, 1, 5, 9, 0);
        assert_eq!(open.next_open(inside), Some(inside));
        assert_eq!(
            open.next_open(utc(2026, 1, 5, 7, 0)),
            Some(utc(2026, 1, 5, 8, 0))
        );
        assert_eq!(
            open.next_open(utc(2026, 1, 9, 22, 30)),
            Some(utc(2026, 1, 12, 8, 0)),
            "Friday night waits for Monday"
        );
    }

    #[test]
    fn a_window_opening_in_a_gap_opens_after_it() {
        // New York has no 02:30 on 2026-03-08: the window opens at 03:00 EDT.
        let early = window("02:30", "05:00", EVERY_DAY, "America/New_York");
        assert_eq!(
            early.next_open(utc(2026, 3, 8, 5, 0)),
            Some(utc(2026, 3, 8, 7, 0))
        );
    }

    #[test]
    fn an_interval_outside_the_window_waits_for_the_next_opening() {
        let day = window("08:00", "22:00", EVERY_DAY, "UTC");
        assert_eq!(
            next_fire_after(&HALF_HOUR, Some(&day), utc(2026, 1, 5, 21, 45)),
            Ok(utc(2026, 1, 6, 8, 0))
        );
        assert_eq!(
            upcoming_fires(&HALF_HOUR, Some(&day), utc(2026, 1, 5, 21, 0), 3),
            Ok(vec![
                utc(2026, 1, 5, 21, 30),
                utc(2026, 1, 6, 8, 0),
                utc(2026, 1, 6, 8, 30),
            ])
        );
        assert_eq!(
            upcoming_fires(&HALF_HOUR, None, utc(2026, 1, 5, 21, 0), 2),
            Ok(vec![utc(2026, 1, 5, 21, 30), utc(2026, 1, 5, 22, 0)])
        );
        assert_eq!(upcoming_fires(&HALF_HOUR, None, 1, 0), Ok(vec![]));
    }

    #[test]
    fn cron_and_daily_skip_fires_outside_the_window() {
        let mornings = window("09:00", "11:00", WEEKDAYS, "UTC");
        assert_eq!(
            upcoming_fires(
                &cron("0 * * * *"),
                Some(&mornings),
                utc(2026, 1, 5, 7, 30),
                3
            ),
            Ok(vec![
                utc(2026, 1, 5, 9, 0),
                utc(2026, 1, 5, 10, 0),
                utc(2026, 1, 6, 9, 0),
            ])
        );
        let early = ScheduleTrigger::Daily {
            hour: 7,
            minute: 0,
            time_zone: "UTC".into(),
        };
        let day = window("08:00", "22:00", EVERY_DAY, "UTC");
        assert_eq!(
            next_fire_after(&early, Some(&day), utc(2026, 1, 5, 0, 0)),
            Err(SCHEDULE_NEVER_IN_ACTIVE_HOURS.to_string())
        );
        assert_eq!(
            next_fire_after(&cron("0 0 31 2 *"), None, utc(2026, 1, 5, 0, 0)),
            Err(SCHEDULE_NEVER_RUNS.to_string())
        );
    }

    #[test]
    fn claims_keep_an_intervals_cadence_inside_the_window() {
        let day = window("08:00", "22:00", EVERY_DAY, "UTC");
        assert_eq!(
            next_fire_after_claim(
                &HALF_HOUR,
                Some(&day),
                utc(2026, 1, 5, 21, 30),
                utc(2026, 1, 5, 21, 31)
            ),
            Ok(utc(2026, 1, 6, 8, 0))
        );
        // Claimed 70 minutes late: the missed steps are skipped, not replayed.
        assert_eq!(
            next_fire_after_claim(
                &HALF_HOUR,
                Some(&day),
                utc(2026, 1, 6, 8, 0),
                utc(2026, 1, 6, 9, 10)
            ),
            Ok(utc(2026, 1, 6, 9, 30))
        );
        assert_eq!(
            next_fire_after_claim(&cron("0 9 * * *"), None, 1, utc(2026, 1, 5, 9, 0)),
            Ok(utc(2026, 1, 6, 9, 0))
        );
    }

    #[test]
    fn once_fires_once_and_refuses_active_hours() {
        let at = utc(2026, 1, 5, 15, 0);
        let once = ScheduleTrigger::Once { at_ms: at };
        assert_eq!(next_fire_after(&once, None, at - 1), Ok(at));
        assert_eq!(
            next_fire_after(&once, None, at),
            Err(ONCE_NOT_IN_FUTURE.to_string())
        );
        let day = window("08:00", "22:00", EVERY_DAY, "UTC");
        assert_eq!(
            next_fire_after(&once, Some(&day), at - 1),
            Err(ACTIVE_HOURS_NOT_FOR_ONCE.to_string())
        );
        assert_eq!(upcoming_fires(&once, None, at - 1, 3), Ok(vec![at]));
        assert_eq!(next_fire_after_claim(&once, None, at, at + 5), Ok(at));
    }

    #[test]
    fn stored_triggers_are_validated_variant_by_variant() {
        for good in [
            ScheduleTrigger::Interval {
                interval_ms: 60_000,
            },
            ScheduleTrigger::Daily {
                hour: 23,
                minute: 59,
                time_zone: "Asia/Kuala_Lumpur".into(),
            },
            cron("@daily"),
            ScheduleTrigger::Once { at_ms: 1 },
        ] {
            assert_eq!(validate_stored_trigger(&good), Ok(()), "{good:?}");
        }
        for (bad, problem) in [
            (
                ScheduleTrigger::Interval { interval_ms: 0 },
                INTERVAL_INVALID.to_string(),
            ),
            (
                ScheduleTrigger::Interval { interval_ms: 1_500 },
                INTERVAL_INVALID.to_string(),
            ),
            (
                ScheduleTrigger::Daily {
                    hour: 24,
                    minute: 0,
                    time_zone: "UTC".into(),
                },
                DAILY_INVALID.to_string(),
            ),
            (
                ScheduleTrigger::Daily {
                    hour: 9,
                    minute: 0,
                    time_zone: "".into(),
                },
                TIME_ZONE_INVALID.to_string(),
            ),
            (
                cron("61 * * * *"),
                "minute: must be from 0 to 59".to_string(),
            ),
            (
                ScheduleTrigger::Cron {
                    expression: "@daily".into(),
                    time_zone: "Nowhere/Town".into(),
                },
                TIME_ZONE_INVALID.to_string(),
            ),
            (ScheduleTrigger::Once { at_ms: 0 }, ONCE_INVALID.to_string()),
        ] {
            assert_eq!(validate_stored_trigger(&bad), Err(problem), "{bad:?}");
        }
    }

    #[test]
    fn a_daily_trigger_fires_as_before() {
        // Kuala Lumpur 09:30 is 01:30 UTC; asked at 01:30 UTC exactly, the
        // next one is tomorrow's.
        let daily = ScheduleTrigger::Daily {
            hour: 9,
            minute: 30,
            time_zone: "Asia/Kuala_Lumpur".into(),
        };
        assert_eq!(
            next_fire_after(&daily, None, utc(2026, 1, 5, 1, 0)),
            Ok(utc(2026, 1, 5, 1, 30))
        );
        assert_eq!(
            next_fire_after(&daily, None, utc(2026, 1, 5, 1, 30)),
            Ok(utc(2026, 1, 6, 1, 30))
        );
    }

    #[test]
    fn a_window_that_never_matches_is_bounded_and_refused() {
        // Fires only at midnight, never inside 08:00-09:00: every hop skips
        // one fire, and after MAX_WINDOW_HOPS the schedule is refused.
        let mornings = window("08:00", "09:00", EVERY_DAY, "UTC");
        assert_eq!(
            next_fire_after(&cron("0 0 * * *"), Some(&mornings), utc(2026, 1, 5, 0, 0)),
            Err(SCHEDULE_NEVER_IN_ACTIVE_HOURS.to_string())
        );
        // Rare fires (every Feb 29 at midnight) make each hop span years; the
        // search still ends, and a never-firing expression is refused outright.
        assert_eq!(
            next_fire_after(&cron("0 0 29 2 *"), Some(&mornings), utc(2026, 1, 5, 0, 0)),
            Err(SCHEDULE_NEVER_IN_ACTIVE_HOURS.to_string())
        );
        assert_eq!(
            next_fire_after(&cron("0 0 31 2 *"), Some(&mornings), utc(2026, 1, 5, 0, 0)),
            Err(SCHEDULE_NEVER_RUNS.to_string())
        );
        // A weekly Monday fire with a Friday-only window never matches either.
        let fridays = window("00:00", "23:59", &[5], "UTC");
        assert_eq!(
            next_fire_after(&cron("0 9 * * 1"), Some(&fridays), utc(2026, 1, 5, 0, 0)),
            Err(SCHEDULE_NEVER_IN_ACTIVE_HOURS.to_string())
        );
    }

    #[test]
    fn chained_fires_pass_the_previous_fire_and_do_not_repeat_a_repeated_hour() {
        // New York falls back on 2026-11-01: 01:00-02:00 happens twice (05:00
        // and 06:00 UTC). A daily-at-01:30 cron fires once, at the first 01:30.
        let zone = ScheduleTrigger::Cron {
            expression: "30 1 * * *".into(),
            time_zone: "America/New_York".into(),
        };
        let fires = upcoming_fires(&zone, None, utc(2026, 10, 31, 12, 0), 3).unwrap();
        assert_eq!(
            fires,
            vec![
                utc(2026, 11, 1, 5, 30),
                utc(2026, 11, 2, 6, 30),
                utc(2026, 11, 3, 6, 30)
            ]
        );
        // A claim of that first 01:30 asks for the next one from the fire.
        assert_eq!(
            next_fire_after_claim(&zone, None, fires[0], fires[0] + 200),
            Ok(fires[1])
        );
    }

    #[test]
    fn a_late_claim_in_the_repeated_hour_does_not_fire_it_twice() {
        let trigger = ScheduleTrigger::Cron {
            expression: "30 1 * * *".into(),
            time_zone: "America/New_York".into(),
        };
        // 01:30 EDT fired; the claim lands at 01:20 EST, after the fall-back.
        let (due, claimed_at) = (utc(2026, 11, 1, 5, 30), utc(2026, 11, 1, 6, 20));
        assert_eq!(
            next_fire_after_claim(&trigger, None, due, claimed_at).unwrap(),
            utc(2026, 11, 2, 6, 30),
            "01:30 EST today is the same wall time as the fire just claimed"
        );
        assert_eq!(
            next_fire_after(&trigger, None, claimed_at).unwrap(),
            utc(2026, 11, 1, 6, 30),
            "without the floor the repeated 01:30 is still ahead"
        );
    }

    #[test]
    fn a_window_opening_in_the_repeated_hour_is_found_at_its_second_instant() {
        let window = window("01:30", "06:00", EVERY_DAY, "America/New_York");
        // 01:10 EST, after the first 01:30 (EDT) has passed.
        assert_eq!(
            window.next_open(utc(2026, 11, 1, 6, 10)),
            Some(utc(2026, 11, 1, 6, 30))
        );
        // Before the first 01:30, it opens at that first instant.
        assert_eq!(
            window.next_open(utc(2026, 11, 1, 5, 10)),
            Some(utc(2026, 11, 1, 5, 30))
        );
    }

    #[test]
    fn trigger_time_zones_are_normalized_by_trimming() {
        let daily = ScheduleTrigger::Daily {
            hour: 1,
            minute: 2,
            time_zone: " UTC
"
            .into(),
        };
        assert_eq!(
            normalized_trigger(daily),
            ScheduleTrigger::Daily {
                hour: 1,
                minute: 2,
                time_zone: "UTC".into()
            }
        );
        assert_eq!(normalized_trigger(HALF_HOUR), HALF_HOUR);
    }
}
