//! Opening a link with the system's opener, shared by the TUI's `o` and
//! `ms-todo tasks open`. Behind a trait, so tests never start a browser.

use std::process::{Command, Stdio};

use url::Url;

pub trait Opener {
    fn open(&self, url: &Url) -> std::io::Result<()>;
}

/// `open` on macOS, `xdg-open` elsewhere: run directly, never through a
/// shell, with the URL as its one argument. Only URLs that passed
/// [`ms_todo_core::links::openable`] get here, so none starts with `-`.
pub struct SystemOpener;

impl Opener for SystemOpener {
    fn open(&self, url: &Url) -> std::io::Result<()> {
        let program = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        // Its output would draw over the TUI, so it has none.
        let mut child = Command::new(program)
            .arg(url.as_str())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        // Reaped on its own thread: xdg-open can wait for the browser.
        std::thread::spawn(move || child.wait());
        Ok(())
    }
}
