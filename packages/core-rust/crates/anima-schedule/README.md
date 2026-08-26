# anima-schedule

`anima-schedule` is the scheduling engine for proactive personal-assistant agents. A `Scheduler` tracks a set of `ScheduledJob`s and decides, for any timestamp passed in by the host, which jobs are due to run. The crate is host-agnostic and fully deterministic: no async runtime, no wall-clock access, no network — a daemon host drives it from its own tick loop and executes the returned jobs with `anima-core` agents.

---

## Triggers

| Trigger | How it works | When to use |
|---|---|---|
| `Every { interval_secs }` | Fires every `interval_secs` seconds, measured from the last fire time. Never-fired jobs are due immediately on the first tick. | Recurring check-ins, polling, periodic summaries. |
| `DailyAt { hour, minute, tz_offset_minutes }` | Fires once per local calendar day after the wall-clock time `hour:minute` in the given fixed timezone offset (minutes from UTC, ±840 max). If a tick first observes the job after the target time, it fires late rather than skipping the day. | Morning briefings, end-of-day digests, anything anchored to the user's local clock. |

---

## Quick usage

```rust
use anima_schedule::{load_jobs_file, ScheduleTrigger, ScheduledJob, Scheduler};

let jobs = vec![
    ScheduledJob {
        name: "check-in".into(),
        agent_name: "assistant".into(),
        prompt: "Check in with the user.".into(),
        trigger: ScheduleTrigger::Every { interval_secs: 300 },
        enabled: true,
    },
    ScheduledJob {
        name: "morning-briefing".into(),
        agent_name: "assistant".into(),
        prompt: "Summarize today's calendar.".into(),
        trigger: ScheduleTrigger::DailyAt {
            hour: 9,
            minute: 0,
            tz_offset_minutes: -300, // UTC-5
        },
        enabled: true,
    },
];

let mut scheduler = Scheduler::new(jobs)?;

// In the host's tick loop:
for job in scheduler.due_jobs(now_ms) {
    // dispatch `job.prompt` to the `job.agent_name` agent
}

// Sleep hint for the host loop.
if let Some(wait_ms) = scheduler.next_due_in_ms(now_ms) {
    // sleep up to `wait_ms` before ticking again
}
```

`Scheduler::new` validates jobs up front: non-empty `name`/`agent_name`/`prompt`, `interval_secs > 0`, `hour < 24`, `minute < 60`, `|tz_offset_minutes| <= 14 * 60`, and no duplicate job names. Disabled jobs (`enabled: false`) are kept but never fire.

---

## JSON file format

`load_jobs_file(path)` accepts either a bare array of jobs or an object with a `jobs` key. Field names are camelCase; the trigger is tagged by `type`. `enabled` defaults to `true` when omitted.

```json
{
  "jobs": [
    {
      "name": "check-in",
      "agentName": "assistant",
      "prompt": "Check in with the user.",
      "trigger": { "type": "every", "intervalSecs": 300 }
    },
    {
      "name": "morning-briefing",
      "agentName": "assistant",
      "prompt": "Summarize today's calendar.",
      "trigger": { "type": "dailyAt", "hour": 9, "minute": 0, "tzOffsetMinutes": -300 },
      "enabled": false
    }
  ]
}
```

A missing or unreadable file is an `std::io::Error`; malformed JSON or a wrong shape is an `InvalidData` error that includes the file path and parse context.
