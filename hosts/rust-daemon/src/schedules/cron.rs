//! Five-field cron expressions (spec §9.1), evaluated on an IANA time
//! zone's wall clock. In-house, because the daemon takes no cron crate (M6
//! Global Constraints). Day of month and day of week combine the Vixie way:
//! when both are restricted, either may match.

use chrono::{Datelike, LocalResult, NaiveDate, NaiveDateTime, TimeDelta, TimeZone, Timelike, Utc};
use chrono_tz::Tz;

/// The longest expression accepted, in characters.
pub(crate) const MAX_CRON_EXPRESSION_CHARS: usize = 200;
/// Days searched for the next fire: 28 years cover every pairing of a day
/// of the month with a day of the week; anything rarer never fires. This
/// bounds the search's work (M6 ruling 5): at most this many calendar days
/// are checked, and a matching day tries at most 24 x 60 wall times.
pub(crate) const CRON_SEARCH_DAYS: u32 = 28 * 366;

pub(crate) const CRON_EMPTY: &str = "cron expression is empty";
pub(crate) const CRON_TOO_LONG: &str = "cron expression must be at most 200 characters";
pub(crate) const CRON_FIELD_COUNT: &str =
    "cron expression must have 5 fields: minute hour day-of-month month day-of-week";
pub(crate) const CRON_MACRO_UNKNOWN: &str =
    "only @hourly, @daily, @midnight, @weekly, @monthly, @yearly, and @annually are supported";
pub(crate) const CRON_SPECIAL_UNSUPPORTED: &str = "L, W, #, and ? are not supported";
pub(crate) const CRON_EMPTY_ITEM: &str = "has an empty list item";
pub(crate) const CRON_STEP_INVALID: &str = "a step must be a whole number from 1";
pub(crate) const CRON_RANGE_BACKWARDS: &str = "a range must run from low to high";
pub(crate) const CRON_NOT_A_VALUE: &str = "must be a number or a three-letter name";

pub(crate) fn cron_out_of_range(min: u32, max: u32) -> String {
    format!("must be from {min} to {max}")
}

pub(crate) fn cron_field_error(field: &str, detail: &str) -> String {
    format!("{field}: {detail}")
}

#[derive(Clone, Copy)]
struct Field {
    name: &'static str,
    min: u32,
    max: u32,
    /// Three-letter names, the first worth `name_base`.
    names: &'static [&'static str],
    name_base: u32,
}

const MINUTE: Field = Field {
    name: "minute",
    min: 0,
    max: 59,
    names: &[],
    name_base: 0,
};
const HOUR: Field = Field {
    name: "hour",
    min: 0,
    max: 23,
    names: &[],
    name_base: 0,
};
const DAY_OF_MONTH: Field = Field {
    name: "day of month",
    min: 1,
    max: 31,
    names: &[],
    name_base: 0,
};
const MONTH: Field = Field {
    name: "month",
    min: 1,
    max: 12,
    names: &[
        "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
    ],
    name_base: 1,
};
const DAY_OF_WEEK: Field = Field {
    name: "day of week",
    min: 0,
    max: 7,
    names: &["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"],
    name_base: 0,
};

/// ASCII digits only: `str::parse` would also take a leading `+`.
fn is_whole_number(token: &str) -> bool {
    !token.is_empty() && token.bytes().all(|byte| byte.is_ascii_digit())
}

impl Field {
    fn error(&self, detail: &str) -> String {
        cron_field_error(self.name, detail)
    }

    fn value(&self, token: &str) -> Result<u32, String> {
        if token.is_empty() {
            return Err(self.error(CRON_EMPTY_ITEM));
        }
        if is_whole_number(token) {
            return token
                .parse::<u32>()
                .ok()
                .filter(|value| (self.min..=self.max).contains(value))
                .ok_or_else(|| self.error(&cron_out_of_range(self.min, self.max)));
        }
        if let Some(index) = self
            .names
            .iter()
            .position(|name| name.eq_ignore_ascii_case(token))
        {
            return Ok(self.name_base + index as u32);
        }
        let last = token.chars().last().map(|last| last.to_ascii_uppercase());
        if token.contains(['#', '?']) || matches!(last, Some('L' | 'W')) {
            return Err(self.error(CRON_SPECIAL_UNSUPPORTED));
        }
        Err(self.error(CRON_NOT_A_VALUE))
    }

