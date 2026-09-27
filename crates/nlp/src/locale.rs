//! How dates are written where the user lives: which comes first in
//! `12/10`, and which day starts a week (`[dates]` in config.toml, D-068).
//! The defaults are the UK's, what ms-todo read before these were
//! settings: day first, weeks from Monday.

use chrono::{Datelike, Days, NaiveDate, Weekday};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Locale {
    pub date_order: DateOrder,
    pub week_start: WeekStart,
}

/// Which number comes first in a date written with a slash.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DateOrder {
    /// `12/10` is 12 October (`dmy`).
    #[default]
    DayMonth,
    /// `12/10` is 10 December (`mdy`).
    MonthDay,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WeekStart {
    #[default]
    Monday,
    Sunday,
}

impl DateOrder {
    /// `dmy` or `mdy`, as config.toml spells it.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "dmy" => Some(Self::DayMonth),
            "mdy" => Some(Self::MonthDay),
            _ => None,
        }
    }
}

impl WeekStart {
    /// `monday` or `sunday`, as config.toml spells it.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "monday" => Some(Self::Monday),
            "sunday" => Some(Self::Sunday),
            _ => None,
        }
    }

    /// As Graph's `firstDayOfWeek` spells it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Monday => "monday",
            Self::Sunday => "sunday",
        }
    }

    pub fn weekday(self) -> Weekday {
        match self {
            Self::Monday => Weekday::Mon,
            Self::Sunday => Weekday::Sun,
        }
    }

    /// The first day of the week `date` is in.
    pub fn week_of(self, date: NaiveDate) -> NaiveDate {
        let into = u64::from(date.weekday().days_since(self.weekday()));
        date.checked_sub_days(Days::new(into)).unwrap_or(date)
    }

    /// The last day of the week `date` is in.
    pub fn end_of_week(self, date: NaiveDate) -> NaiveDate {
        let first = self.week_of(date);
        first.checked_add_days(Days::new(6)).unwrap_or(first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").expect("a date")
    }

    #[test]
    fn a_week_runs_from_its_first_day() {
        // Thursday 24 September 2026.
        let thursday = day("2026-09-24");
        assert_eq!(WeekStart::Monday.week_of(thursday), day("2026-09-21"));
        assert_eq!(WeekStart::Monday.end_of_week(thursday), day("2026-09-27"));
        assert_eq!(WeekStart::Sunday.week_of(thursday), day("2026-09-20"));
        assert_eq!(WeekStart::Sunday.end_of_week(thursday), day("2026-09-26"));
        // A Sunday starts its own week, or ends Monday's.
        let sunday = day("2026-09-27");
        assert_eq!(WeekStart::Sunday.week_of(sunday), sunday);
        assert_eq!(WeekStart::Monday.week_of(sunday), day("2026-09-21"));
    }

    #[test]
    fn names_are_config_tomls() {
        assert_eq!(DateOrder::from_name("mdy"), Some(DateOrder::MonthDay));
        assert_eq!(DateOrder::from_name("dmy"), Some(DateOrder::DayMonth));
        assert_eq!(DateOrder::from_name("ymd"), None);
        assert_eq!(WeekStart::from_name("sunday"), Some(WeekStart::Sunday));
        assert_eq!(WeekStart::from_name("Sunday"), None);
    }
}
