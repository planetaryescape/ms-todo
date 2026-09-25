//! Writing a downloaded attachment to disk, which is a trust boundary
//! (docs/blueprint/03-graph-provider.md, D-033 item 5): the name comes from
//! Microsoft To Do, where any device can set it (S16: Graph keeps
//! `../weird:name?.txt`), so it's made safe before it names a file.
//!
//! - The name loses path separators, control characters and the
//!   characters other systems refuse; leading dots and spaces go, so it
//!   can't be `..` or hidden, and runs of dots become one.
//! - The directory is opened **once**, as a handle, refusing a symlink
//!   (`O_DIRECTORY | O_NOFOLLOW`), and every file is made relative to that
//!   handle (`openat`, `linkat`, `renameat`), so swapping the path for a
//!   link while the bytes download can't redirect a write.
//! - The bytes go to a `.part` file created 0600 and new (`O_EXCL`, never
//!   an existing file or link), synced, then put in place. Without
//!   `force` nothing is ever replaced: a hard link, else a rename that
//!   refuses to replace (`RENAME_NOREPLACE`, `RENAME_EXCL` on macOS),
//!   else a new file made `O_EXCL` with the bytes written again, each of
//!   which fails on a name taken meanwhile. With `force`, a rename, which
//!   replaces a link rather than write through it.
//! - A name taken gets ` (1)`, ` (2)`, … before its extension.

use std::fs::File;
use std::io::{self, ErrorKind, Write};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

#[cfg(any(target_os = "macos", target_os = "linux"))]
use rustix::fs::RenameFlags;
use rustix::fs::{AtFlags, Mode, OFlags};
use rustix::io::Errno;

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

/// The download directory, opened once: every file is made relative to
/// `fd`, and `path` is only for saying where it went.
pub(crate) struct Dir {
    fd: OwnedFd,
    path: PathBuf,
}

