//! `ms-todo sync [--wait]`: refresh the daemon's cache from Graph. With
//! `--wait` it returns once a sync that started after the command has
//! finished (docs/blueprint/04-sync-cache.md#freshness-guarantee-for-the-cli),
//! showing progress on stderr in a terminal.

use std::io::{IsTerminal, Write};

use ms_todo_core::Paths;
use ms_todo_protocol::{Event, Request, ResponseData, SyncProgress, SyncReport};

use crate::daemon_client;
use crate::error::CliError;
use crate::output::Render;

impl Render for SyncReport {
    fn table_rows(&self) -> Vec<(&'static str, String)> {
        if !self.waited {
            return vec![("Sync", "started; `ms-todo sync --wait` waits for it".into())];
        }
        vec![
            ("Synced", format!("{} scopes", self.scopes)),
            ("Changed", self.changed.to_string()),
            ("Generation", self.generation.to_string()),
        ]
    }
}

pub async fn sync(paths: &Paths, wait: bool) -> Result<SyncReport, CliError> {
    let show = std::io::stderr().is_terminal();
    let mut shown = false;
    let answer = daemon_client::ask_with_events(paths, Request::Sync { wait }, |event| {
        if let Event::SyncProgress(progress) = event
            && show
        {
            shown = true;
            show_progress(&progress);
        }
    })
    .await;
    if shown {
        eprintln!();
    }
    match answer? {
        ResponseData::Sync(report) => Ok(report),
        _ => Err(crate::unexpected_response()),
    }
}

// One line, rewritten in place.
fn show_progress(progress: &SyncProgress) {
    let mut stderr = std::io::stderr().lock();
    let counts = if progress.scopes_total == 0 {
        String::new()
    } else {
        format!(" {}/{}", progress.scopes_done, progress.scopes_total)
    };
    let doing = if progress.doing.is_empty() {
        String::new()
    } else {
        format!(": {}", progress.doing)
    };
    let _ = write!(stderr, "\r\x1b[2KSyncing{counts}{doing}");
    let _ = stderr.flush();
}
