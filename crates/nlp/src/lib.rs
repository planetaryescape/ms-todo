//! Reading what people type: dates, times, importance, and quick add's
//! whole task in one line (docs/blueprint/06-natural-language.md). Pure:
//! no I/O and no clock. `now` comes in through [`ParseContext`], so every
//! answer is a function of the input and the context, and tests pick their
//! own `now`.

mod dates;
mod importance;
mod quick_add;
mod recurrence;

pub use dates::{
    DueSpec, Lean, NotUnderstood, ParseContext, Reading, read_due, read_past_date, read_reminder,
    read_when,
};
pub use importance::{Importance, read_importance};
pub use quick_add::{
    DeterministicParser, ListRef, ParsedTask, QuickAddContext, QuickAddParser, Span, SpanKind,
    list_token,
};
pub use recurrence::{Pattern, Recurrence, RecurrenceEnd, WeekIndex};
