//! A move's attachment bytes, kept on disk from the moment they're read
//! from the source until the move is settled: the copy is uploaded from
//! them, checked against their hashes, and if the source and the copy are
//! both lost they're the only copy left (04: the full local copy). Files
//! are 0600 in a 0700 directory under the instance's data directory,
//! named by a hash of the operation's ID, which comes from a client.
//!
//! Attachments run to 25 MB, so the disk and the hashing run off the async
//! workers, where they'd stall the daemon's other requests.

use std::io;
use std::path::{Path, PathBuf};

use ms_todo_graph::private_file::{atomic_write_mode_0600, ensure_private_dir};
use sha2::{Digest, Sha256};

/// `bytes`' sha256, as hex.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The directory of operation `op_id`'s files.
fn dir(root: &Path, op_id: &str) -> PathBuf {
    root.join(&sha256_hex(op_id.as_bytes())[..32])
}

/// Disk work, off the async workers: files run to 25 MB.
pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(io::Error::other)?
}

/// `bytes`' sha256, as hex.
pub(crate) async fn hash(bytes: Vec<u8>) -> io::Result<(Vec<u8>, String)> {
    blocking(move || {
        let hash = sha256_hex(&bytes);
        Ok((bytes, hash))
    })
    .await
}

/// Keep `bytes` as the file `name` of operation `op_id`; returns their
/// sha256.
pub(crate) async fn write(
    root: &Path,
    op_id: &str,
    name: &str,
    bytes: Vec<u8>,
) -> io::Result<String> {
    let dir = dir(root, op_id);
    let path = dir.join(name);
    blocking(move || {
        ensure_private_dir(&dir)?;
        atomic_write_mode_0600(&path, &bytes)?;
        Ok(sha256_hex(&bytes))
    })
    .await
}

/// The file `name` of operation `op_id`, and its sha256.
pub(crate) async fn read(root: &Path, op_id: &str, name: &str) -> io::Result<(Vec<u8>, String)> {
    let path = dir(root, op_id).join(name);
    blocking(move || {
        let bytes = std::fs::read(path)?;
        let hash = sha256_hex(&bytes);
        Ok((bytes, hash))
    })
    .await
}

/// Forget operation `op_id`'s files, once the move is settled.
pub(crate) async fn remove(root: &Path, op_id: &str) {
    let dir = dir(root, op_id);
    let removed = blocking(move || match std::fs::remove_dir_all(dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    })
    .await;
    if let Err(error) = removed {
        eprintln!("ms-todo daemon: cannot remove a move's spool for {op_id}: {error}");
    }
}
