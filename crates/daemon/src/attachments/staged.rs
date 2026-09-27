//! Files attached from the daemon's own copy (D-067): what `attachments
//! add T -` read from stdin (the CLI saves it to a temporary file and asks
//! for a copy), and what `attachments add T https://…` downloads. Either
//! is kept here until its operation is done, so the upload, a retry after
//! a daemon restart, or `outbox retry` reads bytes that can't change or
//! vanish under it: 0600, in a 0700 directory under the instance's data
//! directory, named at random.
//!
//! A download is `https` only, and follows a redirect only to `https` and
//! never to this machine or its network (`public_only`); it stops past
//! 25 MB, whatever the server said it would send.
//!
//! [`sweep`] removes a copy once nothing waiting to be sent reads it (its
//! operation done or discarded), and any copy past a week.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use ms_todo_graph::MAX_ATTACHMENT_BYTES;
use ms_todo_graph::private_file::{atomic_write_mode_0600, ensure_private_dir};
use reqwest::Url;

use super::public_only::{PublicOnly, redirect_allowed};
use crate::handlers::State;

/// How long a copy is kept when no operation reads it: long enough that a
/// copy made for a command still being queued is never taken.
const UNCLAIMED_FOR: Duration = Duration::from_secs(10 * 60);

/// The most redirects a download follows.
const MAX_REDIRECTS: usize = 5;

/// Debug builds only: let a download use `http` to this machine, for the
/// tests' local server. Release builds always need `https`.
const LOOPBACK_HTTP_ENV: &str = "MS_TODO_DOWNLOAD_LOOPBACK_HTTP";

/// A file downloaded here: where it is, and the name and type its URL
/// and answer suggest.
pub(crate) struct Downloaded {
    pub path: PathBuf,
    pub name: String,
    pub content_type: Option<String>,
}

/// Copy `source` (read whole: attachments are 25 MB at most) into `root`.
pub(crate) async fn copy_in(root: &Path, source: &Path) -> Result<PathBuf, String> {
    let (root, source) = (root.to_owned(), source.to_owned());
    tokio::task::spawn_blocking(move || {
        let bytes = std::fs::read(&source)
            .map_err(|error| format!("cannot read {}: {error}", source.display()))?;
        keep(&root, &bytes).map_err(|error| format!("cannot keep a copy of it: {error}"))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Check `url` is one a download may start from, before anything is
/// sent: `https` (or, in a debug build that allows it, `http` to this
/// machine).
pub(crate) fn check_url(url: &str) -> Result<Url, String> {
    let parsed = Url::parse(url).map_err(|error| format!("{url:?} isn't a URL: {error}"))?;
    if allowed(&parsed) {
        Ok(parsed)
    } else {
        Err(format!(
            "only https URLs are downloaded, so {url:?} wasn't; save it yourself and attach the \
             file"
        ))
    }
}

fn allowed(url: &Url) -> bool {
    url.scheme() == "https"
        || (cfg!(debug_assertions)
            && std::env::var_os(LOOPBACK_HTTP_ENV).is_some()
            && url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "localhost")))
}

/// The name a URL suggests: its path's last segment, decoded, or
/// `attachment`.
pub(crate) fn name_of(url: &Url) -> String {
    url.path_segments()
        .and_then(|mut segments| segments.rfind(|segment| !segment.is_empty()))
        .and_then(|segment| {
            percent_encoding::percent_decode_str(segment)
                .decode_utf8()
                .ok()
                .map(std::borrow::Cow::into_owned)
                .filter(|name| !name.trim().is_empty())
        })
        .unwrap_or_else(|| "attachment".to_owned())
}

/// The client for one leg of a download: redirects followed by hand, no
/// proxy (a proxy would resolve the name itself, past [`PublicOnly`]),
/// and the resolver [`PublicOnly`], `trusted` for the first request.
fn download_client(trusted: bool) -> Result<reqwest::Client, String> {
    crate::semantic::model::http_client_builder()
        .no_proxy()
        .dns_resolver(PublicOnly { trusted })
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("can't make an HTTP client: {error}"))
}

