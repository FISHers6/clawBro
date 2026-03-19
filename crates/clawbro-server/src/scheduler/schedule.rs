use super::models::{ScheduleInput, ScheduleSpec};
use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use std::str::FromStr;

pub fn default_timezone() -> String {
    "UTC".to_string()
}

pub fn parse_timezone(name: &str) -> Result<Tz> {
    name.parse::<Tz>()
        .with_context(|| format!("invalid timezone '{name}'"))
}

pub fn normalize_schedule_input(input: ScheduleInput, now: DateTime<Utc>) -> Result<ScheduleSpec> {
    match input {
        ScheduleInput::Cron { expr } => {
            let _ = Schedule::from_str(&expr).with_context(|| format!("invalid cron '{expr}'"))?;
            Ok(ScheduleSpec::Cron { expr })
        }
        ScheduleInput::At { run_at } => Ok(ScheduleSpec::At { run_at }),
        ScheduleInput::Every { interval_ms } => {
            if interval_ms <= 0 {
                bail!("every interval must be positive");
            }
            Ok(ScheduleSpec::Every { interval_ms })
        }
        ScheduleInput::Delay { delay_ms } => {
            if delay_ms <= 0 {
                bail!("delay must be positive");
            }
            Ok(ScheduleSpec::At {
                run_at: now + Duration::milliseconds(delay_ms),
            })
        }
    }
}

pub fn initial_next_run_at(
    schedule: &ScheduleSpec,
    timezone: &str,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>> {
    match schedule {
        ScheduleSpec::At { run_at } => Ok(Some(*run_at)),
        ScheduleSpec::Every { interval_ms } => Ok(Some(now + Duration::milliseconds(*interval_ms))),
        ScheduleSpec::Cron { .. } => next_run_after(schedule, timezone, now),
    }
}

pub fn next_run_after(
    schedule: &ScheduleSpec,
    timezone: &str,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>> {
    match schedule {
        ScheduleSpec::At { .. } => Ok(None),
        ScheduleSpec::Every { interval_ms } => {
            if *interval_ms <= 0 {
                bail!("every interval must be positive");
            }
            Ok(Some(after + Duration::milliseconds(*interval_ms)))
        }
        ScheduleSpec::Cron { expr } => {
            let tz = parse_timezone(timezone)?;
            let schedule =
                Schedule::from_str(expr).with_context(|| format!("invalid cron '{expr}'"))?;
            let local_after = after.with_timezone(&tz);
            let next = schedule
                .after(&local_after)
                .next()
                .ok_or_else(|| anyhow!("cron '{expr}' has no next occurrence"))?;
            Ok(Some(next.with_timezone(&Utc)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::ExecutionPrecondition;
    use chrono::TimeZone;

    #[test]
    fn cron_schedule_next_run_is_computed() {
        let now = Utc.with_ymd_and_hms(2026, 3, 19, 10, 0, 0).unwrap();
        let spec = normalize_schedule_input(
            ScheduleInput::Cron {
                expr: "0 30 10 * * *".to_string(),
            },
            now,
        )
        .unwrap();
        let next = initial_next_run_at(&spec, "UTC", now).unwrap().unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 3, 19, 10, 30, 0).unwrap());
    }

    #[test]
    fn at_schedule_round_trips() {
        let now = Utc.with_ymd_and_hms(2026, 3, 19, 10, 0, 0).unwrap();
        let at = Utc.with_ymd_and_hms(2026, 3, 20, 9, 0, 0).unwrap();
        let spec = normalize_schedule_input(ScheduleInput::At { run_at: at }, now).unwrap();
        assert_eq!(spec, ScheduleSpec::At { run_at: at });
        assert_eq!(initial_next_run_at(&spec, "UTC", now).unwrap(), Some(at));
        assert_eq!(next_run_after(&spec, "UTC", at).unwrap(), None);
    }

    #[test]
    fn every_schedule_next_run_is_offset() {
        let now = Utc.with_ymd_and_hms(2026, 3, 19, 10, 0, 0).unwrap();
        let spec = normalize_schedule_input(
            ScheduleInput::Every {
                interval_ms: 30_000,
            },
            now,
        )
        .unwrap();
        let next = initial_next_run_at(&spec, "UTC", now).unwrap().unwrap();
        assert_eq!(next, now + Duration::seconds(30));
    }

    #[test]
    fn delay_is_normalized_to_at() {
        let now = Utc.with_ymd_and_hms(2026, 3, 19, 10, 0, 0).unwrap();
        let spec =
            normalize_schedule_input(ScheduleInput::Delay { delay_ms: 90_000 }, now).unwrap();
        assert_eq!(
            spec,
            ScheduleSpec::At {
                run_at: now + Duration::seconds(90)
            }
        );
    }

    #[test]
    fn invalid_timezone_and_expression_are_rejected() {
        let now = Utc.with_ymd_and_hms(2026, 3, 19, 10, 0, 0).unwrap();
        let spec = normalize_schedule_input(
            ScheduleInput::Cron {
                expr: "0 * * * * *".to_string(),
            },
            now,
        )
        .unwrap();
        assert!(initial_next_run_at(&spec, "Mars/Olympus", now).is_err());
        assert!(normalize_schedule_input(
            ScheduleInput::Cron {
                expr: "not-a-cron".to_string()
            },
            now
        )
        .is_err());
    }

    #[test]
    fn typed_precondition_round_trip() {
        let cond = ExecutionPrecondition::IdleGtSeconds {
            threshold_seconds: 42,
        };
        let json = serde_json::to_string(&cond).unwrap();
        let decoded: ExecutionPrecondition = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, cond);
    }
}
