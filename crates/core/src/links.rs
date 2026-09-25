//! A task's links (D-050): the web addresses of its linked resources, then
//! the URLs in its notes, deduplicated. Finding them is `linkify`'s job,
//! not a hand-made pattern; opening one is refused unless it's http,
//! https or mailto; and copying one goes to the terminal as OSC 52, whose
//! payload is base64, so nothing in a link can act as an escape sequence.

use std::collections::HashSet;
use std::ops::Range;

use base64::Engine as _;
use linkify::{LinkFinder, LinkKind};
use serde_json::{Map, Value};
use url::Url;

/// Where a link was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkSource {
    /// Graph's `linkedResources`: the app or email a task came from.
    LinkedResource,
    Notes,
}

impl LinkSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LinkedResource => "linked_resource",
            Self::Notes => "notes",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub url: String,
    /// What to show for it: a linked resource's name, a markdown link's
    /// text, or else the host (or a mailto's address).
    pub text: String,
    pub source: LinkSource,
}

/// Wider than any line, so html notes aren't wrapped mid-URL.
const HTML_WIDTH: usize = 10_000;

/// Every link in `task`, a task entity: its linked resources first, then
/// its notes, each URL once.
pub fn task_links(task: &Map<String, Value>) -> Vec<Link> {
    let linked = linked_resources(task);
    let linked = linked
        .iter()
        .map(|(url, name)| (url.as_str(), name.as_deref()));
    let notes = task.get("body").and_then(|body| {
        let content = body.get("content")?.as_str()?;
        let html = body
            .get("contentType")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind.eq_ignore_ascii_case("html"));
        Some((content, html))
    });
    links(linked, notes)
}

/// A task entity's `linkedResources` as `(webUrl, name)`: the
/// resource's display name, else its app's.
pub fn linked_resources(task: &Map<String, Value>) -> Vec<(String, Option<String>)> {
    task.get("linkedResources")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|resource| {
            let url = resource.get("webUrl")?.as_str()?.to_owned();
            let name = resource
                .get("displayName")
                .or_else(|| resource.get("applicationName"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            Some((url, name))
        })
        .collect()
}

/// [`task_links`] from its parts: `(webUrl, name)` pairs and the notes
/// with whether they're html.
pub fn links<'a>(
    linked: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
    notes: Option<(&str, bool)>,
) -> Vec<Link> {
    let mut found: Vec<Link> = linked
        .into_iter()
        .filter(|(url, _)| !url.trim().is_empty())
        .map(|(url, name)| Link {
            url: url.trim().to_owned(),
            text: name
                .filter(|name| !name.trim().is_empty())
                .map_or_else(|| label(url.trim()), str::to_owned),
            source: LinkSource::LinkedResource,
        })
        .collect();
    if let Some((content, html)) = notes {
        found.extend(notes_links(content, html));
    }
    let mut seen = HashSet::new();
    found.retain(|link| seen.insert(link.url.clone()));
    found
}

fn notes_links(content: &str, html: bool) -> Vec<Link> {
    // html2text lists each anchor's href as a footnote, entities decoded,
    // so the hrefs are found along with the URLs in the text.
    let text = if html {
        html2text::config::plain()
            .string_from_read(content.as_bytes(), HTML_WIDTH)
            .unwrap_or_else(|_| content.to_owned())
    } else {
        content.to_owned()
    };
    finder()
        .links(&text)
        .map(|found| {
            let url = match found.kind() {
                LinkKind::Email => format!("mailto:{}", found.as_str()),
                _ => found.as_str().to_owned(),
            };
            let text =
                markdown_text(&text, found.start()).map_or_else(|| label(&url), str::to_owned);
            Link {
                url,
                text,
                source: LinkSource::Notes,
            }
        })
        .collect()
}

/// Where each URL or email address in `text` is, for drawing them as
/// links.
pub fn find_urls(text: &str) -> Vec<Range<usize>> {
    finder()
        .links(text)
        .map(|found| found.start()..found.end())
        .collect()
}

fn finder() -> LinkFinder {
    let mut finder = LinkFinder::new();
    finder.kinds(&[LinkKind::Url, LinkKind::Email]);
    finder
}

/// A markdown link's text, `[text](url)`, when the URL at `start` is one.
fn markdown_text(text: &str, start: usize) -> Option<&str> {
    let before = text.get(..start)?.strip_suffix("](")?;
    let open = before.rfind('[')?;
    let label = before.get(open + 1..)?;
    (!label.trim().is_empty() && !label.contains(['[', ']', '\n'])).then_some(label)
}

/// A link's host, or a mailto's address, or else the URL itself.
fn label(url: &str) -> String {
    match Url::parse(url) {
        Ok(parsed) if parsed.scheme() == "mailto" => parsed.path().to_owned(),
        Ok(parsed) => parsed.host_str().unwrap_or(url).to_owned(),
        Err(_) => url.to_owned(),
    }
}

