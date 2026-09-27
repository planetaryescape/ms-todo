//! Showing a notification (S18). On macOS, `osascript`'s `display
//! notification`: `notify-rust` reports success from a process with no
//! app bundle and macOS 27 drops what it sends. The title and body are
//! passed as argv to the script's `on run`, never spliced into it, and no
//! shell is involved, so a title is only ever text. Elsewhere there's no
//! notifier yet, and `doctor` says so.

use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;

use ms_todo_core::one_line_safe;

/// How this machine shows a notification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Notifier {
    Osascript,
    /// Debug builds' tests: one line per notification, `title\tbody`.
    File(PathBuf),
    /// Why there's no way to notify here.
    Unavailable(&'static str),
}

const OSASCRIPT: &str = "/usr/bin/osascript";

/// AppleScript that shows `item 1` of its arguments as the title and
/// `item 2` as the body.
const SCRIPT: [&str; 3] = [
    "on run argv",
    "display notification (item 2 of argv) with title (item 1 of argv)",
    "end run",
];

impl Notifier {
    /// `macos` is whether this system can: tests pass false to stand in
    /// for Linux on a Mac.
    pub fn for_this_system(notify_file: Option<PathBuf>, macos: bool) -> Self {
        match notify_file {
            Some(path) => Self::File(path),
            None if macos => Self::Osascript,
            None => Self::Unavailable(
                "notifications are only built for macOS so far (S18), so nothing nags here",
            ),
        }
    }

    /// What `doctor` calls it, or `None` when there's none.
    pub fn name(&self) -> Option<&'static str> {
        match self {
            Self::Osascript => Some("osascript"),
            Self::File(_) => Some("file"),
            Self::Unavailable(_) => None,
        }
    }

    /// Why there's no way to notify here, if there isn't.
    pub fn unavailable(&self) -> Option<&'static str> {
        match self {
            Self::Unavailable(why) => Some(why),
            _ => None,
        }
    }

    /// Show `title` over `body`, each made one safe line first: they come
    /// from Graph, so from anyone who shares the list.
    pub async fn show(&self, title: &str, body: &str) -> Result<(), String> {
        let (title, body) = (one_line_safe(title), one_line_safe(body));
        match self {
            Self::Osascript => {
                let mut command = tokio::process::Command::new(OSASCRIPT);
                for line in SCRIPT {
                    command.args(["-e", line]);
                }
                let output = command
                    .arg("--")
                    .args([&title, &body])
                    .stdin(Stdio::null())
                    .output()
                    .await
                    .map_err(|error| format!("cannot run {OSASCRIPT}: {error}"))?;
                if output.status.success() {
                    Ok(())
                } else {
                    Err(format!(
                        "{OSASCRIPT} failed ({}): {}",
                        output.status,
                        String::from_utf8_lossy(&output.stderr).trim()
                    ))
                }
            }
            Self::File(path) => std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut file| writeln!(file, "{title}\t{body}"))
                .map_err(|error| format!("{}: {error}", path.display())),
            Self::Unavailable(why) => Err((*why).to_owned()),
        }
    }
}
