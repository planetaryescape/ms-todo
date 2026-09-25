//! Quick add: a task typed the way people think it, `Pay rent every 1st
//! #Finances p1 9am`, read into its fields
//! (docs/blueprint/06-natural-language.md). Deterministic, no LLM
//! (D-016): [`DeterministicParser`] is the one [`QuickAddParser`], and the
//! trait is where another could go.

mod passes;

use chrono::{NaiveDate, NaiveDateTime};
use serde_json::{Value, json};

pub use passes::DeterministicParser;

use crate::dates::{DueSpec, ParseContext, preview};
use crate::importance::Importance;
use crate::recurrence::Recurrence;

/// A list `#List` can name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListRef {
    pub id: String,
    pub name: String,
}

/// How to type the list `name` so quick add reads it back whole: `#Name`
/// for one word the reader keeps as it is, else `#"Two words"`.
pub fn list_token(name: &str) -> String {
    let bare = !name.is_empty()
        && !name.contains(|ch: char| ch.is_whitespace() || ch == '"')
        && !name.ends_with([',', '.', ';', ':', '!', '?', ')']);
    if bare {
        format!("#{name}")
    } else {
        format!("#\"{name}\"")
    }
}

/// What quick add reads against: now, the lists `#` can name, and the
/// categories `@` knows. `categories` is `None` when they couldn't be
/// read, and then no label is called unknown. `due` is a due date given
/// outside the text (the CLI's `--due`): it wins over the text's, and the
/// reminder and a recurrence's start follow it.
#[derive(Clone, Copy, Debug)]
pub struct QuickAddContext<'a> {
    pub when: ParseContext,
    pub lists: &'a [ListRef],
    pub categories: Option<&'a [String]>,
    pub due: Option<NaiveDate>,
}

pub trait QuickAddParser {
    fn parse(&self, input: &str, ctx: &QuickAddContext) -> ParsedTask;
}

/// A task's fields as read from its text. Due and start are dates only
/// (D-027): a time goes to `reminder`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedTask {
    /// What's left once the recognised parts are taken out, with runs of
    /// spaces made one. Empty when nothing else was typed.
    pub title: String,
    /// `None`: the caller's default (D-022's "Tasks").
    pub list: Option<ListRef>,
    pub due: Option<NaiveDate>,
    pub start: Option<NaiveDate>,
    pub reminder: Option<NaiveDateTime>,
    pub recurrence: Option<Recurrence>,
    pub importance: Option<Importance>,
    /// The level typed, `p1`–`p4`: Graph keeps three, so p2 and p3 are
    /// both normal (D-017).
    pub priority: Option<u8>,
    pub categories: Vec<String>,
    /// `+myday` or `*` was typed. My Day arrives in rung 7; until then
    /// this is only reported, with a warning.
    pub my_day: bool,
    /// The recognised parts, by byte range of the input, in order.
    pub spans: Vec<Span>,
    /// What was typed but not used, and why.
    pub warnings: Vec<String>,
}

/// A recognised part of the input, for highlighting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: SpanKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanKind {
    List,
    Label,
    Priority,
    Recurrence,
    Reminder,
    Start,
    /// The due date, and a time that goes with it.
    Date,
    MyDay,
    /// Escapes and quote marks: taken out, not a field.
    Syntax,
}

impl SpanKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Label => "label",
            Self::Priority => "priority",
            Self::Recurrence => "recurrence",
            Self::Reminder => "reminder",
            Self::Start => "start",
            Self::Date => "date",
            Self::MyDay => "my_day",
            Self::Syntax => "syntax",
        }
    }
}

impl ParsedTask {
    /// The fields read, as a person reads them back, in the order the
    /// preview line shows them: `p1`, `due Thu 1 Oct`, `every month on the
    /// 1st`, `remind 09:00`. The list isn't here: the caller knows the
    /// target when none was typed.
    pub fn summary(&self, ctx: &ParseContext) -> Vec<String> {
        let mut parts = Vec::new();
        if let Some(priority) = self.priority {
            parts.push(format!("p{priority}"));
        }
        if let Some(due) = self.due {
            parts.push(format!("due {}", preview(DueSpec::Date(due), ctx)));
        }
        if let Some(start) = self.start {
            parts.push(format!("start {}", preview(DueSpec::Date(start), ctx)));
        }
        if let Some(recurrence) = &self.recurrence {
            parts.push(recurrence.describe());
        }
        if let Some(at) = self.reminder {
            if Some(at.date()) == self.due {
                parts.push(at.format("remind %H:%M").to_string());
            } else {
                parts.push(format!("remind {}", preview(DueSpec::DateTime(at), ctx)));
            }
        }
        for category in &self.categories {
            parts.push(format!("@{category}"));
        }
        parts
    }

    /// For `tasks parse` and `--dry-run`: every field, dates as
    /// `YYYY-MM-DD`, the reminder as local `YYYY-MM-DDTHH:MM`, the
    /// recurrence as Graph's `patternedRecurrence` (less the zone, which
    /// the daemon adds) with its description, and each span with the text
    /// it covers.
    pub fn to_json(&self, input: &str) -> Value {
        let date = |date: Option<NaiveDate>| date.map(|date| date.format("%Y-%m-%d").to_string());
        let recurrence = self.recurrence.as_ref().map(|recurrence| {
            let mut graph = recurrence.to_graph();
            graph["description"] = json!(recurrence.describe());
            graph
        });
        let spans: Vec<Value> = self
            .spans
            .iter()
            .map(|span| {
                json!({
                    "start": span.start,
                    "end": span.end,
                    "kind": span.kind.name(),
                    "text": input.get(span.start..span.end).unwrap_or_default(),
                })
            })
            .collect();
        json!({
            "title": self.title,
            "list": self.list.as_ref().map(|list| json!({ "id": list.id, "name": list.name })),
            "due": date(self.due),
            "start": date(self.start),
            "reminder": self.reminder.map(|at| at.format("%Y-%m-%dT%H:%M").to_string()),
            "recurrence": recurrence,
            "importance": self.importance.map(Importance::name),
            "priority": self.priority,
            "categories": self.categories,
            "my_day": self.my_day,
            "spans": spans,
            "warnings": self.warnings,
        })
    }
}

#[cfg(test)]
mod tests;