    /// The field's values as bits (bit `n` is value `n`).
    fn parse(&self, text: &str) -> Result<u64, String> {
        let mut bits = 0u64;
        for item in text.split(',') {
            if item.is_empty() {
                return Err(self.error(CRON_EMPTY_ITEM));
            }
            let (range, step) = match item.split_once('/') {
                Some((range, step)) => {
                    let step = Some(step)
                        .filter(|step| is_whole_number(step))
                        .and_then(|step| step.parse::<u32>().ok())
                        .filter(|step| *step >= 1)
                        .ok_or_else(|| self.error(CRON_STEP_INVALID))?;
                    (range, Some(step))
                }
                None => (item, None),
            };
            let (start, end) = if range == "*" {
                (self.min, self.max)
            } else if let Some((low, high)) = range.split_once('-') {
                let (low, high) = (self.value(low)?, self.value(high)?);
                if low > high {
                    return Err(self.error(CRON_RANGE_BACKWARDS));
                }
                (low, high)
            } else {
                let value = self.value(range)?;
                // `a/s` runs from `a` to the field's end.
                (value, if step.is_some() { self.max } else { value })
            };
            let step = step.unwrap_or(1);
            let mut value = start;
            while value <= end {
                bits |= 1u64 << value;
                value = match value.checked_add(step) {
                    Some(next) => next,
                    None => break,
                };
            }
        }
        Ok(bits)
    }
}

/// A parsed expression: the allowed values of each field as bits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CronSchedule {
    minutes: u64,
    hours: u64,
    days_of_month: u64,
    months: u64,
    /// Sunday is bit 0 (a 7 in the expression folds onto it).
    days_of_week: u64,
    dom_restricted: bool,
    dow_restricted: bool,
}

pub(crate) fn parse_cron(expression: &str) -> Result<CronSchedule, String> {
    let trimmed = expression.trim();
    if trimmed.is_empty() {
        return Err(CRON_EMPTY.into());
    }
    if trimmed.chars().count() > MAX_CRON_EXPRESSION_CHARS {
        return Err(CRON_TOO_LONG.into());
    }
    let expanded = match trimmed.strip_prefix('@') {
        Some(name) => match name.to_ascii_lowercase().as_str() {
            "hourly" => "0 * * * *",
            "daily" | "midnight" => "0 0 * * *",
            "weekly" => "0 0 * * 0",
            "monthly" => "0 0 1 * *",
            "yearly" | "annually" => "0 0 1 1 *",
            _ => return Err(CRON_MACRO_UNKNOWN.into()),
        },
        None => trimmed,
    };
    let fields = expanded.split_whitespace().collect::<Vec<_>>();
    let &[minute, hour, day_of_month, month, day_of_week] = fields.as_slice() else {
        return Err(CRON_FIELD_COUNT.into());
    };
    let minutes = MINUTE.parse(minute)?;
    let hours = HOUR.parse(hour)?;
    let days_of_month = DAY_OF_MONTH.parse(day_of_month)?;
    let months = MONTH.parse(month)?;
    let mut days_of_week = DAY_OF_WEEK.parse(day_of_week)?;
    if days_of_week & (1 << 7) != 0 {
        days_of_week = (days_of_week | 1) & !(1 << 7);
    }
    Ok(CronSchedule {
        minutes,
        hours,
        days_of_month,
        months,
        days_of_week,
        dom_restricted: !day_of_month.starts_with('*'),
        dow_restricted: !day_of_week.starts_with('*'),
    })
}

impl CronSchedule {
    fn day_matches(&self, date: NaiveDate) -> bool {
        if self.months & (1u64 << date.month()) == 0 {
            return false;
        }
        let dom = self.days_of_month & (1u64 << date.day()) != 0;
        let dow = self.days_of_week & (1u64 << date.weekday().num_days_from_sunday()) != 0;
        if self.dom_restricted && self.dow_restricted {
            dom || dow
        } else {
            dom && dow
        }
    }

