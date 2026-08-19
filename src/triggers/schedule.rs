//! A minimal five-field cron parser and its "next firing" arithmetic.
//!
//! No crate for this: the field grammar is small (lists, ranges, steps,
//! wildcards) and every general-purpose cron crate available pulls in more
//! surface than parsing five comma-separated fields warrants. `next_after`
//! walks minute-by-minute rather than solving each field analytically —
//! slower, but the whole function fits in one screen and is trivial to audit,
//! which matters more for a scheduler that decides when smed spends a
//! provider turn unattended.

use std::collections::BTreeSet;

use time::{OffsetDateTime, Weekday};

/// A parsed `minute hour day-of-month month day-of-week` expression, UTC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronSchedule {
    minutes: BTreeSet<u8>,
    hours: BTreeSet<u8>,
    days_of_month: BTreeSet<u8>,
    months: BTreeSet<u8>,
    days_of_week: BTreeSet<u8>,
}

/// How far ahead `next_after` will search before giving up.
///
/// A schedule that can never match (`31 2 30 2 *` — February never has a
/// 30th) must terminate rather than loop forever; four years covers every
/// leap-year alignment a valid expression could need.
const SEARCH_LIMIT_MINUTES: i64 = 4 * 366 * 24 * 60;

impl CronSchedule {
    /// # Errors
    /// A human-readable reason the expression could not be parsed.
    pub fn parse(expression: &str) -> Result<Self, String> {
        let fields: Vec<&str> = expression.split_whitespace().collect();
        let [minute, hour, day_of_month, month, day_of_week] = fields.as_slice() else {
            return Err(format!(
                "expected 5 fields (minute hour day-of-month month day-of-week), found {}",
                fields.len()
            ));
        };
        Ok(Self {
            minutes: parse_field(minute, 0, 59)?,
            hours: parse_field(hour, 0, 23)?,
            days_of_month: parse_field(day_of_month, 1, 31)?,
            months: parse_field(month, 1, 12)?,
            days_of_week: parse_field(day_of_week, 0, 6)?,
        })
    }

    /// The next minute-aligned instant strictly after `from` that this
    /// schedule matches, or `None` if none exists within the search limit.
    #[must_use]
    pub fn next_after(&self, from: OffsetDateTime) -> Option<OffsetDateTime> {
        let start = from
            .replace_second(0)
            .ok()?
            .replace_nanosecond(0)
            .ok()?
            .saturating_add(time::Duration::minutes(1));
        let mut candidate = start;
        for _ in 0..SEARCH_LIMIT_MINUTES {
            if self.matches(candidate) {
                return Some(candidate);
            }
            candidate = candidate.saturating_add(time::Duration::minutes(1));
        }
        None
    }

    fn matches(&self, when: OffsetDateTime) -> bool {
        self.minutes.contains(&when.minute())
            && self.hours.contains(&when.hour())
            && self.days_of_month.contains(&when.day())
            && self.months.contains(&(when.month() as u8))
            && self.days_of_week.contains(&weekday_number(when.weekday()))
    }
}

const fn weekday_number(weekday: Weekday) -> u8 {
    // Cron's day-of-week is 0 = Sunday .. 6 = Saturday.
    match weekday {
        Weekday::Sunday => 0,
        Weekday::Monday => 1,
        Weekday::Tuesday => 2,
        Weekday::Wednesday => 3,
        Weekday::Thursday => 4,
        Weekday::Friday => 5,
        Weekday::Saturday => 6,
    }
}

fn parse_field(field: &str, min: u8, max: u8) -> Result<BTreeSet<u8>, String> {
    let mut values = BTreeSet::new();
    for part in field.split(',') {
        let (range_part, step) = match part.split_once('/') {
            Some((range_part, step)) => (
                range_part,
                step.parse::<u8>()
                    .map_err(|_| format!("invalid step in `{part}`"))?,
            ),
            None => (part, 1),
        };
        if step == 0 {
            return Err(format!("step 0 in `{part}` never matches"));
        }
        let (low, high) = if range_part == "*" {
            (min, max)
        } else if let Some((low, high)) = range_part.split_once('-') {
            (
                low.parse::<u8>()
                    .map_err(|_| format!("invalid range start in `{part}`"))?,
                high.parse::<u8>()
                    .map_err(|_| format!("invalid range end in `{part}`"))?,
            )
        } else {
            let value = range_part
                .parse::<u8>()
                .map_err(|_| format!("invalid value `{range_part}`"))?;
            (value, value)
        };
        if low < min || high > max || low > high {
            return Err(format!("`{part}` is outside the valid range {min}-{max}"));
        }
        let mut value = low;
        while value <= high {
            values.insert(value);
            let Some(next) = value.checked_add(step) else {
                break;
            };
            value = next;
        }
    }
    if values.is_empty() {
        return Err(format!("`{field}` matches nothing"));
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn a_wildcard_schedule_matches_every_minute() {
        let schedule = CronSchedule::parse("* * * * *").expect("parse");
        let now = datetime!(2026-07-20 10:30:00 UTC);
        assert_eq!(
            schedule.next_after(now),
            Some(now.saturating_add(time::Duration::minutes(1)))
        );
    }

    #[test]
    fn a_daily_schedule_finds_the_next_matching_day() {
        // Every day at 03:00 UTC.
        let schedule = CronSchedule::parse("0 3 * * *").expect("parse");
        let now = datetime!(2026-07-20 10:30:00 UTC);
        let next = schedule.next_after(now).expect("next firing");
        assert_eq!(next, datetime!(2026-07-21 03:00:00 UTC));
    }

    #[test]
    fn a_schedule_already_at_the_boundary_advances_to_the_next_one() {
        let schedule = CronSchedule::parse("0 3 * * *").expect("parse");
        let now = datetime!(2026-07-20 03:00:00 UTC);
        let next = schedule.next_after(now).expect("next firing");
        assert_eq!(next, datetime!(2026-07-21 03:00:00 UTC));
    }

    #[test]
    fn a_step_expression_matches_every_fifteen_minutes() {
        let schedule = CronSchedule::parse("*/15 * * * *").expect("parse");
        let now = datetime!(2026-07-20 10:16:00 UTC);
        let next = schedule.next_after(now).expect("next firing");
        assert_eq!(next, datetime!(2026-07-20 10:30:00 UTC));
    }

    #[test]
    fn day_of_week_restricts_to_matching_weekdays() {
        // Mondays only.
        let schedule = CronSchedule::parse("0 9 * * 1").expect("parse");
        let now = datetime!(2026-07-20 10:00:00 UTC); // a Monday
        let next = schedule.next_after(now).expect("next firing");
        assert_eq!(next.weekday(), time::Weekday::Monday);
        assert!(next > now);
    }

    #[test]
    fn five_fields_are_required() {
        assert!(CronSchedule::parse("* * * *").is_err());
        assert!(CronSchedule::parse("* * * * * *").is_err());
    }

    #[test]
    fn an_out_of_range_value_is_refused() {
        assert!(CronSchedule::parse("60 * * * *").is_err());
        assert!(CronSchedule::parse("* 24 * * *").is_err());
    }

    #[test]
    fn an_impossible_schedule_terminates_with_none() {
        // February never has a 30th.
        let schedule = CronSchedule::parse("0 0 30 2 *").expect("parse");
        let now = datetime!(2026-07-20 10:00:00 UTC);
        assert_eq!(schedule.next_after(now), None);
    }
}
