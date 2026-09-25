//! ms-todo's terminal UI (docs/blueprint/08-tui.md): a sidebar of smart
//! views and lists, the task list and a detail pane, driven from the
//! keyboard. It talks to the daemon over the protocol only, never to the
//! store or Graph (D-031): it starts from the daemon's `Seed` and follows
//! its events.
//!
//! - `app`: the state and `update`, a pure function over messages.
//! - `ui`: pure render functions of the state.
//! - `runner`: the event loop over keys, the daemon and a tick.

// Answers carry the wire type, `ErrorPayload`, which is over clippy's 128
// bytes. It's the cold path, once per request, as in the daemon and CLI.
#![allow(clippy::result_large_err)]

mod action;
mod app;
mod glyphs;
mod ipc;
mod keybindings;
mod latency;
pub mod open;
mod runner;
pub mod theme;
mod ui;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{Event as TermEvent, EventStream, KeyCode, KeyEvent, KeyModifiers};
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use tokio::sync::{mpsc, oneshot};

pub use runner::RunError;

pub struct Options {
    /// The daemon's socket; the caller has made sure it's running.
    pub socket: PathBuf,
    /// Plain ASCII instead of Unicode symbols.
    pub ascii: bool,
    /// The theme, from [`theme::load`].
    pub theme: theme::ThemeChoice,
    /// Measure the start and a scripted run of keys, then quit and print
    /// the numbers.
    pub bench_startup: bool,
    /// When the process started, for the cold-start measurement.
    pub started: Instant,
    /// Write tracing spans and events, the latency measurements among
    /// them, to this file.
    pub trace: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum TuiError {
    #[error("the TUI needs a terminal; use the `ms-todo` commands in scripts")]
    NotATerminal,
    #[error("cannot open the trace file {path}: {source}")]
    Trace {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Run(#[from] RunError),
}

/// Run the TUI until the user quits. With `bench_startup`, returns the
/// measurements to print.
pub async fn run(options: Options) -> Result<Option<String>, TuiError> {
    if !std::io::stdout().is_terminal() {
        return Err(TuiError::NotATerminal);
    }
    if let Some(path) = &options.trace {
        trace_to(path)?;
    }
    let glyphs = if options.ascii {
        glyphs::ASCII
    } else {
        glyphs::UNICODE
    };
    let app = app::App::new(glyphs, app::Clock::now()).with_theme(options.theme);
    // The date rules' regexes compile on first use, about 3 ms: do it
    // now, off the render path, not in the first frame of a date editor.
    let now = app.parse_context();
    std::thread::spawn(move || ms_todo_nlp::read_when("today", &now));
    let (link, daemon) = ipc::connect(options.socket.clone());
    // Restores the terminal on a panic too.
    let mut terminal = ratatui::init();
    // With --bench-startup, scripted keys stand in for the keyboard.
    let (input, painted): (BoxStream<'static, std::io::Result<TermEvent>>, _) =
        if options.bench_startup {
            let (painted, first_paint) = oneshot::channel();
            let (keys, scripted) = mpsc::unbounded_channel();
            tokio::spawn(bench_script(first_paint, keys));
            let input = futures_util::stream::unfold(scripted, |mut keys| async move {
                keys.recv().await.map(|key| (Ok(key), keys))
            });
            (input.boxed(), Some(painted))
        } else {
            (EventStream::new().boxed(), None)
        };
    // The terminal's own title goes on its title stack (XTWINOPS 22), to be
    // put back on exit; where that isn't supported, it's cleared instead.
    let _ = write_to_terminal(TITLE_PUSH);
    let result = runner::run_loop(
        &mut terminal,
        input,
        link,
        daemon,
        app,
        options.started,
        painted,
        |title| {
            let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::SetTitle(title));
        },
    )
    .await;
    ratatui::restore();
    let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::SetTitle(""));
    let _ = write_to_terminal(TITLE_POP);
    let latency = result?;
    Ok(options.bench_startup.then(|| latency.report()))
}

/// XTWINOPS: save the window title on the terminal's stack, and restore it.
const TITLE_PUSH: &str = "\x1b[22;2t";
const TITLE_POP: &str = "\x1b[23;2t";

fn write_to_terminal(sequence: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut stdout = std::io::stdout();
    stdout.write_all(sequence.as_bytes())?;
    stdout.flush()
}

/// Once the first list is painted: move through the tasks, across to the
/// sidebar and through the views and back; open the title's editor, move
/// and type in it and cancel; filter and cancel; then quit. Nothing is
/// written. Spaced so each view's seed lands before the next key.
async fn bench_script(first_paint: oneshot::Receiver<()>, keys: mpsc::UnboundedSender<TermEvent>) {
    if first_paint.await.is_err() {
        return;
    }
    let key = |code, modifiers| TermEvent::Key(KeyEvent::new(code, modifiers));
    let none = KeyModifiers::NONE;
    let typed = |text: &str| -> Vec<TermEvent> {
        text.chars()
            .map(|ch| key(KeyCode::Char(ch), KeyModifiers::NONE))
            .collect()
    };
    let mut script = typed("jjjjjjjjjjkkkkkkkkkkGghkkkkkjjjjjl");
    script.extend(typed("et"));
    script.extend([
        key(KeyCode::Left, none),
        key(KeyCode::Left, none),
        key(KeyCode::Home, none),
        key(KeyCode::Char('f'), KeyModifiers::ALT),
        key(KeyCode::End, none),
    ]);
    script.extend(typed("xy"));
    script.extend([
        key(KeyCode::Char('w'), KeyModifiers::CONTROL),
        key(KeyCode::Esc, none),
    ]);
    script.extend(typed("/a"));
    script.extend([key(KeyCode::Backspace, none), key(KeyCode::Esc, none)]);
    script.extend(typed("q"));
    for event in script {
        tokio::time::sleep(Duration::from_millis(40)).await;
        if keys.send(event).is_err() {
            return;
        }
    }
}

fn trace_to(path: &std::path::Path) -> Result<(), TuiError> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|source| TuiError::Trace {
            path: path.to_owned(),
            source,
        })?;
    let _ = tracing_subscriber::fmt()
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
        .try_init();
    Ok(())
}

/// The event loop with its parts supplied, for tests: a terminal backend,
/// a key stream and the daemon's socket.
#[doc(hidden)]
pub mod testing {
    pub use crate::app::{App, Clock, Msg};
    pub use crate::glyphs::UNICODE;
    pub use crate::ipc::connect;
    pub use crate::runner::run_loop;
}
