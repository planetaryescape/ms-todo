//! Writing a downloaded attachment to disk, which is a trust boundary
//! (docs/blueprint/03-graph-provider.md, D-033 item 5): the name comes from
//! Microsoft To Do, where any device can set it (S16: Graph keeps
//! `../weird:name?.txt`), so it's made safe before it names a file.
//!
//! - The name loses path separators, control characters and the
//!   characters other systems refuse; leading dots and spaces go, so it
//!   can't be `..` or hidden, and runs of dots become one.
//! - The directory must be a real directory, not a symlink, and the file
//!   is created inside it, never through a symlink.
//! - The bytes go to a `.part` file created 0600 and new (never an
//!   existing file or link), synced, then put in place: without `force`
//!   by a hard link, which fails rather than replace a file that appeared
//!   meanwhile, so a file already there is never overwritten (where the
//!   filesystem has no hard links, a check then a rename); with it, by a
//!   rename, which replaces a link rather than write through it.
//! - A name taken gets ` (1)`, ` (2)`, … before its extension.

use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// The longest name written, in bytes: most filesystems take 255, and the
/// `.part` suffix and a counter need room.
const MAX_NAME_BYTES: usize = 200;

/// What stands in for a name with nothing safe left in it.
const FALLBACK_NAME: &str = "attachment";

/// Characters never kept in a file name: separators, and what Windows,
/// SMB shares and shells treat specially.
const UNSAFE: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// `name` as a file name that can only mean a file in the directory.
pub(crate) fn safe_name(name: &str) -> String {
    let mut safe = String::with_capacity(name.len());
    for ch in name.chars() {
        let ch = if ch.is_control() || UNSAFE.contains(&ch) {
            '_'
        } else {
            ch
        };
        // `..` anywhere is harmless without a separator, but one dot is
        // all a name needs.
        if ch == '.' && safe.ends_with('.') {
            continue;
        }
        safe.push(ch);
    }
    let trimmed = safe
        .trim_start_matches(['.', ' '])
        .trim_end_matches([' ', '.']);
    let trimmed = truncate(trimmed, MAX_NAME_BYTES);
    if trimmed.is_empty() {
        FALLBACK_NAME.to_owned()
    } else {
        trimmed
    }
}

/// `name` cut to at most `max` bytes on a character boundary, keeping its
/// extension when that's short.
fn truncate(name: &str, max: usize) -> String {
    if name.len() <= max {
        return name.to_owned();
    }
    let (stem, extension) = split_extension(name);
    let extension = if extension.len() <= 16 { extension } else { "" };
    let mut end = max.saturating_sub(extension.len());
    while !stem.is_char_boundary(end.min(stem.len())) {
        end -= 1;
    }
    format!("{}{extension}", &stem[..end.min(stem.len())])
}

/// `("report", ".pdf")` for `report.pdf`; a name with no dot, or only a
/// leading one, has no extension.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(at) if at > 0 => name.split_at(at),
        _ => (name, ""),
    }
}

/// `dir`, checked to be a directory that isn't a symlink, as its real
/// path.
pub(crate) fn real_dir(dir: &Path) -> io::Result<PathBuf> {
    if !dir.is_absolute() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("{} isn't an absolute path", dir.display()),
        ));
    }
    let metadata = std::fs::symlink_metadata(dir)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "{} is a symlink; name the directory it points to instead",
                dir.display()
            ),
        ));
    }
    if !metadata.is_dir() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("{} isn't a directory", dir.display()),
        ));
    }
    dir.canonicalize()
}

/// Write `bytes` as the file `name` (made safe) in `dir`, which
/// [`real_dir`] checked. Returns where it went.
pub(crate) fn write_new(dir: &Path, name: &str, bytes: &[u8], force: bool) -> io::Result<PathBuf> {
    let name = safe_name(name);
    let part = write_part(dir, &name, bytes)?;
    let placed = if force {
        let target = dir.join(&name);
        std::fs::rename(&part, &target).map(|()| target)
    } else {
        link_unique(dir, &name, &part)
    };
    // The part is gone after a rename, and only a name after a link.
    let _ = std::fs::remove_file(&part);
    let placed = placed?;
    sync_dir(dir);
    Ok(placed)
}

/// The bytes in a new 0600 `.part` file in `dir`, synced to disk.
fn write_part(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    for _ in 0..100 {
        let unique = uuid::Uuid::new_v4().simple().to_string();
        let part = dir.join(format!(".{name}.{}.part", &unique[..8]));
        // `create_new` is O_CREAT | O_EXCL, which fails on any existing
        // name, a symlink included (POSIX), so nothing is written through
        // a link planted there.
        let opened = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&part);
        let mut file = match opened {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let written = file.write_all(bytes).and_then(|()| file.sync_all());
        if let Err(error) = written {
            let _ = std::fs::remove_file(&part);
            return Err(error);
        }
        return Ok(part);
    }
    Err(io::Error::new(
        ErrorKind::AlreadyExists,
        "couldn't find a free name for a .part file",
    ))
}

