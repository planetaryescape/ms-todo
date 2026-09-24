// Adapted from mxr crates/daemon/src/commands/activity.rs (`confirm`)
// @ dfb23d10138b1cfc24f8ea7450d3426e5e4da37a. Changes: the caller decides
// whether a prompt is possible (`can_prompt`), so a command off a terminal
// fails before it contacts the daemon; any line counts as an answer.

//! Asking before a destructive command (docs/blueprint/07-cli.md#global-flags):
//! only in a terminal, and never when stdin is carrying input.

use std::io::{BufRead, IsTerminal, Write};

/// Whether there's a person to ask: stdin and stderr are both a terminal.
pub fn can_prompt() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// Ask on stderr, so stdout keeps only the result. Only "y" or "yes" is yes.
pub fn confirm(prompt: &str) -> std::io::Result<bool> {
    let mut stderr = std::io::stderr().lock();
    write!(stderr, "{prompt} [y/N] ")?;
    stderr.flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    let answer = answer.trim().to_lowercase();
    Ok(answer == "y" || answer == "yes")
}
