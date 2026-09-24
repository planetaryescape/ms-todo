//! Reading what people type: dates, times and importance now, rung 6's
//! quick add later (docs/blueprint/06-natural-language.md). Pure: no I/O and no
//! clock. `now` comes in through [`ParseContext`], so every answer is a
//! function of the input and the context, and tests pick their own `now`.

mod dates;
mod importance;

pub use dates::{
    DueSpec, NotUnderstood, ParseContext, Reading, read_due, read_reminder, read_when,
};
pub use importance::{Importance, read_importance};