    /// The first fire strictly after `after_ms` on `time_zone`'s wall clock:
    /// wall-clock minutes after the current one are tried in order, each
    /// resolved by [`resolve_wall_time`]. The wall clock only moves forward,
    /// so after a fire inside a repeated (fall-back) hour that hour's times
    /// do not fire again. `None` when none falls within `CRON_SEARCH_DAYS`
    /// days.
    #[cfg(test)]
    pub(crate) fn next_after(&self, time_zone: Tz, after_ms: u64) -> Option<u64> {
        self.next_after_floor(time_zone, after_ms, after_ms)
    }

    /// Like [`Self::next_after`], but the wall clock never starts before
    /// `floor_ms`'s: a fire claimed late (inside a repeated hour, after the
    /// clock fell back past the fire's own wall time) does not fire again.
    /// The instant must still come after `after_ms`.
    pub(crate) fn next_after_floor(
        &self,
        time_zone: Tz,
        after_ms: u64,
        floor_ms: u64,
    ) -> Option<u64> {
        let wall_of = |at_ms: u64| {
            let at = Utc
                .timestamp_millis_opt(i64::try_from(at_ms).ok()?)
                .single()?;
            Some(at.with_timezone(&time_zone).naive_local())
        };
        let local = wall_of(after_ms)?.max(wall_of(floor_ms)?);
        let first = local
            .date()
            .and_hms_opt(local.hour(), local.minute(), 0)?
            .checked_add_signed(TimeDelta::minutes(1))?;
        let mut date = first.date();
        for day in 0..CRON_SEARCH_DAYS {
            if self.day_matches(date) {
                let earliest = if day == 0 {
                    first.hour() * 60 + first.minute()
                } else {
                    0
                };
                for hour in (0..24u32).filter(|hour| self.hours & (1u64 << hour) != 0) {
                    for minute in (0..60u32).filter(|minute| self.minutes & (1u64 << minute) != 0) {
                        if hour * 60 + minute < earliest {
                            continue;
                        }
                        let wall = date.and_hms_opt(hour, minute, 0)?;
                        if let Some(at_ms) = resolve_wall_time(time_zone, wall, after_ms) {
                            return Some(at_ms);
                        }
                    }
                }
            }
            date = date.succ_opt()?;
        }
        None
    }
}

