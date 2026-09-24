//! Finding tasks by the words in them (D-041): SQLite FTS5 over each live
//! task's title and plain-text body (migration 0004), ranked by bm25 with
//! the title weighted above the body.
//!
//! Queries use FTS5's syntax (words, `"phrases"`, `prefix*`, `AND`, `OR`,
//! `NOT`, parentheses), but every word is quoted before FTS5 sees it, so
//! punctuation in a word (`e-mail`, `don't`, `v1.2`) is searched for as
//! text instead of failing as syntax. A query with no operators is all its
//! words ANDed.

use sqlx::sqlite::SqliteRow;
use sqlx::{AssertSqlSafe, FromRow, Row, SqlitePool};

use crate::graph_columns::body_text;
use crate::tasks::{TaskRecord, TaskRow, task_record_columns};
use crate::{Store, StoreError};

/// Opens and closes each match in a [`SearchHit::snippet`]; the protocol
/// documents it (`ResponseData::SearchResults`) and the CLI's table reads it.
const MATCH_MARK: &str = "**";

/// How much of the matching column a snippet shows, in tokens (FTS5's
/// limit is 64).
const SNIPPET_TOKENS: i64 = 12;

/// How much more a word in the title counts than one in the body.
const TITLE_WEIGHT: f64 = 10.0;

/// Which tasks a search considers by status.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatusFilter {
    /// Not completed.
    #[default]
    Open,
    Completed,
    All,
}

pub struct TaskSearch<'a> {
    /// In FTS5's query syntax; see the module docs.
    pub query: &'a str,
    /// Only this list's tasks.
    pub list_local_id: Option<&'a str>,
    pub status: StatusFilter,
    /// At most this many, best first; `None` is every match.
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub task: TaskRow,
    /// The display name of the task's list.
    pub list_name: String,
    /// A short passage of the title or body that matched, on one line,
    /// each match between [`MATCH_MARK`]s and `…` where it was cut.
    pub snippet: String,
}

impl Store {
    /// Live tasks matching `search`, best match first.
    pub async fn search_tasks(
        &self,
        search: &TaskSearch<'_>,
    ) -> Result<Vec<SearchHit>, StoreError> {
        let expression = match_expression(search.query)?;
        let status = match search.status {
            StatusFilter::Open => "AND tasks.status <> 'completed'",
            StatusFilter::Completed => "AND tasks.status = 'completed'",
            StatusFilter::All => "",
        };
        let rows: Vec<SqliteRow> = sqlx::query(AssertSqlSafe(format!(
            "SELECT {}, lists.display_name AS list_name, \
             snippet(tasks_fts, -1, ?1, ?1, '…', ?2) AS snippet \
             FROM tasks_fts \
             JOIN tasks ON tasks.rowid = tasks_fts.rowid \
             JOIN lists ON lists.local_id = tasks.list_local_id \
             WHERE tasks_fts MATCH ?3 AND tasks.deleted_at IS NULL AND lists.deleted_at IS NULL \
             AND (?4 IS NULL OR tasks.list_local_id = ?4) {status} \
             ORDER BY bm25(tasks_fts, ?5, 1.0), tasks.created_at DESC, tasks.rowid \
             LIMIT ?6",
            task_record_columns()
        )))
        .bind(MATCH_MARK)
        .bind(SNIPPET_TOKENS)
        .bind(&expression)
        .bind(search.list_local_id)
        .bind(TITLE_WEIGHT)
        .bind(search.limit.map_or(-1, i64::from))
        .fetch_all(self.reader())
        .await
        .map_err(|error| query_error(error, search.query))?;
        rows.iter()
            .map(|row| {
                Ok(SearchHit {
                    task: TaskRow::try_from(TaskRecord::from_row(row)?)?,
                    list_name: row.try_get("list_name")?,
                    snippet: tidy_snippet(&row.try_get::<String, _>("snippet")?),
                })
            })
            .collect()
    }
}

