//! What goes into `update` and what comes out: messages in, requests for
//! the daemon and work for the runner out, and the tags that pair an
//! answer with its request.

use crossterm::event::KeyEvent;
use ms_todo_protocol::{ErrorPayload, Event, Request, ResponseData};
use ratatui::layout::Size;

use super::Clock;
use super::diagnostics::Part;
use crate::action::Action;

/// Which request an answer belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tag {
    /// A `Seed`, numbered: only the latest one asked for is drawn.
    Seed(u64),
    /// A smart view's seed read ahead, so switching to it paints at once.
    Prefetch,
    Write(Write),
    /// A change to lists' folders.
    Folders,
    /// A list made, renamed or deleted (rung 8e).
    Lists,
    Undo,
    Sync,
    Diagnostics(Part),
    /// The user's Outlook categories, for `@label` (quick add).
    Categories,
    /// A list suggestion for the task being added (rung 6b).
    ListHint,
    /// An attachment saved to open (rung 8b).
    Download,
    /// The active context switched (rung 9d).
    Context,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Write {
    Add,
    Complete,
    Reopen,
    Edit,
    Delete,
    Move,
    /// Into My Day, or out of it.
    MyDay,
    /// Nagging turned on or off.
    Nag,
}

/// What goes into [`App::update`](super::App::update). A seed makes `Response` the big one;
/// messages are moved once each, so boxing it would buy nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    Action(Action),
    /// A character typed into a prompt.
    Char(char),
    /// Any other key in a prompt, for its line editor: a move or a delete.
    Key(KeyEvent),
    /// Connected to the daemon and subscribed.
    Connected,
    Disconnected(String),
    Response {
        tag: Tag,
        result: Result<ResponseData, ErrorPayload>,
    },
    Event(Event),
    Tick(Clock),
    /// The terminal's size, at the start and on every resize.
    Resize(Size),
}

/// What the runner does on this machine rather than ask the daemon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalEffect {
    /// Write the kept theme to config.toml.
    SaveTheme,
    /// Open a link that passed `links::openable`.
    Open(url::Url),
    /// Put a link on the clipboard with OSC 52.
    Copy(String),
}

/// What comes out: a request for the daemon.
#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    pub tag: Tag,
    pub request: Request,
}
