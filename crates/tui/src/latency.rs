//! Measuring the latency budget (docs/blueprint/08-tui.md#latency-budget):
//! under 16 ms from a keypress to its frame, and under 150 ms from start
//! to the first painted list. Each measurement is a tracing span or event
//! (`MS_TODO_TUI_TRACE=<file>` writes them), and `--bench-startup` prints
//! a summary.

use std::fmt::Write as _;
use std::time::Duration;

pub const KEYPRESS_BUDGET: Duration = Duration::from_millis(16);
pub const COLD_START_BUDGET: Duration = Duration::from_millis(150);

#[derive(Clone, Debug, Default)]
pub struct Latency {
    /// Process start to the first frame with a seeded list.
    pub cold_start: Option<Duration>,
    /// A keypress to the frame that shows it.
    pub keypress: Vec<Duration>,
    /// A key that changed the scope to the frame with the new scope's
    /// tasks: a keypress plus a `Seed` round trip.
    pub view_switch: Vec<Duration>,
    /// A key that wrote (add, complete, delete) to the frame with the
    /// daemon's `Applied` answer drawn.
    pub write: Vec<Duration>,
}

impl Latency {
    pub fn report(&self) -> String {
        let mut report = String::new();
        match self.cold_start {
            Some(cold) => {
                let _ = writeln!(
                    report,
                    "cold start to first painted list: {} (budget {})",
                    millis(cold),
                    millis(COLD_START_BUDGET)
                );
            }
            None => report.push_str("cold start to first painted list: never painted\n"),
        }
        for (name, samples) in [
            ("keypress to render", &self.keypress),
            ("view switch to painted list", &self.view_switch),
            ("write to painted change", &self.write),
        ] {
            if let Some(summary) = summarize(samples) {
                let _ = writeln!(
                    report,
                    "{name}: {summary} (budget {})",
                    millis(KEYPRESS_BUDGET)
                );
            }
        }
        report
    }
}

fn summarize(samples: &[Duration]) -> Option<String> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort();
    let at = |fraction: f64| {
        // Nearest rank.
        let rank = (fraction * sorted.len() as f64).ceil() as usize;
        sorted[rank.clamp(1, sorted.len()) - 1]
    };
    Some(format!(
        "{} samples, median {}, p95 {}, max {}",
        sorted.len(),
        millis(at(0.5)),
        millis(at(0.95)),
        millis(sorted[sorted.len() - 1])
    ))
}

fn millis(duration: Duration) -> String {
    format!("{:.2} ms", duration.as_secs_f64() * 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_summarizes_each_measurement() {
        let latency = Latency {
            cold_start: Some(Duration::from_millis(42)),
            keypress: (1..=20).map(Duration::from_millis).collect(),
            view_switch: Vec::new(),
            write: vec![Duration::from_micros(1500)],
        };
        assert_eq!(
            latency.report(),
            "cold start to first painted list: 42.00 ms (budget 150.00 ms)\n\
             keypress to render: 20 samples, median 10.00 ms, p95 19.00 ms, max 20.00 ms (budget 16.00 ms)\n\
             write to painted change: 1 samples, median 1.50 ms, p95 1.50 ms, max 1.50 ms (budget 16.00 ms)\n"
        );
    }
}
