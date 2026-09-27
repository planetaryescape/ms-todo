//! The arguments of `search`, `done` and `next`: finding tasks across
//! every list, by their words, the day they were completed, or what's
//! most urgent.

use clap::{Args, ValueEnum};

use crate::phrases;

#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Words that must all appear, in any order. Also: "an exact phrase",
    /// prefix* for words starting with it, OR, NOT and parentheses (the
    /// operators in capitals)
    #[arg(required = true, value_name = "QUERY")]
    pub query: Vec<String>,
    /// Only this list (exact name or ID) [default: every list]
    #[arg(long, value_name = "NAME|ID")]
    pub list: Option<String>,
    /// Which tasks to look through
    #[arg(long, value_enum, default_value_t = SearchStatusArg::Open)]
    pub status: SearchStatusArg,
    /// At most this many results
    #[arg(long, value_name = "N", default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..))]
    pub limit: u32,
    /// Find tasks by meaning, not words ("dentist" finds "Book teeth
    /// cleaning"), with a local model; the query is plain text. Needs
    /// `semantic = true` under [search] in config.toml
    #[arg(long)]
    pub semantic: bool,
}

#[derive(Debug, Args)]
pub struct DoneArgs {
    /// From this day: yesterday, mon (the latest Monday, today included),
    /// last week (its Monday), 12 sep, 3 days ago, 2026-09-01 [default: 7
    /// days ago]
    #[arg(long, value_name = "WHEN", value_parser = phrases::past_day, allow_hyphen_values = true)]
    pub since: Option<String>,
    /// Up to and including this day, in the same forms [default: today]
    #[arg(long, value_name = "WHEN", value_parser = phrases::past_day, allow_hyphen_values = true)]
    pub until: Option<String>,
    /// Only this list (exact name or ID) [default: every list]
    #[arg(long, value_name = "NAME|ID", conflicts_with = "folder")]
    pub list: Option<String>,
    /// Only the lists in this folder
    #[arg(long, value_name = "FOLDER")]
    pub folder: Option<String>,
    /// At most this many, newest first
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub limit: Option<u32>,
}

#[derive(Debug, Args)]
pub struct NextArgs {
    /// At most this many
    #[arg(long, value_name = "N", default_value_t = 5, value_parser = clap::value_parser!(u32).range(1..))]
    pub limit: u32,
    /// Only this list (exact name or ID) [default: every list]
    #[arg(long, value_name = "NAME|ID")]
    pub list: Option<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum SearchStatusArg {
    /// Not completed
    Open,
    /// Completed only
    Completed,
    /// Open and completed
    All,
}
