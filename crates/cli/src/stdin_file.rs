//! `attachments add T -` (D-067): stdin saved to a private temporary file
//! (0600, a new random name, never through a link), so the daemon can
//! read it by path as it reads any file; bytes never cross the socket. The
//! daemon copies it into its own staging directory before answering, and
//! the file goes when this is dropped.

use std::fs::{File, OpenOptions};
use std::io::{IsTerminal, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use ms_todo_core::{ErrorKind, MAX_ATTACHMENT_BYTES};

use crate::error::CliError;

pub struct StdinFile {
    path: PathBuf,
}

impl StdinFile {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StdinFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Save stdin, 25 MB at most, into a new private temporary file.
pub fn save() -> Result<StdinFile, CliError> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Err(invalid(
            "`-` attaches what's piped to stdin, but stdin is a terminal: pipe the file in, or \
             name it"
                .into(),
        ));
    }
    let path = std::env::temp_dir().join(format!("ms-todo-stdin-{}", uuid::Uuid::new_v4()));
    let mut file: File = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    // Owned from here, so an error below removes it.
    let saved = StdinFile { path };
    let limit = MAX_ATTACHMENT_BYTES as u64;
    let copied = std::io::copy(&mut stdin.lock().take(limit + 1), &mut file)?;
    if copied > limit {
        return Err(invalid(format!(
            "stdin holds more than 25 MB ({MAX_ATTACHMENT_BYTES} bytes), the most Microsoft To \
             Do takes"
        )));
    }
    if copied == 0 {
        return Err(invalid("stdin was empty: there's nothing to attach".into()));
    }
    file.flush()?;
    Ok(saved)
}

fn invalid(message: String) -> CliError {
    CliError::message(ErrorKind::InvalidInput, message)
}
