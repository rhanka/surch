//! Minimal cron-expression parser for the SLM scheduler.
//!
//! Supports the two cron flavours the ES SLM surface accepts:
//!
//! - 5 fields (POSIX): `min hour day month dow`
//! - 6 fields (Quartz): `sec min hour day month dow`
//!
//! For each field we accept:
//!
//! - `*` — wildcard, any value
//! - `?` — Quartz "no specific value"; treated as `*` for `day` and
//!   `dow` (the two fields where Quartz forbids both being `*`)
//! - `N` — single integer
//! - `N-M` — inclusive range
//! - `*/k` or `N/k` — step (every k starting at N or 0)
//! - `a,b,c` — comma-separated list of any of the above
//!
//! No name aliases (`MON`, `JAN`, …) — those are deferred. The ES
//! `@hourly` / `@daily` macros are not part of the SLM wire shape.
//!
//! Day-of-week follows the Quartz convention (1=Sunday..7=Saturday)
//! when 6 fields are given, and the POSIX convention (0=Sunday..6=Sat)
//! when 5 fields are given — this matches the upstream ES behaviour.

use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Timelike, Utc};

/// Parsed cron schedule, ready to compute "next fire after now".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CronSchedule {
    /// 0..=59
    pub seconds: Vec<u32>,
    /// 0..=59
    pub minutes: Vec<u32>,
    /// 0..=23
    pub hours: Vec<u32>,
    /// 1..=31
    pub days_of_month: Vec<u32>,
    /// 1..=12
    pub months: Vec<u32>,
    /// 0..=6 (Sunday = 0)
    pub days_of_week: Vec<u32>,
    /// Whether the schedule had 6 fields (controls dow numbering).
    pub had_seconds: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum CronParseError {
    #[error("cron expression must have 5 or 6 fields, got {0}")]
    BadFieldCount(usize),
    #[error("invalid cron field `{field}`: {reason}")]
    InvalidField { field: String, reason: String },
}

impl CronSchedule {
    /// Parse a cron expression into a [`CronSchedule`].
    pub fn parse(expr: &str) -> Result<Self, CronParseError> {
        let parts: Vec<&str> = expr.split_whitespace().collect();
        let (had_seconds, seconds, rest) = match parts.len() {
            5 => (false, vec![0u32], &parts[..]),
            6 => {
                let sec = parse_field(parts[0], 0, 59)?;
                (true, sec, &parts[1..])
            }
            other => return Err(CronParseError::BadFieldCount(other)),
        };
        let minutes = parse_field(rest[0], 0, 59)?;
        let hours = parse_field(rest[1], 0, 23)?;
        let days_of_month = parse_field(rest[2], 1, 31)?;
        let months = parse_field(rest[3], 1, 12)?;
        // POSIX uses 0..=6 (Sun=0), Quartz uses 1..=7 (Sun=1). Normalize
        // to POSIX (0..=6) so the chrono `weekday().num_days_from_sunday()`
        // call below stays simple.
        let dow_raw = if had_seconds {
            parse_field(rest[4], 1, 7)?
        } else {
            parse_field(rest[4], 0, 6)?
        };
        let days_of_week: Vec<u32> = if had_seconds {
            dow_raw
                .into_iter()
                .map(|n| if n == 7 { 0 } else { n - 1 })
                .collect()
        } else {
            dow_raw
        };

        Ok(Self {
            seconds,
            minutes,
            hours,
            days_of_month,
            months,
            days_of_week,
            had_seconds,
        })
    }

    /// Returns the first instant strictly after `after` (UTC) that
    /// satisfies the schedule. Caps the search at 4 years out — beyond
    /// that the cron expression is presumed unsatisfiable and we return
    /// `None` so callers can surface a config error rather than busy-loop.
    pub fn next_fire_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        // Start one second after `after` so the *strictly after* contract
        // holds even when `after` itself satisfies the schedule.
        let mut cursor = after + chrono::Duration::seconds(1);
        // Quantise sub-second precision away.
        cursor = Utc
            .with_ymd_and_hms(
                cursor.year(),
                cursor.month(),
                cursor.day(),
                cursor.hour(),
                cursor.minute(),
                cursor.second(),
            )
            .single()?;

        // Search bound: 4 years.
        let limit = after + chrono::Duration::days(366 * 4);
        while cursor <= limit {
            if !self.months.contains(&cursor.month()) {
                cursor = advance_to_next_month(cursor)?;
                continue;
            }
            if !self.days_of_month.contains(&cursor.day())
                || !self
                    .days_of_week
                    .contains(&cursor.weekday().num_days_from_sunday())
            {
                cursor = advance_to_next_day(cursor)?;
                continue;
            }
            if !self.hours.contains(&cursor.hour()) {
                cursor = advance_to_next_hour(cursor)?;
                continue;
            }
            if !self.minutes.contains(&cursor.minute()) {
                cursor = advance_to_next_minute(cursor)?;
                continue;
            }
            if !self.seconds.contains(&cursor.second()) {
                cursor += chrono::Duration::seconds(1);
                continue;
            }
            return Some(cursor);
        }
        None
    }
}

fn advance_to_next_minute(t: DateTime<Utc>) -> Option<DateTime<Utc>> {
    Utc.with_ymd_and_hms(t.year(), t.month(), t.day(), t.hour(), t.minute(), 0)
        .single()
        .map(|q| q + chrono::Duration::minutes(1))
}

