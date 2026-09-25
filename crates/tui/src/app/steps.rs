//! A task's steps and link in the detail pane (rung 8a, D-055). The
//! detail cursor moves down the fields, then the Steps heading, each step
//! and the link, then the notes, in the order they're drawn. With it on
//! a step or the link, `space` checks or unchecks the step, `a` adds a
//! step (Enter adds the next one, Esc stops), `e` or Enter edits the step
//! or the link's URL (adding one when there's none), and `d` deletes
//! either, after asking. Graph has no order for steps (S15), so there's
//! no moving them. Every change is a `ChangeTasks` write, so `u` undoes
//! it.

use ms_todo_core::links::{openable, storable};
use ms_todo_protocol::{LinkEdit, NewLink, TaskChange};

use super::edit::Field;
use super::{App, Effect, Level, LineEditor, Mode, Pane, Task, Write};
use crate::action::Action;

/// A row of the detail pane the cursor can be on, in the order drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DetailRow {
    Field(Field),
    /// The Steps heading: where the first step is added.
    Steps,
    /// The step at this index.
    Step(usize),
    /// The task's one link, or where one is added.
    Link,
}

impl Default for DetailRow {
    fn default() -> Self {
        Self::Field(Field::default())
    }
}

impl DetailRow {
    /// Every row `task` shows, in order.
    pub fn all(task: &Task) -> Vec<Self> {
        let mut rows: Vec<Self> = Field::ALL
            .iter()
            .filter(|field| **field != Field::Notes)
            .map(|field| Self::Field(*field))
            .collect();
        rows.push(Self::Steps);
        rows.extend((0..task.steps.len()).map(Self::Step));
        rows.push(Self::Link);
        rows.push(Self::Field(Field::Notes));
        rows
    }

    /// This row on `task`: a step past its last is its last, or the
    /// heading when it has none.
    pub fn on(self, task: &Task) -> Self {
        match self {
            Self::Step(at) if at >= task.steps.len() => task
                .steps
                .len()
                .checked_sub(1)
                .map_or(Self::Steps, Self::Step),
            row => row,
        }
    }

    /// Whether this row is the steps' or the link's.
    pub fn is_child(self) -> bool {
        matches!(self, Self::Steps | Self::Step(_) | Self::Link)
    }
}

/// What a step or link prompt writes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChildTarget {
    /// A new step, added last.
    NewStep,
    /// The step with this ID.
    Step(String),
    /// The link with this ID, or a new one.
    Link(Option<String>),
}

impl ChildTarget {
    /// What the hint bar calls the prompt.
    pub fn label(&self) -> &'static str {
        match self {
            Self::NewStep => "New step",
            Self::Step(_) => "Edit step",
            Self::Link(None) => "Link URL",
            Self::Link(Some(_)) => "Edit link URL",
        }
    }
}

impl App {
    /// The detail cursor's row on the selected task.
    pub fn detail_row_now(&self) -> Option<DetailRow> {
        self.selected().map(|task| self.detail_row.on(task))
    }

    /// Whether the detail pane's cursor is on the steps or the link.
    pub(super) fn on_child_row(&self) -> bool {
        self.focus == Pane::Detail && self.detail_row_now().is_some_and(DetailRow::is_child)
    }

    /// j, k, g and G in the detail pane.
    pub(super) fn move_detail(&mut self, action: Action) {
        let Some(task) = self.selected() else {
            return;
        };
        let rows = DetailRow::all(task);
        let now = self.detail_row.on(task);
        let at = rows.iter().position(|row| *row == now).unwrap_or(0);
        let last = rows.len() - 1;
        let at = match action {
            Action::MoveDown => (at + 1).min(last),
            Action::MoveUp => at.saturating_sub(1),
            Action::JumpTop => 0,
            Action::JumpBottom => last,
            _ => at,
        };
        self.detail_row = rows[at];
    }

