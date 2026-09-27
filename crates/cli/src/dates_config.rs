//! `[dates]` in config.toml (D-068): how dates are written where the user
//! lives. The CLI reads phrases itself (`phrases.rs`), and hands the same
//! settings to the TUI, so both read `12/10` and `next week` alike.
//!
//! ```toml
//! [dates]
//! date_order = "mdy"     # 12/10 is 10 December; "dmy" (the default) is 12 October
//! week_start = "sunday"  # next week starts on Sunday; "monday" is the default
//! ```

use std::io::ErrorKind;
use std::path::Path;

use ms_todo_nlp::{DateOrder, Locale, WeekStart};

/// The locale `path` sets; the defaults when there's no file or no
/// `[dates]`. Every key is checked, so a typo is an error naming it
/// rather than a setting that silently does nothing.
pub fn read(path: &Path) -> Result<Locale, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Locale::default()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    parse(&raw).map_err(|why| format!("{}: {why}", path.display()))
}

fn parse(raw: &str) -> Result<Locale, String> {
    let document: toml::Table = raw.parse().map_err(|error| format!("{error}"))?;
    let Some(dates) = document.get("dates") else {
        return Ok(Locale::default());
    };
    let dates = dates.as_table().ok_or("dates must be a table: [dates]")?;
    let mut locale = Locale::default();
    for (key, value) in dates {
        let text = value.as_str();
        match key.as_str() {
            "date_order" => {
                locale.date_order = text.and_then(DateOrder::from_name).ok_or(
                    "dates.date_order must be \"dmy\" (12/10 is 12 October) or \"mdy\" (10 \
                     December)",
                )?;
            }
            "week_start" => {
                locale.week_start = text
                    .and_then(WeekStart::from_name)
                    .ok_or("dates.week_start must be \"monday\" or \"sunday\"")?;
            }
            other => {
                return Err(format!(
                    "dates.{other} is not a setting; [dates] takes date_order and week_start"
                ));
            }
        }
    }
    Ok(locale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_day_first_and_monday() {
        assert_eq!(parse(""), Ok(Locale::default()));
        assert_eq!(parse("[tui]\ntheme = \"kanagawa\""), Ok(Locale::default()));
        assert_eq!(
            read(Path::new("/nowhere/config.toml")),
            Ok(Locale::default())
        );
        assert_eq!(Locale::default().date_order, DateOrder::DayMonth);
        assert_eq!(Locale::default().week_start, WeekStart::Monday);
    }

    #[test]
    fn dates_sets_the_order_and_the_week() {
        let locale = parse("[dates]\ndate_order = \"mdy\"\nweek_start = \"sunday\"\n");
        assert_eq!(
            locale,
            Ok(Locale {
                date_order: DateOrder::MonthDay,
                week_start: WeekStart::Sunday,
            })
        );
    }

    #[test]
    fn a_bad_value_or_key_names_itself() {
        let error = parse("[dates]\ndate_order = \"ymd\"").expect_err("bad order");
        assert!(error.contains("dates.date_order"), "{error}");
        let error = parse("[dates]\nweek_start = 1").expect_err("bad start");
        assert!(error.contains("dates.week_start"), "{error}");
        let error = parse("[dates]\nweekstart = \"sunday\"").expect_err("typo");
        assert!(
            error.contains("dates.weekstart is not a setting"),
            "{error}"
        );
        assert!(parse("dates = 1").is_err());
    }
}
