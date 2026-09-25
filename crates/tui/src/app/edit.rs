//! Editing the fields of a task: `e` picks a field, or in the detail
//! pane takes the one under the cursor, and its editor opens there;
//! importance is set by level, with no typing. What's typed is checked
//! before anything is sent, so invalid input costs nothing but an inline
//! error. Dates are read by `ms_todo_nlp`, as the CLI's `--due` and
//! `--reminder` are.

use ms_todo_core::REMINDER_FORMAT;
use ms_todo_nlp::{NotUnderstood, ParseContext, Reading, read_due, read_importance, read_reminder};
use ms_todo_protocol::{Clearable, Importance, TaskChange, TaskEdit};

use super::steps::DetailRow;
use super::{App, Effect, Level, LineEditor, Mode, Pane, Task, Write, change};
use crate::action::Action;

/// A field the detail pane edits, in the order it shows them: j and k
/// move through them in this order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Field {
    #[default]
    Title,
    Due,
    Reminder,
    Importance,
    /// Who the task waits on (rung 8d).
    Assignee,
    Notes,
}

impl Field {
    pub const ALL: [Self; 6] = [
        Self::Title,
        Self::Due,
        Self::Reminder,
        Self::Importance,
        Self::Assignee,
        Self::Notes,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Title => "Title",
            Self::Due => "Due",
            Self::Importance => "Importance",
            Self::Reminder => "Reminder",
            Self::Assignee => "Assignee",
            Self::Notes => "Notes",
        }
    }

    /// What to type, shown under the field while it's edited and nothing
    /// better can be said.
    pub fn format_hint(self) -> &'static str {
        match self {
            Self::Title => "the new title",
            Self::Due => "today, fri, next mon, +3d, 12 oct or 2026-10-02; empty clears",
            Self::Importance => "1 high, 2 or 3 normal, 4 low",
            Self::Reminder => "17:30, tomorrow 9am or fri 5:30pm; empty clears",
            Self::Assignee => "who it waits on, a name or an email; empty clears",
            Self::Notes => "plain text, or empty to clear",
        }
    }

    /// The field's value as it's typed, to start editing from.
    pub fn current(self, task: &Task) -> String {
        match self {
            Self::Title => task.title.clone(),
            Self::Due => task
                .due
                .map(|due| due.format(ms_todo_core::DATE_FORMAT).to_string())
                .unwrap_or_default(),
            Self::Importance => importance_name(task.importance).to_owned(),
            Self::Reminder => task
                .reminder
                .map(|at| at.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default(),
            Self::Assignee => task.assignee.clone().unwrap_or_default(),
            Self::Notes => task.notes().unwrap_or_default(),
        }
    }
}

pub fn importance_name(importance: Importance) -> &'static str {
    match importance {
        Importance::Low => "low",
        Importance::Normal => "normal",
        Importance::High => "high",
    }
}

/// `I` in the picker: round the three levels.
fn next_importance(importance: Importance) -> Importance {
    match importance {
        Importance::Low => Importance::Normal,
        Importance::Normal => Importance::High,
        Importance::High => Importance::Low,
    }
}

/// The edit `text` makes to `field` of `task`: `None` when it changes
/// nothing, and why not when it can't be sent.
pub fn parse(
    field: Field,
    text: &str,
    task: &Task,
    now: &ParseContext,
) -> Result<Option<TaskEdit>, String> {
    let typed = text.trim();
    let edit = match field {
        Field::Title => {
            if typed.is_empty() {
                return Err("A title can't be empty".into());
            }
            (typed != task.title).then(|| TaskEdit {
                title: Some(typed.to_owned()),
                ..TaskEdit::default()
            })
        }
        Field::Due => {
            let due = set_value(read_due(typed, now))?;
            (due != task.due).then(|| TaskEdit {
                due: Some(clearable(
                    due.map(|due| due.format(ms_todo_core::DATE_FORMAT).to_string()),
                )),
                ..TaskEdit::default()
            })
        }
        Field::Reminder => {
            let at = set_value(read_reminder(typed, now))?;
            (at != task.reminder).then(|| TaskEdit {
                reminder: Some(clearable(
                    at.map(|at| at.format(REMINDER_FORMAT).to_string()),
                )),
                ..TaskEdit::default()
            })
        }
        // Set by level in the picker, never typed.
        Field::Importance => None,
        Field::Assignee => assignee_edit(typed, task.assignee.as_deref()),
        // Compared as rendered, so html notes left alone stay html.
        Field::Notes => (typed != task.notes().unwrap_or_default()).then(|| TaskEdit {
            body: Some(typed.to_owned()),
            ..TaskEdit::default()
        }),
    };
    Ok(edit)
}

