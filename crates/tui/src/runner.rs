// The event loop is adapted from mxr crates/tui/src/runner.rs @ dfb23d10138b1cfc24f8ea7450d3426e5e4da37a
// (`run`: a `tokio::select!` over the crossterm `EventStream`, daemon
// results and a tick, applying each to the app and redrawing). Changes:
// the loop is generic over the terminal backend and the key stream, so
// tests drive it with `TestBackend` and scripted keys; and it measures the
// latency budget as it goes.

//! The TUI's event loop: keys, the daemon's answers and events, and a
//! tick go into `App::update`; its effects go to the daemon; every change
//! is drawn at once.

use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::{Stream, StreamExt};
use ratatui::Terminal;
use ratatui::backend::Backend;
use tokio::sync::{mpsc, oneshot};

use crate::app::{App, Clock, Level, LocalEffect, Msg, Tag};
use crate::ipc::DaemonLink;
use crate::keybindings::{Context, resolve};
use crate::latency::Latency;
use crate::open::{Opener, SystemOpener};

const TICK: Duration = Duration::from_millis(250);

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("drawing failed: {0}")]
    Draw(String),
    #[error("reading the keyboard failed: {0}")]
    Input(#[from] io::Error),
}

/// Run until the user quits or `input` ends. `first_paint` hears once the
/// first seeded list is on screen; `set_title` gets the window title each
/// time it changes. Returns what was measured.
#[allow(clippy::too_many_arguments)]
pub async fn run_loop<B, S>(
    terminal: &mut Terminal<B>,
    mut input: S,
    link: DaemonLink,
    mut daemon: mpsc::UnboundedReceiver<Msg>,
    mut app: App,
    started: Instant,
    mut first_paint: Option<oneshot::Sender<()>>,
    mut set_title: impl FnMut(&str),
) -> Result<Latency, RunError>
where
    B: Backend,
    S: Stream<Item = io::Result<TermEvent>> + Unpin,
{
    let mut title = String::new();
    // Every frame goes through here, so the window title never lags the
    // view; setting it is a write only when the view changed.
    let mut paint = |terminal: &mut Terminal<B>, app: &App| -> Result<(), RunError> {
        draw(terminal, app)?;
        let now = app.window_title();
        if now != title {
            set_title(&now);
            title = now;
        }
        Ok(())
    };
    let mut latency = Latency::default();
    // When the key behind a request was pressed, for the requests whose
    // answer finishes a measurement.
    let mut pressed: HashMap<Tag, Instant> = HashMap::new();
    let mut ticks = tokio::time::interval(TICK);
    let size = terminal
        .size()
        .map_err(|error| RunError::Draw(error.to_string()))?;
    app.update(Msg::Resize(size));
    paint(terminal, &app)?;
    loop {
        tokio::select! {
            event = input.next() => {
                let Some(event) = event else {
                    break;
                };
                let at = Instant::now();
                match event? {
                    TermEvent::Key(key) => {
                        let span = tracing::info_span!("keypress_to_render", key = ?key.code);
                        let _entered = span.enter();
                        if let Some(msg) = key_msg(&app, &key) {
                            for effect in app.update(msg) {
                                let cached = matches!(effect.tag, Tag::Seed(_)) && app.painted_from_cache;
                                if matches!(effect.tag, Tag::Seed(_) | Tag::Write(_)) && !cached {
                                    pressed.insert(effect.tag, at);
                                }
                                link.send(effect);
                            }
                        }
                        paint(terminal, &app)?;
                        let took = at.elapsed();
                        tracing::info!(micros = took.as_micros(), "keypress rendered");
                        latency.keypress.push(took);
                        // After the frame, so the picker closes at once and
                        // the file's I/O isn't counted as the keypress.
                        if let Some(effect) = app.local.take() {
                            local(&mut app, effect);
                            paint(terminal, &app)?;
                        }
                        if std::mem::take(&mut app.painted_from_cache) {
                            tracing::info!(micros = took.as_micros(), "view switch rendered");
                            latency.view_switch.push(took);
                        }
                    }
                    TermEvent::Resize(width, height) => {
                        app.update(Msg::Resize(ratatui::layout::Size::new(width, height)));
                        paint(terminal, &app)?;
                    }
                    _ => {}
                }
            }
            msg = daemon.recv() => {
                let Some(msg) = msg else {
                    break;
                };
                // Take everything that's arrived, then draw once.
                let mut finished = Vec::new();
                let mut next = Some(msg);
                while let Some(msg) = next {
                    if let Msg::Response { tag, .. } = &msg
                        && let Some(at) = pressed.remove(tag)
                    {
                        finished.push((*tag, at));
                    }
                    for effect in app.update(msg) {
                        link.send(effect);
                    }
                    next = daemon.try_recv().ok();
                }
                paint(terminal, &app)?;
                // An answer can leave something to do here too: open an
                // attachment the daemon saved.
                if let Some(effect) = app.local.take() {
                    local(&mut app, effect);
                    paint(terminal, &app)?;
                }
                for (tag, at) in finished {
                    let took = at.elapsed();
                    match tag {
                        Tag::Seed(_) | Tag::Prefetch => {
                            tracing::info!(micros = took.as_micros(), "view switch rendered");
                            latency.view_switch.push(took);
                        }
                        _ => {
                            tracing::info!(micros = took.as_micros(), "write rendered");
                            latency.write.push(took);
                        }
                    }
                }
                if app.seeded && latency.cold_start.is_none() {
                    let took = started.elapsed();
                    tracing::info!(micros = took.as_micros(), "cold start to first painted list");
                    latency.cold_start = Some(took);
                    if let Some(painted) = first_paint.take() {
                        let _ = painted.send(());
                    }
                }
            }
            _ = ticks.tick() => {
                for effect in app.update(Msg::Tick(Clock::now())) {
                    link.send(effect);
                }
                paint(terminal, &app)?;
            }
        }
        if app.should_quit {
            break;
        }
    }
    Ok(latency)
}

