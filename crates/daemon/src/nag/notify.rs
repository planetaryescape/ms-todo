//! Showing a notification (S18). On macOS, `osascript`'s `display
//! notification`: `notify-rust` reports success from a process with no
//! app bundle and macOS 27 drops what it sends. The title and body are
//! passed as argv to the script's `on run`, never spliced into it, and no
//! shell is involved, so a title is only ever text. On Linux, `notify-rust`
//! over the D-Bus session bus, when there is one (D-065); a failure to
//! reach a notification server shows in `doctor`. Elsewhere there's no
//! notifier, and `doctor` says so.

use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;

use ms_todo_core::one_line_safe;

/// How this machine shows a notification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Notifier {
    Osascript,
    /// The freedesktop notification service, over the D-Bus session bus.
    DBus,
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
    /// `macos` is whether this system can with `osascript`, and `dbus`
    /// whether it has a D-Bus session bus to send to: tests pass false for
    /// both to stand in for a system with neither.
    pub fn for_this_system(notify_file: Option<PathBuf>, macos: bool, dbus: bool) -> Self {
        match notify_file {
            Some(path) => Self::File(path),
            None if macos => Self::Osascript,
            None if dbus => Self::DBus,
            None if cfg!(target_os = "linux") => Self::Unavailable(
                "there's no D-Bus session bus (DBUS_SESSION_BUS_ADDRESS), so nothing nags here",
            ),
            None => Self::Unavailable(
                "notifications are built for macOS and Linux only, so nothing nags here",
            ),
        }
    }

    /// What `doctor` calls it, or `None` when there's none.
    pub fn name(&self) -> Option<&'static str> {
        match self {
            Self::Osascript => Some("osascript"),
            Self::DBus => Some("dbus"),
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
            Self::DBus => show_over_dbus(&title, &body).await,
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

/// Whether this session has a D-Bus session bus, where zbus looks for one:
/// `DBUS_SESSION_BUS_ADDRESS`, else `$XDG_RUNTIME_DIR/bus`.
pub(crate) fn session_bus() -> bool {
    std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some_and(|address| !address.is_empty())
        || std::env::var_os("XDG_RUNTIME_DIR")
            .is_some_and(|dir| std::path::Path::new(&dir).join("bus").exists())
}

#[cfg(target_os = "linux")]
async fn show_over_dbus(title: &str, body: &str) -> Result<(), String> {
    notify_rust::Notification::new()
        .appname("ms-todo")
        .summary(title)
        .body(body)
        .show_async()
        .await
        .map(|_| ())
        .map_err(|error| format!("cannot notify over D-Bus: {error}"))
}

#[cfg(not(target_os = "linux"))]
async fn show_over_dbus(_title: &str, _body: &str) -> Result<(), String> {
    Err("D-Bus notifications are built for Linux only".to_owned())
}