impl Dir {
    /// Open `dir`, an absolute path to a directory that isn't a symlink.
    pub fn open(dir: &Path) -> io::Result<Self> {
        if !dir.is_absolute() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                format!("{} isn't an absolute path", dir.display()),
            ));
        }
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let fd = rustix::fs::open(dir, flags, Mode::empty()).map_err(|error| {
            let error = io::Error::from(error);
            let is_link = std::fs::symlink_metadata(dir)
                .is_ok_and(|metadata| metadata.file_type().is_symlink());
            if is_link {
                io::Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "{} is a symlink; name the directory it points to instead",
                        dir.display()
                    ),
                )
            } else if error.raw_os_error() == Some(Errno::NOTDIR.raw_os_error()) {
                io::Error::new(
                    ErrorKind::InvalidInput,
                    format!("{} isn't a directory", dir.display()),
                )
            } else {
                error
            }
        })?;
        // For the answer only: the files go through `fd`.
        let path = dir.canonicalize().unwrap_or_else(|_| dir.to_owned());
        Ok(Self { fd, path })
    }

    /// Write `bytes` as the file `name` (made safe) in this directory.
    /// Returns where it went.
    pub fn write_new(&self, name: &str, bytes: &[u8], force: bool) -> io::Result<PathBuf> {
        self.write_with(name, bytes, force, Place::Link)
    }

    /// [`Dir::write_new`], trying `first` and then the ways after it.
    fn write_with(
        &self,
        name: &str,
        bytes: &[u8],
        force: bool,
        first: Place,
    ) -> io::Result<PathBuf> {
        let name = safe_name(name);
        let part = self.write_part(&name, bytes)?;
        let placed = if force {
            rustix::fs::renameat(&self.fd, &part, &self.fd, &name)
                .map(|()| name.clone())
                .map_err(io::Error::from)
        } else {
            self.place_unique(&name, &part, bytes, first)
        };
        // Gone after a rename; a second name after a link.
        let _ = rustix::fs::unlinkat(&self.fd, &part, AtFlags::empty());
        let placed = placed?;
        let _ = rustix::fs::fsync(&self.fd);
        Ok(self.path.join(placed))
    }

    /// The bytes in a new 0600 `.part` file, synced to disk.
    fn write_part(&self, name: &str, bytes: &[u8]) -> io::Result<String> {
        for _ in 0..100 {
            let unique = uuid::Uuid::new_v4().simple().to_string();
            let part = format!(".{name}.{}.part", &unique[..8]);
            match self.create_new(&part) {
                Ok(file) => {
                    if let Err(error) = write_synced(file, bytes) {
                        let _ = rustix::fs::unlinkat(&self.fd, &part, AtFlags::empty());
                        return Err(error);
                    }
                    return Ok(part);
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            ErrorKind::AlreadyExists,
            "couldn't find a free name for a .part file",
        ))
    }

    /// A new 0600 file `name` in the directory: `O_EXCL`, so it fails on
    /// any existing name, a symlink included.
    fn create_new(&self, name: &str) -> io::Result<File> {
        let flags =
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let fd = rustix::fs::openat(&self.fd, name, flags, Mode::from_raw_mode(0o600))?;
        Ok(File::from(fd))
    }

    /// Give the `.part` file the name `name`, or `name (1)`, `name (2)`,
    /// …, never replacing anything. Returns the name used.
    fn place_unique(
        &self,
        name: &str,
        part: &str,
        bytes: &[u8],
        first: Place,
    ) -> io::Result<String> {
        let (stem, extension) = split_extension(name);
        let mut how = first;
        for number in 0..10_000 {
            let candidate = if number == 0 {
                name.to_owned()
            } else {
                format!("{stem} ({number}){extension}")
            };
            loop {
                match self.place(how, part, &candidate, bytes) {
                    Ok(()) => return Ok(candidate),
                    Err(error) if error.kind() == ErrorKind::AlreadyExists => break,
                    // This filesystem can't do it that way: the next way.
                    Err(error) => match how.next() {
                        Some(next) => how = next,
                        None => return Err(error),
                    },
                }
            }
        }
        Err(io::Error::new(
            ErrorKind::AlreadyExists,
            format!("{name} and 10,000 numbered copies of it exist already"),
        ))
    }

    fn place(&self, how: Place, part: &str, name: &str, bytes: &[u8]) -> io::Result<()> {
        match how {
            Place::Link => {
                rustix::fs::linkat(&self.fd, part, &self.fd, name, AtFlags::empty())?;
            }
            Place::RenameNoReplace => rename_no_replace(&self.fd, part, name)?,
            Place::CreateNew => write_synced(self.create_new(name)?, bytes)?,
        }
        Ok(())
    }
}

/// The ways to give a `.part` file its name without replacing one, in the
/// order tried: each fails on a name taken meanwhile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    Link,
    RenameNoReplace,
    /// Where neither works (a filesystem without hard links that can't
    /// refuse a rename either): a new file, with the bytes again.
    CreateNew,
}

impl Place {
    fn next(self) -> Option<Self> {
        match self {
            Self::Link => Some(Self::RenameNoReplace),
            Self::RenameNoReplace => Some(Self::CreateNew),
            Self::CreateNew => None,
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn rename_no_replace(dir: &OwnedFd, from: &str, to: &str) -> io::Result<()> {
    rustix::fs::renameat_with(dir, from, dir, to, RenameFlags::NOREPLACE).map_err(io::Error::from)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn rename_no_replace(_: &OwnedFd, _: &str, _: &str) -> io::Result<()> {
    Err(io::Error::from(ErrorKind::Unsupported))
}

fn write_synced(mut file: File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)?;
    file.sync_all()
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

    fn parts(root: &Path) -> usize {
        std::fs::read_dir(root)
            .expect("list")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".part"))
            .count()
    }

    #[test]
    fn a_taken_name_is_numbered_and_nothing_is_overwritten() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = Dir::open(dir.path()).expect("dir");
        let root = out.path.clone();
        std::fs::write(root.join("a.pdf"), b"mine").expect("write");
        let first = out.write_new("a.pdf", b"one", false).expect("write");
        let second = out.write_new("a.pdf", b"two", false).expect("write");
        assert_eq!(first, root.join("a (1).pdf"));
        assert_eq!(second, root.join("a (2).pdf"));
        assert_eq!(std::fs::read(root.join("a.pdf")).expect("read"), b"mine");
        assert_eq!(std::fs::read(&second).expect("read"), b"two");
        let mode = std::fs::metadata(&first)
            .expect("stat")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(parts(&root), 0, "no .part left behind");
    }

