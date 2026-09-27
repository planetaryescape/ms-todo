//! How often a nag repeats (rung 9b): `15m`, `1h`, `1h30m`, `90 min`,
//! `2 hours`. Whole minutes; the bounds are the daemon's to enforce.

use crate::NotUnderstood;

/// `input` as a number of minutes, above zero.
pub fn read_interval(input: &str) -> Result<u32, NotUnderstood> {
    let typed: String = input
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    let not_understood = || {
        NotUnderstood(format!(
            "didn't understand \"{}\": use minutes or hours, such as 15m, 1h or 1h30m",
            input.trim()
        ))
    };
    let mut rest = typed.as_str();
    let mut minutes: u32 = 0;
    let mut last_unit = u32::MAX;
    while !rest.is_empty() {
        let digits = rest
            .find(|ch: char| !ch.is_ascii_digit())
            .unwrap_or(rest.len());
        let number: u32 = rest[..digits].parse().map_err(|_| not_understood())?;
        rest = &rest[digits..];
        let unit_end = rest
            .find(|ch: char| ch.is_ascii_digit())
            .unwrap_or(rest.len());
        let unit = match &rest[..unit_end] {
            "h" | "hr" | "hrs" | "hour" | "hours" => 60,
            "m" | "min" | "mins" | "minute" | "minutes" => 1,
            _ => return Err(not_understood()),
        };
        // Hours before minutes, each once: `30m1h` is a typo, not 90.
        if unit >= last_unit {
            return Err(not_understood());
        }
        last_unit = unit;
        minutes = number
            .checked_mul(unit)
            .and_then(|part| minutes.checked_add(part))
            .ok_or_else(not_understood)?;
        rest = &rest[unit_end..];
    }
    if minutes == 0 {
        return Err(not_understood());
    }
    Ok(minutes)
}

/// `15m`, `1h` or `1h30m`: an interval as `+nag` takes it.
pub fn interval_label(minutes: u32) -> String {
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes}m"),
        (hours, 0) => format!("{hours}h"),
        (hours, minutes) => format!("{hours}h{minutes}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minutes_and_hours_in_the_forms_people_type() {
        for (typed, minutes) in [
            ("15m", 15),
            ("5 min", 5),
            ("1h", 60),
            ("2 hours", 120),
            ("1h30m", 90),
            ("1H 30M", 90),
            ("90minutes", 90),
        ] {
            assert_eq!(read_interval(typed).expect(typed), minutes, "{typed}");
        }
    }

    #[test]
    fn anything_else_is_not_understood() {
        for typed in [
            "",
            "15",
            "m",
            "0m",
            "15s",
            "1d",
            "30m1h",
            "1h1h",
            "-5m",
            "99999999999h",
        ] {
            assert!(read_interval(typed).is_err(), "{typed}");
        }
    }
}