/// Download `url` into `root`: 25 MB at most, and every redirect to
/// `https` too.
pub(crate) async fn download(root: &Path, url: Url) -> Result<Downloaded, String> {
    let failed = |error: reqwest::Error| {
        format!(
            "downloading {url} failed: {}",
            ms_todo_core::message_with_causes(&error)
        )
    };
    // The first request may reach the host typed; each redirect is
    // requested by a client that reaches only public addresses (D-067).
    let first = download_client(true)?;
    let redirects = download_client(false)?;
    let mut at = url.clone();
    let mut response = first.get(at.clone()).send().await.map_err(failed)?;
    let mut hops = 0;
    while response.status().is_redirection() {
        let Some(next) = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|location| location.to_str().ok())
            .and_then(|location| at.join(location).ok())
        else {
            break;
        };
        hops += 1;
        if hops > MAX_REDIRECTS {
            return Err(format!("downloading {url} failed: too many redirects"));
        }
        if !allowed(&next) {
            return Err(format!(
                "downloading {url} failed: it redirects to a URL that isn't https"
            ));
        }
        if !redirect_allowed(&next) {
            return Err(format!(
                "downloading {url} failed: it redirects to this machine or its network"
            ));
        }
        response = redirects.get(next.clone()).send().await.map_err(failed)?;
        at = next;
    }
    let mut response = response.error_for_status().map_err(failed)?;
    let too_big = || {
        format!(
            "{url} is over 25 MB ({MAX_ATTACHMENT_BYTES} bytes), the most Microsoft To Do takes"
        )
    };
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ATTACHMENT_BYTES as u64)
    {
        return Err(too_big());
    }
    let name = name_of(response.url());
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(|essence| essence.trim().to_owned())
        .filter(|essence| !essence.is_empty());
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(failed)? {
        bytes.extend_from_slice(&chunk);
        // The length a server announces is only a claim.
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(too_big());
        }
    }
    let root = root.to_owned();
    let path = tokio::task::spawn_blocking(move || keep(&root, &bytes))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("cannot keep the download: {error}"))?;
    Ok(Downloaded {
        path,
        name,
        content_type,
    })
}

fn keep(root: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    ensure_private_dir(root)?;
    let path = root.join(uuid::Uuid::new_v4().simple().to_string());
    atomic_write_mode_0600(&path, bytes)?;
    Ok(path)
}

/// Remove each copy no unfinished operation reads, once it's had time to
/// be claimed. Run when the daemon starts: a copy sent is removed then and
/// there, so only a discarded add's, or a done one's the daemon stopped
/// before removing, waits for this.
pub(crate) async fn sweep(state: &State) {
    let in_use = match state.store.staged_files_in_use().await {
        Ok(paths) => paths.into_iter().map(PathBuf::from).collect(),
        Err(error) => {
            eprintln!("ms-todo daemon: cannot tell which staged attachments are in use: {error}");
            return;
        }
    };
    let root = state.staged_dir.clone();
    let swept =
        tokio::task::spawn_blocking(move || sweep_now(&root, &in_use, SystemTime::now())).await;
    if let Ok(Err(error)) = swept {
        eprintln!("ms-todo daemon: cannot tidy the staged attachments: {error}");
    }
}

fn sweep_now(root: &Path, in_use: &HashSet<PathBuf>, now: SystemTime) -> io::Result<()> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        let age = now.duration_since(metadata.modified()?).unwrap_or_default();
        // One an unfinished add reads is kept whatever its age: without
        // it, a retry would have nothing to send.
        if !in_use.contains(&entry.path()) && age > UNCLAIMED_FOR {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_is_downloaded() {
        assert!(check_url("https://example.com/a.pdf").is_ok());
        let refused = check_url("http://example.com/a.pdf").expect_err("http");
        assert!(refused.contains("only https"), "{refused}");
        assert!(check_url("file:///etc/passwd").is_err());
        assert!(check_url("not a url").is_err());
    }

    #[test]
    fn the_name_is_the_last_segment_decoded() {
        let name = |url: &str| name_of(&Url::parse(url).expect("url"));
        assert_eq!(
            name("https://e.com/files/Q3%20report.pdf?x=1"),
            "Q3 report.pdf"
        );
        assert_eq!(name("https://e.com/files/"), "files");
        assert_eq!(name("https://e.com"), "attachment");
        assert_eq!(name("https://e.com/%FF"), "attachment", "not UTF-8");
    }

    #[test]
    fn a_copy_goes_once_nothing_reads_it_and_has_had_time_to_be_claimed() {
        let root = tempfile::tempdir().expect("tempdir");
        let claimed = keep(root.path(), b"claimed").expect("keep");
        let unclaimed = keep(root.path(), b"unclaimed").expect("keep");
        let in_use = HashSet::from([claimed.clone()]);
        sweep_now(root.path(), &in_use, SystemTime::now()).expect("sweep");
        assert!(unclaimed.exists(), "too new to be unclaimed");
        let later = SystemTime::now() + UNCLAIMED_FOR + Duration::from_secs(1);
        sweep_now(root.path(), &in_use, later).expect("sweep");
        assert!(!unclaimed.exists());
        assert!(claimed.exists(), "an operation reads it");
        let years = SystemTime::now() + Duration::from_secs(3 * 365 * 24 * 60 * 60);
        sweep_now(root.path(), &in_use, years).expect("sweep");
        assert!(claimed.exists(), "never while an operation reads it");
    }
}