    #[test]
    fn every_fallback_numbers_rather_than_overwrites() {
        for first in [Place::RenameNoReplace, Place::CreateNew] {
            let dir = tempfile::tempdir().expect("tempdir");
            let out = Dir::open(dir.path()).expect("dir");
            let root = out.path.clone();
            std::fs::write(root.join("a.pdf"), b"mine").expect("write");
            let elsewhere = tempfile::tempdir().expect("tempdir");
            let victim = elsewhere.path().join("victim");
            std::fs::write(&victim, b"keep").expect("write");
            std::os::unix::fs::symlink(&victim, root.join("a (1).pdf")).expect("link");
            let written = out
                .write_with("a.pdf", b"new", false, first)
                .expect("write");
            assert_eq!(written, root.join("a (2).pdf"), "{first:?}");
            assert_eq!(std::fs::read(&written).expect("read"), b"new");
            assert_eq!(std::fs::read(root.join("a.pdf")).expect("read"), b"mine");
            assert_eq!(std::fs::read(&victim).expect("read"), b"keep", "{first:?}");
            let mode = std::fs::metadata(&written)
                .expect("stat")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
            assert_eq!(parts(&root), 0, "{first:?}");
        }
    }

    #[test]
    fn a_directory_swapped_for_a_link_after_opening_gets_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("out");
        std::fs::create_dir(&path).expect("mkdir");
        let out = Dir::open(&path).expect("dir");
        // While the bytes download: the directory moves away, and a link
        // to somewhere else takes its name.
        let moved = dir.path().join("moved");
        std::fs::rename(&path, &moved).expect("move");
        let elsewhere = tempfile::tempdir().expect("tempdir");
        std::os::unix::fs::symlink(elsewhere.path(), &path).expect("link");
        out.write_new("a.txt", b"bytes", false).expect("write");
        out.write_new("b.txt", b"bytes", true).expect("write");
        assert_eq!(std::fs::read(moved.join("a.txt")).expect("read"), b"bytes");
        assert_eq!(std::fs::read(moved.join("b.txt")).expect("read"), b"bytes");
        assert_eq!(
            std::fs::read_dir(elsewhere.path()).expect("list").count(),
            0,
            "nothing written through the link"
        );
    }

    #[test]
    fn force_replaces_a_file_but_never_writes_through_a_link() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = Dir::open(dir.path()).expect("dir");
        let root = out.path.clone();
        let elsewhere = tempfile::tempdir().expect("tempdir");
        let victim = elsewhere.path().join("victim");
        std::fs::write(&victim, b"keep").expect("write");
        std::os::unix::fs::symlink(&victim, root.join("a.txt")).expect("link");
        let written = out.write_new("a.txt", b"new", true).expect("write");
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
        let numbered = out.write_new("b.txt", b"b", false).expect("write");
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
        let refused = Dir::open(&link).err().expect("a symlink");
        assert!(refused.to_string().contains("symlink"), "{refused}");
        assert!(Dir::open(Path::new("relative/dir")).is_err());
        std::fs::write(dir.path().join("file"), b"x").expect("write");
        let file = Dir::open(&dir.path().join("file")).err().expect("a file");
        assert!(file.to_string().contains("isn't a directory"), "{file}");
        assert!(Dir::open(&dir.path().join("missing")).is_err());
    }
}