/// Assigning to `typed`, or clearing the assignee when it's empty; `None`
/// when that's what the task has. The daemon pairs the status with it.
pub(super) fn assignee_edit(typed: &str, current: Option<&str>) -> Option<TaskEdit> {
    let typed = typed.trim();
    let assignee = match (typed, current) {
        ("", None) => return None,
        ("", Some(_)) => Clearable::Clear,
        (name, Some(current)) if name == current => return None,
        (name, _) => Clearable::Set(name.to_owned()),
    };
    Some(TaskEdit {
        assignee: Some(assignee),
        ..TaskEdit::default()
    })
}

pub(super) fn set_value<T>(
    reading: Result<Reading<T>, NotUnderstood>,
) -> Result<Option<T>, String> {
    match reading.map_err(|why| capitalised(&why.0))? {
        Reading::Clear => Ok(None),
        Reading::Set { value, .. } => Ok(Some(value)),
    }
}

/// `→ Fri 2 Oct`, or what clearing means.
fn preview_line<T>(reading: Reading<T>) -> String {
    match reading {
        Reading::Clear => "\u{2192} none: clears it".to_owned(),
        Reading::Set { preview, .. } => format!("\u{2192} {preview}"),
    }
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn clearable(value: Option<String>) -> Clearable<String> {
    value.map_or(Clearable::Clear, Clearable::Set)
}

impl App {
    /// What typed dates are read against.
    pub fn parse_context(&self) -> ParseContext {
        ParseContext::new(self.clock.now)
    }

    /// What the date being typed resolves to, for the line under it:
    /// `→ Fri 2 Oct`, or why it can't be read yet. `None` for a field
    /// that isn't a date, or nothing typed that would change it.
    pub fn date_preview(&self) -> Option<Result<String, String>> {
        let (field, input) = match &self.mode {
            Mode::Editing { field, input, .. } => (field, input),
            // Several tasks' due date: nothing typed is nothing to show,
            // not a clear.
            Mode::SettingDue { input, .. } if !input.text().trim().is_empty() => {
                (&Field::Due, input)
            }
            _ => return None,
        };
        let now = self.parse_context();
        let text = input.text();
        let resolved = match field {
            Field::Due => read_due(&text, &now).map(preview_line),
            Field::Reminder => read_reminder(&text, &now).map(preview_line),
            _ => return None,
        };
        Some(resolved.map_err(|why| capitalised(&why.0)))
    }

    /// The task an edit is for: the one the picker was opened on, else
    /// the selected one.
    fn edit_target(&self) -> Option<String> {
        match &self.mode {
            Mode::ChoosingField { id } | Mode::ChoosingImportance { id } => Some(id.clone()),
            Mode::Normal => self.selected().map(|task| task.id.clone()),
            _ => None,
        }
    }

    /// The rows may have changed since the edit began: only a task still
    /// on screen, found by its ID, is edited (5a).
    pub(super) fn task_in_scope(&self, id: &str) -> Option<&Task> {
        self.tasks
            .iter()
            .find(|task| task.id == id)
            .filter(|_| !self.loading())
    }

    pub(super) fn gone(&mut self) -> Vec<Effect> {
        self.mode = Mode::Normal;
        self.show(Level::Info, "That task is gone; nothing was changed");
        Vec::new()
    }

    /// `e` (the picker, or the field under the detail cursor), Enter in
    /// the detail pane, a field picked, and importance by level or cycled.
    pub(super) fn edit_action(&mut self, action: Action) -> Vec<Effect> {
        if self.mode == Mode::Normal && self.still_loading() {
            return Vec::new();
        }
        // With a selection, a due date or an assignee goes to every task
        // in it.
        if let Action::EditField(field @ (Field::Due | Field::Assignee)) = action
            && !self.selection.is_empty()
        {
            self.mode = Mode::Normal;
            match field {
                Field::Due => self.start_set_due(),
                _ => self.start_assign(),
            }
            return Vec::new();
        }
        let Some(id) = self.edit_target() else {
            self.mode = Mode::Normal;
            return Vec::new();
        };
        match action {
            // In the detail pane the cursor already says which field;
            // elsewhere, or for a selection, the picker asks.
            Action::Edit => match self.detail_row_now() {
                Some(DetailRow::Field(field))
                    if self.focus == Pane::Detail && self.selection.is_empty() =>
                {
                    self.open_field(id, field)
                }
                _ => {
                    self.mode = Mode::ChoosingField { id };
                    Vec::new()
                }
            },
            Action::EditHere => match self.detail_row {
                DetailRow::Field(field) => self.open_field(id, field),
                // The steps' and link's rows are `steps`' to edit.
                _ => Vec::new(),
            },
            Action::EditField(field) => self.open_field(id, field),
            Action::CycleImportance => {
                let Some(task) = self.task_in_scope(&id) else {
                    return self.gone();
                };
                let next = next_importance(task.importance);
                self.set_importance(id, next)
            }
            Action::SetImportance(level) => match read_importance(level) {
                Ok(level) => self.set_importance(id, protocol_importance(level)),
                Err(why) => {
                    self.mode = Mode::Normal;
                    self.show(Level::Error, &why.0);
                    Vec::new()
                }
            },
            _ => Vec::new(),
        }
    }

    /// Open `field`'s editor on the task `id`, or for importance, the
    /// level picker. The detail pane's cursor follows, for the next one.
    fn open_field(&mut self, id: String, field: Field) -> Vec<Effect> {
        let Some(task) = self.task_in_scope(&id) else {
            return self.gone();
        };
        let input = match field {
            Field::Importance => None,
            Field::Notes => Some(LineEditor::multi(&field.current(task))),
            _ => Some(LineEditor::single(&field.current(task))),
        };
        self.focus = Pane::Detail;
        self.set_detail_row(DetailRow::Field(field));
        self.mode = match input {
            None => Mode::ChoosingImportance { id },
            Some(input) => Mode::Editing {
                id,
                field,
                input,
                error: None,
            },
        };
        Vec::new()
    }

    /// Send `importance` for the task `id`, unless it has it already.
    fn set_importance(&mut self, id: String, importance: Importance) -> Vec<Effect> {
        let Some(task) = self.task_in_scope(&id) else {
            return self.gone();
        };
        let same = task.importance == importance;
        self.mode = Mode::Normal;
        self.set_detail_row(DetailRow::Field(Field::Importance));
        if same {
            return Vec::new();
        }
        let edit = TaskEdit {
            importance: Some(importance),
            ..TaskEdit::default()
        };
        vec![change(Write::Edit, vec![id], TaskChange::Edit(edit))]
    }

    /// Enter while editing (Ctrl-s in notes): send the change, or say why
    /// it can't be sent and keep editing.
    pub(super) fn submit_edit(&mut self) -> Vec<Effect> {
        let Mode::Editing {
            id, field, input, ..
        } = &self.mode
        else {
            return Vec::new();
        };
        let (id, field, text) = (id.clone(), *field, input.text());
        let now = self.parse_context();
        let Some(task) = self.task_in_scope(&id) else {
            return self.gone();
        };
        match parse(field, &text, task, &now) {
            Err(why) => {
                if let Mode::Editing { error, .. } = &mut self.mode {
                    *error = Some(why);
                }
                Vec::new()
            }
            Ok(None) => {
                self.mode = Mode::Normal;
                Vec::new()
            }
            Ok(Some(edit)) => {
                self.mode = Mode::Normal;
                vec![change(Write::Edit, vec![id], TaskChange::Edit(edit))]
            }
        }
    }
}

pub(super) fn protocol_importance(level: ms_todo_nlp::Importance) -> Importance {
    match level {
        ms_todo_nlp::Importance::High => Importance::High,
        ms_todo_nlp::Importance::Normal => Importance::Normal,
        ms_todo_nlp::Importance::Low => Importance::Low,
    }
}

#[cfg(test)]
mod tests;