/// Why a link isn't opened.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refused {
    #[error("it has a control character in it")]
    Control,
    #[error("it isn't a valid URL")]
    Invalid,
    #[error("only http, https and mailto links are opened, not {0}:")]
    Scheme(String),
}

/// The link as a URL that's safe to hand to the system's opener: parsed,
/// http, https or mailto, and without a control character anywhere.
pub fn openable(url: &str) -> Result<Url, Refused> {
    if url.chars().any(char::is_control) {
        return Err(Refused::Control);
    }
    let parsed = Url::parse(url).map_err(|_| Refused::Invalid)?;
    match parsed.scheme() {
        "http" | "https" | "mailto" => Ok(parsed),
        other => Err(Refused::Scheme(other.to_owned())),
    }
}

/// The OSC 52 sequence that puts `text` on the clipboard, over SSH too.
/// `text` goes in base64, so it can't end the sequence or start another.
pub fn osc52_copy(text: &str) -> String {
    let payload = base64::engine::general_purpose::STANDARD.encode(text);
    format!("\x1b]52;c;{payload}\x07")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn urls(found: &[Link]) -> Vec<&str> {
        found.iter().map(|link| link.url.as_str()).collect()
    }

    #[test]
    fn urls_in_notes_are_found_without_trailing_punctuation() {
        let found = links(
            [],
            Some((
                "See https://example.com/a, then (https://example.org/b). \
                 Mail sam@example.com or read https://en.wikipedia.org/wiki/Rust_(programming_language)!",
                false,
            )),
        );
        assert_eq!(
            urls(&found),
            [
                "https://example.com/a",
                "https://example.org/b",
                "mailto:sam@example.com",
                "https://en.wikipedia.org/wiki/Rust_(programming_language)",
            ]
        );
        assert_eq!(found[0].text, "example.com");
        assert_eq!(found[2].text, "sam@example.com");
        assert!(found.iter().all(|link| link.source == LinkSource::Notes));
    }

    #[test]
    fn a_markdown_link_keeps_its_text() {
        let found = links(
            [],
            Some(("Read [the docs](https://docs.rs/linkify) first", false)),
        );
        assert_eq!(urls(&found), ["https://docs.rs/linkify"]);
        assert_eq!(found[0].text, "the docs");
    }

    #[test]
    fn linked_resources_come_first_and_each_url_once() {
        let task = json!({
            "linkedResources": [
                { "webUrl": "https://mail.example.com/m/1", "displayName": "The email", "applicationName": "Outlook" },
                { "webUrl": "https://app.example.com/x", "applicationName": "App" },
                { "displayName": "no url" }
            ],
            "body": { "content": "https://mail.example.com/m/1 and https://notes.example.com", "contentType": "text" }
        });
        let found = task_links(task.as_object().expect("object"));
        assert_eq!(
            urls(&found),
            [
                "https://mail.example.com/m/1",
                "https://app.example.com/x",
                "https://notes.example.com"
            ]
        );
        assert_eq!(found[0].text, "The email");
        assert_eq!(found[0].source, LinkSource::LinkedResource);
        assert_eq!(found[1].text, "App");
        assert_eq!(found[2].source, LinkSource::Notes);
    }

    #[test]
    fn html_notes_give_their_hrefs_decoded() {
        let task = json!({
            "body": {
                "content": "<html><body><p>Book <a href=\"https://example.com/t?a=1&amp;b=2\">tickets</a></p></body></html>",
                "contentType": "html"
            }
        });
        let found = task_links(task.as_object().expect("object"));
        assert_eq!(urls(&found), ["https://example.com/t?a=1&b=2"]);
    }

    #[test]
    fn only_http_https_and_mailto_open() {
        assert!(openable("https://example.com/a?b=c").is_ok());
        assert!(openable("http://example.com").is_ok());
        assert!(openable("mailto:sam@example.com").is_ok());
        assert_eq!(
            openable("javascript:alert(1)"),
            Err(Refused::Scheme("javascript".into()))
        );
        assert_eq!(
            openable("file:///etc/passwd"),
            Err(Refused::Scheme("file".into()))
        );
        assert_eq!(
            openable("https://example.com/\x1b]52;c;aGk=\x07"),
            Err(Refused::Control)
        );
        assert_eq!(openable("not a url"), Err(Refused::Invalid));
    }

    #[test]
    fn a_copied_link_is_base64_inside_osc_52() {
        let sequence = osc52_copy("https://example.com/\x1b\x07");
        assert_eq!(sequence, "\x1b]52;c;aHR0cHM6Ly9leGFtcGxlLmNvbS8bBw==\x07");
        // Only the sequence's own ESC and BEL are control characters.
        let inner = &sequence[1..sequence.len() - 1];
        assert!(!inner.chars().any(char::is_control), "{sequence:?}");
    }

    #[test]
    fn find_urls_gives_their_places() {
        let text = "go to https://example.com now";
        let ranges = find_urls(text);
        assert_eq!(ranges.len(), 1);
        assert_eq!(&text[ranges[0].clone()], "https://example.com");
    }
}