fn advance_to_next_hour(t: DateTime<Utc>) -> Option<DateTime<Utc>> {
    Utc.with_ymd_and_hms(t.year(), t.month(), t.day(), t.hour(), 0, 0)
        .single()
        .map(|q| q + chrono::Duration::hours(1))
}

fn advance_to_next_day(t: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let next = NaiveDate::from_ymd_opt(t.year(), t.month(), t.day())?
        .checked_add_signed(chrono::Duration::days(1))?;
    Utc.with_ymd_and_hms(next.year(), next.month(), next.day(), 0, 0, 0)
        .single()
}

fn advance_to_next_month(t: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let (next_year, next_month) = if t.month() == 12 {
        (t.year() + 1, 1)
    } else {
        (t.year(), t.month() + 1)
    };
    Utc.with_ymd_and_hms(next_year, next_month, 1, 0, 0, 0)
        .single()
}

fn parse_field(spec: &str, lo: u32, hi: u32) -> Result<Vec<u32>, CronParseError> {
    if spec == "*" || spec == "?" {
        return Ok((lo..=hi).collect());
    }
    let mut acc: Vec<u32> = Vec::new();
    for chunk in spec.split(',') {
        let chunk = chunk.trim();
        if chunk.is_empty() {
            return Err(CronParseError::InvalidField {
                field: spec.to_owned(),
                reason: "empty comma-separated entry".into(),
            });
        }
        let (range_part, step) = if let Some((base, step_str)) = chunk.split_once('/') {
            let step: u32 = step_str.parse().map_err(|_| CronParseError::InvalidField {
                field: spec.to_owned(),
                reason: format!("invalid step `{step_str}`"),
            })?;
            if step == 0 {
                return Err(CronParseError::InvalidField {
                    field: spec.to_owned(),
                    reason: "step must be > 0".into(),
                });
            }
            (base, Some(step))
        } else {
            (chunk, None)
        };

        let (start, end) = if range_part == "*" {
            (lo, hi)
        } else if let Some((a, b)) = range_part.split_once('-') {
            let a: u32 = a.parse().map_err(|_| CronParseError::InvalidField {
                field: spec.to_owned(),
                reason: format!("invalid range start `{a}`"),
            })?;
            let b: u32 = b.parse().map_err(|_| CronParseError::InvalidField {
                field: spec.to_owned(),
                reason: format!("invalid range end `{b}`"),
            })?;
            (a, b)
        } else {
            let n: u32 = range_part
                .parse()
                .map_err(|_| CronParseError::InvalidField {
                    field: spec.to_owned(),
                    reason: format!("invalid integer `{range_part}`"),
                })?;
            // Plain `N/k` (without `-`) means "every k starting at N up to hi".
            let end = if step.is_some() { hi } else { n };
            (n, end)
        };

        if start < lo || end > hi || start > end {
            return Err(CronParseError::InvalidField {
                field: spec.to_owned(),
                reason: format!("range {start}-{end} out of bounds [{lo},{hi}]"),
            });
        }
        let step = step.unwrap_or(1);
        let mut v = start;
        while v <= end {
            acc.push(v);
            v += step;
        }
    }
    acc.sort_unstable();
    acc.dedup();
    Ok(acc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_six_field_es_example() {
        // The ES SLM doc example: every day at 01:30:00.
        let s = CronSchedule::parse("0 30 1 * * ?").expect("parse");
        assert_eq!(s.seconds, vec![0]);
        assert_eq!(s.minutes, vec![30]);
        assert_eq!(s.hours, vec![1]);
    }

    #[test]
    fn parses_five_field_posix() {
        let s = CronSchedule::parse("30 1 * * *").expect("parse");
        assert_eq!(s.seconds, vec![0]);
        assert_eq!(s.minutes, vec![30]);
        assert_eq!(s.hours, vec![1]);
        // dow wildcard = every day.
        assert_eq!(s.days_of_week, vec![0, 1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn step_field_expands() {
        let s = CronSchedule::parse("*/5 * * * * *").expect("parse");
        assert_eq!(
            s.seconds,
            vec![0, 5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55]
        );
    }

    #[test]
    fn rejects_bad_field_count() {
        let err = CronSchedule::parse("a b c").unwrap_err();
        matches!(err, CronParseError::BadFieldCount(3));
    }

    #[test]
    fn next_fire_skips_non_matching_seconds() {
        let s = CronSchedule::parse("0 * * * * *").expect("parse");
        // 12:00:30 -> next 0-second is 12:01:00.
        let after = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 30).unwrap();
        let next = s.next_fire_after(after).expect("next");
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 1, 1, 12, 1, 0).unwrap());
    }

    #[test]
    fn next_fire_every_5_seconds() {
        let s = CronSchedule::parse("*/5 * * * * *").expect("parse");
        let after = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        let next = s.next_fire_after(after).expect("next");
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 5).unwrap());
    }

    #[test]
    fn next_fire_strict_after() {
        let s = CronSchedule::parse("0 30 1 * * ?").expect("parse");
        let on_time = Utc.with_ymd_and_hms(2026, 1, 1, 1, 30, 0).unwrap();
        let next = s.next_fire_after(on_time).expect("next");
        // Strictly after -> tomorrow 01:30:00.
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 1, 2, 1, 30, 0).unwrap());
    }
}