/// Give `part` the name `name`, or `name (1)`, `name (2)`, … when it's
/// taken, never replacing anything.
fn link_unique(dir: &Path, name: &str, part: &Path) -> io::Result<PathBuf> {
    let (stem, extension) = split_extension(name);
    for number in 0..10_000 {
        let candidate = if number == 0 {
            name.to_owned()
        } else {
            format!("{stem} ({number}){extension}")
        };
        let target = dir.join(candidate);
        match std::fs::hard_link(part, &target) {
            Ok(()) => return Ok(target),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            // A filesystem without hard links (exFAT, some network
            // shares): check, then rename. A file made in between could
            // be replaced, which only the link rules out.
            Err(_) => match std::fs::symlink_metadata(&target) {
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    std::fs::rename(part, &target)?;
                    return Ok(target);
                }
                Err(error) => return Err(error),
            },
        }
    }
    Err(io::Error::new(
        ErrorKind::AlreadyExists,
        format!("{name} and 10,000 numbered copies of it exist already"),
    ))
}

/// Make the new name durable. Best effort: the file itself is synced.
fn sync_dir(dir: &Path) {
    if let Ok(dir) = File::open(dir) {
        let _ = dir.sync_all();
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn a_name_can_only_mean_a_file_in_the_directory() {
        assert_eq!(safe_name("invoice.pdf"), "invoice.pdf");
        assert_eq!(safe_name("../../etc/passwd"), "_._etc_passwd");
        assert_eq!(safe_name(".."), "attachment");
        assert_eq!(safe_name("..."), "attachment");
        assert_eq!(safe_name(".bashrc"), "bashrc");
        assert_eq!(safe_name("s16 ../weird:name?.txt"), "s16 ._weird_name_.txt");
        assert_eq!(safe_name("a\u{0}b\nc.txt"), "a_b_c.txt");
        assert_eq!(safe_name("C:\\Users\\x.doc"), "C__Users_x.doc");
        assert_eq!(safe_name("Q3 résumé.pdf"), "Q3 résumé.pdf");
        assert_eq!(safe_name("  "), "attachment");
        let long = format!("{}.pdf", "é".repeat(300));
        let cut = safe_name(&long);
        assert!(cut.len() <= MAX_NAME_BYTES, "{}", cut.len());
        assert!(cut.ends_with(".pdf"));
    }

    #[test]
    fn a_taken_name_is_numbered_and_nothing_is_overwritten() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = real_dir(dir.path()).expect("dir");
        std::fs::write(root.join("a.pdf"), b"mine").expect("write");
        let first = write_new(&root, "a.pdf", b"one", false).expect("write");
        let second = write_new(&root, "a.pdf", b"two", false).expect("write");
        assert_eq!(first, root.join("a (1).pdf"));
        assert_eq!(second, root.join("a (2).pdf"));
        assert_eq!(std::fs::read(root.join("a.pdf")).expect("read"), b"mine");
        assert_eq!(std::fs::read(&second).expect("read"), b"two");
        let mode = std::fs::metadata(&first)
            .expect("stat")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let left: Vec<_> = std::fs::read_dir(&root)
            .expect("list")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".part"))
            .collect();
        assert!(left.is_empty(), "no .part left behind");
    }

    #[test]
    fn force_replaces_a_file_but_never_writes_through_a_link() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = real_dir(dir.path()).expect("dir");
        let elsewhere = tempfile::tempdir().expect("tempdir");
        let victim = elsewhere.path().join("victim");
        std::fs::write(&victim, b"keep").expect("write");
        std::os::unix::fs::symlink(&victim, root.join("a.txt")).expect("link");
        let written = write_new(&root, "a.txt", b"new", true).expect("write");
        assert_eq!(written, root.join("a.txt"));
        assert_eq!(std::fs::read(&victim).expect("read"), b"keep");
        assert!(
            !std::fs::symlink_metadata(&written)
                .expect("stat")
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&written).expect("read"), b"new");
        // Without force, a link at the name counts as taken.
        std::os::unix::fs::symlink(&victim, root.join("b.txt")).expect("link");
        let numbered = write_new(&root, "b.txt", b"b", false).expect("write");
        assert_eq!(numbered, root.join("b (1).txt"));
        assert_eq!(std::fs::read(&victim).expect("read"), b"keep");
    }

    #[test]
    fn a_symlinked_or_relative_directory_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("real");
        std::fs::create_dir(&real).expect("mkdir");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("link");
        let refused = real_dir(&link).expect_err("a symlink");
        assert!(refused.to_string().contains("symlink"), "{refused}");
        assert!(real_dir(Path::new("relative/dir")).is_err());
        std::fs::write(dir.path().join("file"), b"x").expect("write");
        assert!(real_dir(&dir.path().join("file")).is_err());
        assert!(real_dir(&dir.path().join("missing")).is_err());
    }
}
