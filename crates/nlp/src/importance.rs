//! Importance by number, D-017's Todoist mapping: 1 (or `p1`) is high,
//! 2 and 3 are normal, 4 is low. Graph has three levels, so 2 and 3 land
//! on the same one; keeping the level typed in our extension is Q3 in
//! docs/blueprint/12-open-questions.md, still open. `p1`–`p4` is also a
//! quick-add token in rung 6, which reads it through here.

use crate::NotUnderstood;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Importance {
    Low,
    Normal,
    High,
}

impl Importance {
    /// Graph's name for it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
        }
    }
}

/// `1`–`4`, `p1`–`p4`, or `high`, `normal` or `low`, in any case.
pub fn read_importance(input: &str) -> Result<Importance, NotUnderstood> {
    let typed = input.trim().to_lowercase();
    let level = typed.strip_prefix('p').unwrap_or(&typed);
    match (level, typed.as_str()) {
        ("1", _) | (_, "high") => Ok(Importance::High),
        ("2" | "3", _) | (_, "normal") => Ok(Importance::Normal),
        ("4", _) | (_, "low") => Ok(Importance::Low),
        _ => Err(NotUnderstood(format!(
            "didn't understand \"{}\": use 1 (high), 2 or 3 (normal), 4 (low), p1–p4, \
             high, normal or low",
            input.trim()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_p_levels_and_names_follow_d017() {
        for (input, expected) in [
            ("1", Importance::High),
            ("p1", Importance::High),
            ("P1", Importance::High),
            ("high", Importance::High),
            (" High ", Importance::High),
            ("2", Importance::Normal),
            ("3", Importance::Normal),
            ("p2", Importance::Normal),
            ("p3", Importance::Normal),
            ("normal", Importance::Normal),
            ("4", Importance::Low),
            ("p4", Importance::Low),
            ("LOW", Importance::Low),
        ] {
            assert_eq!(read_importance(input), Ok(expected), "{input:?}");
        }
    }

    #[test]
    fn anything_else_is_refused_by_name() {
        for input in ["0", "5", "p5", "pp1", "urgent", "", "phigh"] {
            let why = read_importance(input).expect_err(input);
            assert!(why.0.contains("1 (high)"), "{input:?}: {why}");
        }
    }
}