    /// A key on a step or the link row. `None` when it's not a step or
    /// link action, which the task's own handling then takes.
    pub(super) fn child_action(&mut self, action: Action) -> Option<Vec<Effect>> {
        let is_child_key = matches!(
            action,
            Action::ToggleStep | Action::Add | Action::Edit | Action::EditHere | Action::Delete
        );
        if !is_child_key {
            return None;
        }
        let task = self.selected()?.clone();
        let row = self.detail_row.on(&task);
        let effects = match (action, row) {
            (Action::ToggleStep, DetailRow::Step(at)) => {
                let step = &task.steps[at];
                let change = TaskChange::CheckSteps {
                    steps: vec![step.id.clone()],
                    checked: !step.checked,
                };
                vec![change_one(&task, change)]
            }
            (Action::Add, _) | (Action::Edit | Action::EditHere, DetailRow::Steps) => {
                self.prompt(&task, ChildTarget::NewStep, "");
                Vec::new()
            }
            (Action::Edit | Action::EditHere, DetailRow::Step(at)) => {
                let step = &task.steps[at];
                self.prompt(&task, ChildTarget::Step(step.id.clone()), &step.name);
                Vec::new()
            }
            (Action::Edit | Action::EditHere, DetailRow::Link) => {
                let url = task.linked.first().map(|(url, _)| url.clone());
                let target = ChildTarget::Link(task.link_id.clone());
                self.prompt(&task, target, url.as_deref().unwrap_or_default());
                Vec::new()
            }
            (Action::Delete, DetailRow::Step(at)) => {
                let step = &task.steps[at];
                self.mode = Mode::ConfirmDeleteChild {
                    id: task.id.clone(),
                    change: TaskChange::DeleteSteps {
                        steps: vec![step.id.clone()],
                    },
                    what: format!("step \"{}\"", step.name),
                };
                Vec::new()
            }
            (Action::Delete, DetailRow::Link) if task.link_id.is_some() => {
                self.mode = Mode::ConfirmDeleteChild {
                    id: task.id.clone(),
                    change: TaskChange::DeleteLink {
                        link: task.link_id.clone(),
                    },
                    what: "the link".into(),
                };
                Vec::new()
            }
            // Nothing to toggle or delete on this row.
            (Action::ToggleStep | Action::Delete, _) => Vec::new(),
            _ => return None,
        };
        Some(effects)
    }

    fn prompt(&mut self, task: &Task, target: ChildTarget, text: &str) {
        self.mode = Mode::EditingChild {
            id: task.id.clone(),
            target,
            input: LineEditor::single(text),
            error: None,
        };
    }

    /// Enter in a step or link prompt: send it, or say why it can't be
    /// sent. A new step leaves the prompt open for the next one.
    pub(super) fn submit_child(&mut self) -> Vec<Effect> {
        let Mode::EditingChild {
            id, target, input, ..
        } = &self.mode
        else {
            return Vec::new();
        };
        let (id, target, text) = (id.clone(), target.clone(), input.text().trim().to_owned());
        let Some(task) = self.task_in_scope(&id).cloned() else {
            return self.gone();
        };
        let change = match (&target, text.is_empty()) {
            (ChildTarget::NewStep, true) => {
                self.mode = Mode::Normal;
                return Vec::new();
            }
            (ChildTarget::NewStep, false) => {
                self.prompt(&task, ChildTarget::NewStep, "");
                return vec![change_one(
                    &task,
                    TaskChange::AddSteps { steps: vec![text] },
                )];
            }
            (ChildTarget::Step(_), true) => {
                return self.refuse("A step can't be empty; d deletes it");
            }
            (ChildTarget::Step(step), false) => {
                let same = task.steps.iter().any(|s| s.id == *step && s.name == text);
                if same {
                    self.mode = Mode::Normal;
                    return Vec::new();
                }
                TaskChange::EditStep {
                    step: step.clone(),
                    text,
                }
            }
            (ChildTarget::Link(_), true) => {
                return self.refuse("Type a URL; d deletes the link");
            }
            (ChildTarget::Link(link), false) => {
                if let Err(why) = storable(&text) {
                    return self.refuse(&format!("Not a link: {why}"));
                }
                if openable(&text).is_err() {
                    self.show(
                        Level::Info,
                        "Kept, but only http, https and mailto links open",
                    );
                }
                match link {
                    None => TaskChange::AddLink(NewLink {
                        url: text,
                        ..NewLink::default()
                    }),
                    Some(_) if task.linked.first().is_some_and(|(url, _)| *url == text) => {
                        self.mode = Mode::Normal;
                        return Vec::new();
                    }
                    Some(link) => TaskChange::EditLink(LinkEdit {
                        link: Some(link.clone()),
                        url: Some(text),
                        ..LinkEdit::default()
                    }),
                }
            }
        };
        self.mode = Mode::Normal;
        vec![change_one(&task, change)]
    }

    /// `y` on a step or link deletion.
    pub(super) fn confirm_child_delete(&mut self) -> Vec<Effect> {
        let Mode::ConfirmDeleteChild { id, change, .. } =
            std::mem::replace(&mut self.mode, Mode::Normal)
        else {
            return Vec::new();
        };
        vec![super::change(Write::Edit, vec![id], change)]
    }

    fn refuse(&mut self, why: &str) -> Vec<Effect> {
        if let Mode::EditingChild { error, .. } = &mut self.mode {
            *error = Some(why.to_owned());
        }
        Vec::new()
    }
}

fn change_one(task: &Task, change: TaskChange) -> Effect {
    super::change(Write::Edit, vec![task.id.clone()], change)
}

#[cfg(test)]
mod tests;
