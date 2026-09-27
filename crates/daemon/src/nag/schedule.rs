//! When a nag is shown: the pure rules, with the clock passed in.

use chrono::{DateTime, Duration, NaiveTime, Utc};

/// A span of the local day when nothing is shown, which may wrap past
/// midnight (`22:00-07:00`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QuietHours {
    pub from: NaiveTime,
    pub until: NaiveTime,
}

impl QuietHours {
    /// `HH:MM-HH:MM`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let bad = || {
            format!(
                "nag.quiet_hours = {text:?} isn't a span of the day: use \"HH:MM-HH:MM\", such \
                 as \"22:00-07:00\", or \"\" for none"
            )
        };
        let (from, until) = text.trim().split_once('-').ok_or_else(bad)?;
        let time = |part: &str| NaiveTime::parse_from_str(part.trim(), "%H:%M").map_err(|_| bad());
        let (from, until) = (time(from)?, time(until)?);
        if from == until {
            return Err(bad());
        }
        Ok(Self { from, until })
    }

    /// Whether local time `now` is inside: from included, until not.
    pub fn contains(self, now: NaiveTime) -> bool {
        if self.from < self.until {
            self.from <= now && now < self.until
        } else {
            now >= self.from || now < self.until
        }
    }

    pub fn label(self) -> String {
        format!(
            "{}-{}",
            self.from.format("%H:%M"),
            self.until.format("%H:%M")
        )
    }
}

/// Whether a task whose reminder is at `reminder` and that nags every
/// `every` is due a notification at `now`, given when it was last shown
/// one. Once the reminder passes, it's due at once, then `every` after
/// each one shown. A notification shown before the reminder was moved
/// later doesn't count. Only the last one is kept, so however long the
/// daemon was away, it owes one notification, not one per interval
/// missed.
pub(crate) fn due(
    now: DateTime<Utc>,
    reminder: DateTime<Utc>,
    every: Duration,
    last: Option<DateTime<Utc>>,
) -> bool {
    if now < reminder {
        return false;
    }
    match last {
        Some(last) if last >= reminder => now - last >= every,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("time")
            .with_timezone(&Utc)
    }

    fn time(text: &str) -> NaiveTime {
        NaiveTime::parse_from_str(text, "%H:%M").expect("time")
    }

    #[test]
    fn nothing_before_the_reminder_then_at_once_then_every_interval() {
        let reminder = at("2026-09-27T09:00:00Z");
        let every = Duration::minutes(15);
        assert!(!due(at("2026-09-27T08:59:00Z"), reminder, every, None));
        assert!(due(at("2026-09-27T09:00:00Z"), reminder, every, None));
        let shown = Some(at("2026-09-27T09:00:30Z"));
        assert!(!due(at("2026-09-27T09:15:00Z"), reminder, every, shown));
        assert!(due(at("2026-09-27T09:15:30Z"), reminder, every, shown));
    }

    #[test]
    fn a_long_absence_owes_one_notification_not_one_per_interval() {
        let reminder = at("2026-09-27T09:00:00Z");
        let every = Duration::minutes(5);
        let shown = at("2026-09-27T09:05:00Z");
        // The daemon was stopped for hours: one is due now…
        let back = at("2026-09-27T13:00:00Z");
        assert!(due(back, reminder, every, Some(shown)));
        // …and once it's shown, the next waits a whole interval.
        assert!(!due(
            back + Duration::seconds(30),
            reminder,
            every,
            Some(back)
        ));
    }

    #[test]
    fn a_reminder_moved_later_starts_again_from_it() {
        let every = Duration::minutes(15);
        let shown = Some(at("2026-09-27T09:00:00Z"));
        let moved = at("2026-09-27T12:00:00Z");
        assert!(!due(at("2026-09-27T11:00:00Z"), moved, every, shown));
        assert!(due(at("2026-09-27T12:00:00Z"), moved, every, shown));
    }

    #[test]
    fn quiet_hours_wrap_past_midnight_or_not() {
        let night = QuietHours::parse("22:00-07:00").expect("night");
        assert!(night.contains(time("22:00")));
        assert!(night.contains(time("03:00")));
        assert!(!night.contains(time("07:00")));
        assert!(!night.contains(time("12:00")));
        let lunch = QuietHours::parse(" 12:00 - 13:30 ").expect("lunch");
        assert!(lunch.contains(time("12:45")));
        assert!(!lunch.contains(time("13:30")));
        assert_eq!(lunch.label(), "12:00-13:30");
        for bad in ["22:00", "22-07", "07:00-07:00", "25:00-07:00"] {
            assert!(QuietHours::parse(bad).is_err(), "{bad}");
        }
    }
}