/// What a key means now: an action from the registry, or in a prompt or
/// the palette, text typed or a key for its line editor.
fn key_msg(app: &App, key: &KeyEvent) -> Option<Msg> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    let context = app.context();
    if let Some(action) = resolve(context, key) {
        return Some(Msg::Action(action));
    }
    if !matches!(
        context,
        Context::Prompt
            | Context::Adding
            | Context::Notes
            | Context::Palette
            | Context::Folder
            | Context::MoveTo
    ) {
        return None;
    }
    match key.code {
        KeyCode::Char(ch)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            Some(Msg::Char(ch))
        }
        _ => Some(Msg::Key(*key)),
    }
}

/// Do what `update` left for this machine: save the theme, open a link or
/// copy one.
fn local(app: &mut App, effect: LocalEffect) {
    match effect {
        LocalEffect::SaveTheme => {
            if let Some(path) = app.theme_choice.config_file.clone() {
                let name = app.theme_choice.builtin.name;
                app.theme_saved(crate::theme::save(&path, name));
            }
        }
        LocalEffect::Open(url) => {
            if let Err(error) = SystemOpener.open(&url) {
                app.show(Level::Error, &format!("Couldn't open {url}: {error}"));
            }
        }
        LocalEffect::Copy(text) => {
            let copied = crate::write_to_terminal(&ms_todo_core::links::osc52_copy(&text));
            if let Err(error) = copied {
                app.show(Level::Error, &format!("Couldn't copy the link: {error}"));
            }
        }
    }
}

fn draw<B: Backend>(terminal: &mut Terminal<B>, app: &App) -> Result<(), RunError> {
    terminal
        .draw(|frame| crate::ui::draw(frame, app))
        .map(|_| ())
        .map_err(|error| RunError::Draw(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Action;
    use crate::app::tests::seeded;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn in_a_prompt_unbound_keys_go_to_its_line_editor() {
        let mut app = seeded();
        let left = key(KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(key_msg(&app, &left), None, "nothing to edit in the list");
        app.update(Msg::Action(Action::Filter));
        assert_eq!(key_msg(&app, &left), Some(Msg::Key(left)));
        let ctrl_w = key(KeyCode::Char('w'), KeyModifiers::CONTROL);
        assert_eq!(key_msg(&app, &ctrl_w), Some(Msg::Key(ctrl_w)));
        assert_eq!(
            key_msg(&app, &key(KeyCode::Char('x'), KeyModifiers::NONE)),
            Some(Msg::Char('x'))
        );
        assert_eq!(
            key_msg(&app, &key(KeyCode::Enter, KeyModifiers::NONE)),
            Some(Msg::Action(Action::Submit))
        );
        // The field picker takes its own keys only.
        app.update(Msg::Action(Action::Cancel));
        app.update(Msg::Action(Action::Edit));
        assert_eq!(key_msg(&app, &left), None);
    }
}