/// The instant of wall-clock `wall` in `time_zone` that comes after
/// `after_ms`. A time a daylight-saving jump skips does not exist (`None`);
/// a repeated time gives its first occurrence after `after_ms`.
pub(crate) fn resolve_wall_time(time_zone: Tz, wall: NaiveDateTime, after_ms: u64) -> Option<u64> {
    let instants = match time_zone.from_local_datetime(&wall) {
        LocalResult::Single(at) => [Some(at), None],
        LocalResult::Ambiguous(first, second) => [Some(first.min(second)), Some(first.max(second))],
        LocalResult::None => [None, None],
    };
    instants
        .into_iter()
        .flatten()
        .filter_map(|at| u64::try_from(at.timestamp_millis()).ok())
        .find(|at_ms| *at_ms > after_ms)
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

    fn next(expression: &str, zone: &str, after_ms: u64) -> Option<u64> {
        parse_cron(expression)
            .unwrap()
            .next_after(zone.parse().unwrap(), after_ms)
    }

    fn bits(values: &[u32]) -> u64 {
        values.iter().fold(0, |bits, value| bits | 1u64 << value)
    }

    const DAY_MS: u64 = 24 * 60 * 60 * 1000;

    #[test]
    fn the_limits_and_strings_are_the_specs() {
        assert_eq!(MAX_CRON_EXPRESSION_CHARS, 200);
        assert_eq!(CRON_SEARCH_DAYS, 28 * 366);
        assert_eq!(CRON_EMPTY, "cron expression is empty");
        assert_eq!(
            CRON_TOO_LONG,
            "cron expression must be at most 200 characters"
        );
        assert_eq!(
            CRON_FIELD_COUNT,
            "cron expression must have 5 fields: minute hour day-of-month month day-of-week"
        );
        assert_eq!(
            CRON_MACRO_UNKNOWN,
            "only @hourly, @daily, @midnight, @weekly, @monthly, @yearly, and @annually are supported"
        );
        assert_eq!(CRON_SPECIAL_UNSUPPORTED, "L, W, #, and ? are not supported");
        assert_eq!(CRON_EMPTY_ITEM, "has an empty list item");
        assert_eq!(CRON_STEP_INVALID, "a step must be a whole number from 1");
        assert_eq!(CRON_RANGE_BACKWARDS, "a range must run from low to high");
        assert_eq!(CRON_NOT_A_VALUE, "must be a number or a three-letter name");
        assert_eq!(cron_out_of_range(0, 59), "must be from 0 to 59");
        assert_eq!(cron_field_error("hour", "x"), "hour: x");
    }

    #[test]
    fn fields_accept_numbers_names_ranges_steps_and_lists() {
        let schedule = parse_cron("*/15 9-17 1,15 JAN,jul MON-FRI").unwrap();
        assert_eq!(schedule.minutes, bits(&[0, 15, 30, 45]));
        assert_eq!(schedule.hours, bits(&[9, 10, 11, 12, 13, 14, 15, 16, 17]));
        assert_eq!(schedule.days_of_month, bits(&[1, 15]));
        assert_eq!(schedule.months, bits(&[1, 7]));
        assert_eq!(schedule.days_of_week, bits(&[1, 2, 3, 4, 5]));
        assert!(schedule.dom_restricted && schedule.dow_restricted);

        let every = parse_cron("* * * * *").unwrap();
        assert_eq!(every.minutes, (1u64 << 60) - 1);
        assert_eq!(every.days_of_week, bits(&[0, 1, 2, 3, 4, 5, 6]));
        assert!(!every.dom_restricted && !every.dow_restricted);
        assert_eq!(
            parse_cron("  0   9 * * 1  ").unwrap(),
            parse_cron("0 9 * * mon").unwrap()
        );
    }

    #[test]
    fn seven_is_sunday_and_a_slash_runs_to_the_field_end() {
        assert_eq!(parse_cron("0 0 * * 7").unwrap().days_of_week, bits(&[0]));
        assert_eq!(
            parse_cron("0 0 * * 5-7").unwrap().days_of_week,
            bits(&[0, 5, 6])
        );
        assert_eq!(
            parse_cron("5/20 * * * *").unwrap().minutes,
            bits(&[5, 25, 45])
        );
        assert_eq!(
            parse_cron("0-10/5 * * * *").unwrap().minutes,
            bits(&[0, 5, 10])
        );
        // A huge step is one value, never an overflow.
        assert_eq!(
            parse_cron("*/4294967295 * * * *").unwrap().minutes,
            bits(&[0])
        );
    }

    #[test]
    fn macros_expand_and_reboot_is_refused() {
        for (shorthand, fields) in [
            ("@hourly", "0 * * * *"),
            ("@daily", "0 0 * * *"),
            ("@midnight", "0 0 * * *"),
            ("@Weekly", "0 0 * * 0"),
            ("@monthly", "0 0 1 * *"),
            ("@yearly", "0 0 1 1 *"),
            ("@ANNUALLY", "0 0 1 1 *"),
        ] {
            assert_eq!(parse_cron(shorthand), parse_cron(fields), "{shorthand}");
        }
        assert_eq!(parse_cron("@reboot"), Err(CRON_MACRO_UNKNOWN.to_string()));
    }

    #[test]
    fn bad_expressions_name_their_problem() {
        let too_long = format!("0 0 * * *{}x", " ".repeat(200));
        for (expression, problem) in [
            ("", CRON_EMPTY.to_string()),
            ("   ", CRON_EMPTY.to_string()),
            (too_long.as_str(), CRON_TOO_LONG.to_string()),
            ("* * * *", CRON_FIELD_COUNT.to_string()),
            ("* * * * * *", CRON_FIELD_COUNT.to_string()),
            ("60 * * * *", "minute: must be from 0 to 59".to_string()),
            ("* 24 * * *", "hour: must be from 0 to 23".to_string()),
            (
                "* * 0 * *",
                "day of month: must be from 1 to 31".to_string(),
            ),
            ("* * * 13 *", "month: must be from 1 to 12".to_string()),
            ("* * * * 8", "day of week: must be from 0 to 7".to_string()),
            (
                "99999999999 * * * *",
                "minute: must be from 0 to 59".to_string(),
            ),
            ("*/0 * * * *", cron_field_error("minute", CRON_STEP_INVALID)),
            ("*/x * * * *", cron_field_error("minute", CRON_STEP_INVALID)),
            (
                "*/+5 * * * *",
                cron_field_error("minute", CRON_STEP_INVALID),
            ),
            ("*/ * * * *", cron_field_error("minute", CRON_STEP_INVALID)),
            ("+5 * * * *", cron_field_error("minute", CRON_NOT_A_VALUE)),
            (
                "5-1 * * * *",
                cron_field_error("minute", CRON_RANGE_BACKWARDS),
            ),
            ("1,,2 * * * *", cron_field_error("minute", CRON_EMPTY_ITEM)),
            ("-5 * * * *", cron_field_error("minute", CRON_EMPTY_ITEM)),
            (
                "* * L * *",
                cron_field_error("day of month", CRON_SPECIAL_UNSUPPORTED),
            ),
            (
                "* * 15W * *",
                cron_field_error("day of month", CRON_SPECIAL_UNSUPPORTED),
            ),
            (
                "* * ? * *",
                cron_field_error("day of month", CRON_SPECIAL_UNSUPPORTED),
            ),
            (
                "* * * * 1#2",
                cron_field_error("day of week", CRON_SPECIAL_UNSUPPORTED),
            ),
            (
                "* * * * 5L",
                cron_field_error("day of week", CRON_SPECIAL_UNSUPPORTED),
            ),
            ("* * * FOO *", cron_field_error("month", CRON_NOT_A_VALUE)),
            (
                "* * * * SUNDAY",
                cron_field_error("day of week", CRON_NOT_A_VALUE),
            ),
        ] {
            assert_eq!(parse_cron(expression), Err(problem), "{expression:?}");
        }
    }

    #[test]
    fn the_next_fire_is_strictly_after_and_on_the_wall_clock() {
        let nine_thirty = utc(2026, 1, 5, 9, 30);
        assert_eq!(
            next("30 9 * * *", "UTC", nine_thirty - 1),
            Some(nine_thirty)
        );
        assert_eq!(
            next("30 9 * * *", "UTC", nine_thirty),
            Some(utc(2026, 1, 6, 9, 30)),
            "never the minute it was asked from"
        );
        assert_eq!(
            next("30 9 * * *", "UTC", nine_thirty + 30_000),
            Some(utc(2026, 1, 6, 9, 30))
        );
        // Kuala Lumpur is UTC+8 all year: 09:00 there is 01:00 UTC.
        assert_eq!(
            next("0 9 * * *", "Asia/Kuala_Lumpur", utc(2026, 1, 5, 0, 0)),
            Some(utc(2026, 1, 5, 1, 0))
        );
        assert_eq!(
            next("*/15 * * * *", "UTC", utc(2026, 1, 5, 23, 59)),
            Some(utc(2026, 1, 6, 0, 0)),
            "the search rolls over midnight"
        );
    }

    #[test]
    fn a_time_zone_shifts_the_day() {
        // Kiritimati is UTC+14: Sunday 14:00 there when it is 00:00 UTC.
        assert_eq!(
            next("0 0 * * 1", "Pacific/Kiritimati", utc(2026, 1, 4, 0, 0)),
            Some(utc(2026, 1, 4, 10, 0))
        );
    }

    #[test]
    fn dom_and_dow_follow_vixie() {
        // 2026-01-05 is a Monday. Both restricted: the 13th or any Friday.
        assert_eq!(
            next("0 0 13 * 5", "UTC", utc(2026, 1, 5, 0, 0)),
            Some(utc(2026, 1, 9, 0, 0))
        );
        assert_eq!(
            next("0 0 13 * *", "UTC", utc(2026, 1, 5, 0, 0)),
            Some(utc(2026, 1, 13, 0, 0))
        );
        // A day-of-month field starting with `*` is unrestricted: both must
        // match, so an odd-numbered Monday (19 January, not 7 January).
        assert_eq!(
            next("0 0 */2 * 1", "UTC", utc(2026, 1, 5, 0, 0)),
            Some(utc(2026, 1, 19, 0, 0))
        );
    }

    #[test]
    fn a_skipped_wall_time_does_not_fire_and_a_repeated_one_fires_once() {
        // New York springs forward at 02:00 on 2026-03-08: 02:30 does not
        // exist that day, so the next 02:30 is on the 9th (EDT, UTC-4).
        assert_eq!(
            next("30 2 * * *", "America/New_York", utc(2026, 3, 7, 12, 0)),
            Some(utc(2026, 3, 9, 6, 30))
        );
        // It falls back at 02:00 on 2026-11-01: 01:30 happens twice.
        let first = utc(2026, 11, 1, 5, 30); // 01:30 EDT
        assert_eq!(
            next("30 1 * * *", "America/New_York", utc(2026, 11, 1, 4, 0)),
            Some(first)
        );
        assert_eq!(
            next("30 1 * * *", "America/New_York", first),
            Some(utc(2026, 11, 2, 6, 30)),
            "the repeated 01:30 does not fire again"
        );
        // Asked from inside the repeated hour, the next 01:30 is its second
        // occurrence (EST, UTC-5), the first one after the question.
        assert_eq!(
            next("30 1 * * *", "America/New_York", utc(2026, 11, 1, 6, 10)),
            Some(utc(2026, 11, 1, 6, 30))
        );
    }

    #[test]
    fn a_frequent_schedule_steps_over_the_gap_and_the_repeated_hour() {
        // Spring forward: 01:45 EST, then 03:00 EDT (02:xx does not exist).
        let zone = "America/New_York";
        let quarter_to_two = utc(2026, 3, 8, 6, 45);
        assert_eq!(
            next("*/15 * * * *", zone, quarter_to_two),
            Some(utc(2026, 3, 8, 7, 0))
        );
        // Fall back: the walk from 00:45 EDT fires 01:00..01:45 EDT once,
        // then 02:00 EST; the repeated 01:xx EST never fires.
        let mut fires = Vec::new();
        let mut at = utc(2026, 11, 1, 4, 45);
        for _ in 0..6 {
            at = next("*/15 * * * *", zone, at).unwrap();
            fires.push(at);
        }
        assert_eq!(
            fires,
            vec![
                utc(2026, 11, 1, 5, 0),
                utc(2026, 11, 1, 5, 15),
                utc(2026, 11, 1, 5, 30),
                utc(2026, 11, 1, 5, 45),
                utc(2026, 11, 1, 7, 0),
                utc(2026, 11, 1, 7, 15),
            ]
        );
    }

    #[test]
    fn february_31st_never_runs_and_february_29th_waits_for_a_leap_year() {
        assert_eq!(next("0 0 31 2 *", "UTC", utc(2026, 1, 5, 0, 0)), None);
        assert_eq!(
            next("0 0 29 2 *", "UTC", utc(2026, 3, 1, 0, 0)),
            Some(utc(2028, 2, 29, 0, 0))
        );
    }

    #[test]
    fn the_search_stops_after_cron_search_days() {
        // `*/7` starts with `*`, so day of week is unrestricted and both
        // fields must match: 29 February on a Sunday. 2100 is not a leap
        // year, so after 2088 the next one is 2128, 14,609 days later.
        let expression = "0 0 29 2 */7";
        let target = utc(2128, 2, 29, 0, 0);
        let last_day_searched = target - u64::from(CRON_SEARCH_DAYS - 1) * DAY_MS;
        assert_eq!(next(expression, "UTC", last_day_searched), Some(target));
        let one_day_short = target - u64::from(CRON_SEARCH_DAYS) * DAY_MS;
        assert_eq!(next(expression, "UTC", one_day_short), None);
    }

    #[test]
    fn pathological_expressions_end_without_a_fire() {
        // Every minute of a day that never comes, and the latest minute of
        // it: each search walks the whole bound and gives up.
        for expression in ["* * 31 2 *", "59 23 30 2 *", "* * 31 4,6,9,11 */7"] {
            for zone in ["UTC", "America/New_York", "Pacific/Apia"] {
                assert_eq!(
                    next(expression, zone, utc(2026, 1, 5, 0, 0)),
                    None,
                    "{expression} in {zone}"
                );
            }
        }
        // The latest instant chrono can show still answers.
        let max_ms = chrono::DateTime::<Utc>::MAX_UTC.timestamp_millis() as u64;
        assert_eq!(next("* * * * *", "UTC", max_ms), None);
    }

    #[test]
    fn an_instant_outside_chronos_range_has_no_next_fire() {
        assert_eq!(next("* * * * *", "UTC", u64::MAX), None);
    }
}