/// Fill `body_text` for html bodies cached before migration 0004, which SQL
/// couldn't render; the update trigger indexes each. Later writes fill it
/// themselves, so after the first open this finds nothing, through a
/// partial index that's empty.
pub(crate) async fn fill_body_text(writer: &SqlitePool) -> Result<(), StoreError> {
    let missing: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT local_id, body_content, body_content_type FROM tasks \
         WHERE body_text IS NULL AND body_content IS NOT NULL",
    )
    .fetch_all(writer)
    .await?;
    if missing.is_empty() {
        return Ok(());
    }
    // Rendered before the transaction, so the writer isn't held meanwhile.
    let rendered: Vec<(String, String)> = missing
        .into_iter()
        .map(|(local_id, content, content_type)| {
            (local_id, body_text(&content, content_type.as_deref()))
        })
        .collect();
    let mut tx = writer.begin().await?;
    for (local_id, text) in rendered {
        sqlx::query("UPDATE tasks SET body_text = ? WHERE local_id = ?")
            .bind(text)
            .bind(local_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// The FTS5 expression for a user's query: each word quoted (a trailing
/// `*` kept outside the quotes), quoted phrases, operators and parentheses
/// as they are, and `AND` between operands with no operator. FTS5 still judges the structure, such as `OR` with nothing
/// before it.
fn match_expression(query: &str) -> Result<String, StoreError> {
    let mut parts: Vec<String> = Vec::new();
    let mut terms = 0;
    let mut chars = query.chars().peekable();
    while let Some(&next) = chars.peek() {
        match next {
            c if c.is_whitespace() => {
                chars.next();
            }
            '(' | ')' => {
                push(&mut parts, next.to_string());
                chars.next();
            }
            '"' => {
                chars.next();
                let mut phrase = String::from('"');
                loop {
                    match chars.next() {
                        // `""` inside a phrase is a literal quote in FTS5.
                        Some('"') if chars.peek() == Some(&'"') => {
                            chars.next();
                            phrase.push_str("\"\"");
                        }
                        Some('"') => break,
                        Some(c) => phrase.push(c),
                        None => {
                            return Err(StoreError::InvalidQuery(
                                "a quoted phrase isn't closed: add the closing \"".into(),
                            ));
                        }
                    }
                }
                phrase.push('"');
                if chars.peek() == Some(&'*') {
                    chars.next();
                    phrase.push('*');
                }
                push(&mut parts, phrase);
                terms += 1;
            }
            _ => {
                let mut word = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || matches!(c, '(' | ')' | '"') {
                        break;
                    }
                    word.push(c);
                    chars.next();
                }
                if matches!(word.as_str(), "AND" | "OR" | "NOT") {
                    push(&mut parts, word);
                    continue;
                }
                let stem = word.trim_end_matches('*');
                // Only punctuation: the tokenizer would find no word in it.
                if !stem.chars().any(char::is_alphanumeric) {
                    continue;
                }
                let prefix = if stem.len() < word.len() { "*" } else { "" };
                push(&mut parts, format!("\"{stem}\"{prefix}"));
                terms += 1;
            }
        }
    }
    if terms == 0 {
        return Err(StoreError::InvalidQuery(format!(
            "{query:?} has no words to search for"
        )));
    }
    Ok(parts.join(" "))
}

/// Add `part`, with an explicit `AND` between two operands: FTS5 ANDs
/// adjacent phrases by itself, but not a phrase and a parenthesis, as in
/// `milk (eggs OR bread)`.
fn push(parts: &mut Vec<String>, part: String) {
    let starts_operand = part == "(" || part.starts_with('"');
    let after_operand = parts
        .last()
        .is_some_and(|last| last == ")" || last.starts_with('"'));
    if starts_operand && after_operand {
        parts.push("AND".into());
    }
    parts.push(part);
}

/// FTS5 rejects a malformed expression when the query runs.
fn query_error(error: sqlx::Error, query: &str) -> StoreError {
    match &error {
        sqlx::Error::Database(database) if database.message().starts_with("fts5:") => {
            StoreError::InvalidQuery(format!(
                "{query:?} isn't a valid search ({}); use words, \"phrases\", prefix*, AND, OR, NOT and parentheses",
                database.message()
            ))
        }
        _ => error.into(),
    }
}

/// A body's snippet can span lines; a snippet is one.
fn tidy_snippet(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expression(query: &str) -> String {
        match_expression(query).expect("valid")
    }

    #[test]
    fn plain_words_are_quoted_and_anded() {
        assert_eq!(expression("car insurance"), r#""car" AND "insurance""#);
        assert_eq!(
            expression("e-mail don't v1.2"),
            r#""e-mail" AND "don't" AND "v1.2""#
        );
        assert_eq!(
            expression("milk (eggs OR bread)"),
            r#""milk" AND ( "eggs" OR "bread" )"#
        );
    }

    #[test]
    fn operators_phrases_prefixes_and_parentheses_pass_through() {
        assert_eq!(
            expression(r#"(insur* OR "car tax") NOT renew"#),
            r#"( "insur"* OR "car tax" ) NOT "renew""#
        );
        assert_eq!(expression(r#""car ta"*"#), r#""car ta"*"#);
        assert_eq!(expression(r#""say ""hi""""#), r#""say ""hi""""#);
        // Lowercase operators are words, as in FTS5.
        assert_eq!(expression("rock or roll"), r#""rock" AND "or" AND "roll""#);
    }

    #[test]
    fn punctuation_alone_is_skipped_and_nothing_left_is_an_error() {
        assert_eq!(expression("milk - eggs"), r#""milk" AND "eggs""#);
        assert!(matches!(
            match_expression("  -- * "),
            Err(StoreError::InvalidQuery(_))
        ));
        assert!(matches!(
            match_expression(r#""unclosed"#),
            Err(StoreError::InvalidQuery(_))
        ));
    }

    #[test]
    fn snippets_are_one_line() {
        assert_eq!(
            tidy_snippet("renew\n the **car**  insurance…"),
            "renew the **car** insurance…"
        );
    }
}
