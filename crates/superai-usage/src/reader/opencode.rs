//! opencode local v2 history database reader (USG-03), read-only. Only
//! `session_message` and `session_v2` are ever selected; credential tables
//! never leave the file.

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, Row};

use crate::cost::{RecordedUsage, UsageEvent};

/// Resolve the v2 history database per docs/harness-configs/opencode.md: a
/// non-empty `OPENCODE_DB` value wins, else `$XDG_DATA_HOME/opencode/` with
/// a channel-suffixed file name on non-default channels; an unset or
/// relative `XDG_DATA_HOME` falls back to `~/.local/share`. Returns `None`
/// when the channel is not one safe file-name component (ASCII
/// alphanumerics, `-`, `_`).
pub fn history_db_path(
    opencode_db_override: Option<&Path>,
    xdg_data_home: Option<&Path>,
    home: &Path,
    channel: Option<&str>,
) -> Option<PathBuf> {
    if let Some(path) = opencode_db_override.filter(|path| !path.as_os_str().is_empty()) {
        return Some(path.to_path_buf());
    }
    let file_name = match channel.map(str::trim).filter(|channel| !channel.is_empty()) {
        Some(channel) if safe_channel(channel) => format!("opencode-{channel}.db"),
        Some(_) => return None,
        None => "opencode.db".to_owned(),
    };
    Some(match xdg_data_home {
        Some(dir) if dir.is_absolute() => dir.join("opencode").join(file_name),
        _ => home
            .join(".local")
            .join("share")
            .join("opencode")
            .join(file_name),
    })
}

fn safe_channel(channel: &str) -> bool {
    !channel.is_empty()
        && channel
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Errors from the opencode v2 history scan.
#[derive(Debug, thiserror::Error)]
pub enum OpenCodeError {
    /// The database could not be opened read-only (missing, unreadable,
    /// locked, or not a regular file).
    #[error("opencode database open failed: {0}")]
    Open(OpenDbFailure),
    /// The history query failed before any row was read.
    #[error("opencode history query failed: {0}")]
    Scan(rusqlite::Error),
}

/// Why the read-only open was refused.
#[derive(Debug, thiserror::Error)]
pub enum OpenDbFailure {
    /// Opening a FIFO or device node would block the scan forever, so the
    /// path is refused before `SQLite` ever sees it.
    #[error("{} is not a regular file", .0.display())]
    NotRegularFile(PathBuf),
    /// `SQLite` refused the open itself.
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

// Mirrors opencode's own usage scan (packages/core/src/session/stats.ts at
// v2.0.27); the CASE keeps malformed JSON a counted skip, not a query error.
const HISTORY_QUERY: &str = "\
SELECT
    parsed.time_created,
    json_extract(parsed.data, '$.model.providerID') AS provider_id,
    json_extract(parsed.data, '$.model.id') AS model_id,
    json_extract(parsed.data, '$.tokens.input') AS tokens_input,
    json_extract(parsed.data, '$.tokens.output') AS tokens_output,
    json_extract(parsed.data, '$.tokens.reasoning') AS tokens_reasoning,
    json_extract(parsed.data, '$.tokens.cache.read') AS tokens_cache_read,
    json_extract(parsed.data, '$.tokens.cache.write') AS tokens_cache_write
FROM (
    SELECT
        message.time_created AS time_created,
        message.session_id AS session_id,
        message.type AS type,
        CASE WHEN json_valid(message.data) THEN message.data END AS data
    FROM session_message AS message
) AS parsed
JOIN session_v2 AS session ON session.id = parsed.session_id
WHERE parsed.type = 'assistant'
  AND (session.fork_session_id IS NULL OR parsed.time_created >= session.time_created)";

/// Scan the v2 history database into recorded usage, opening it read-only
/// and never writing; unmappable rows are counted and all-zero rows are
/// dropped, never fatal. A failure after rows have folded keeps those rows
/// and counts the unread remainder as one skip, so the result is a floor.
pub fn scan_history(db_path: &Path) -> Result<RecordedUsage, OpenCodeError> {
    match std::fs::metadata(db_path) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => {
            return Err(OpenCodeError::Open(OpenDbFailure::NotRegularFile(
                db_path.to_path_buf(),
            )));
        }
        // A failed stat is reported by the open below, which owns the reason.
        Err(_) => {}
    }
    let connection = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|source| OpenCodeError::Open(OpenDbFailure::Sqlite(source)))?;
    let mut statement = connection
        .prepare(HISTORY_QUERY)
        .map_err(OpenCodeError::Scan)?;
    let mut rows = statement.query([]).map_err(OpenCodeError::Scan)?;
    let mut usage = RecordedUsage::default();
    loop {
        let row = match rows.next() {
            Ok(Some(row)) => row,
            Ok(None) => break,
            Err(_) => {
                usage.unscanned_records += 1;
                break;
            }
        };
        match row_usage(row) {
            RowUsage::Event(event) => usage.events.push(event),
            RowUsage::Zero => {}
            RowUsage::Malformed => usage.unscanned_records += 1,
        }
    }
    Ok(usage)
}

enum RowUsage {
    Event(UsageEvent),
    Zero,
    Malformed,
}

#[derive(Clone, Copy)]
enum Cell {
    Absent,
    Value(u64),
    Malformed,
}

fn token_cell(row: &Row<'_>, column: &str) -> Option<Cell> {
    Some(match row.get_ref(column).ok()? {
        ValueRef::Null => Cell::Absent,
        ValueRef::Integer(n) => u64::try_from(n).map_or(Cell::Malformed, Cell::Value),
        _ => Cell::Malformed,
    })
}

fn zero_when_absent(cell: Cell) -> Option<u64> {
    match cell {
        Cell::Absent => Some(0),
        Cell::Value(v) => Some(v),
        Cell::Malformed => None,
    }
}

fn text_cell<'a>(row: &'a Row<'_>, column: &str) -> Option<&'a str> {
    match row.get_ref(column).ok()? {
        ValueRef::Text(bytes) => std::str::from_utf8(bytes).ok().filter(|s| !s.is_empty()),
        _ => None,
    }
}

fn row_event(row: &Row<'_>) -> Option<UsageEvent> {
    let provider = text_cell(row, "provider_id")?.to_owned();
    let model = text_cell(row, "model_id")?.to_owned();
    let input_tokens = zero_when_absent(token_cell(row, "tokens_input")?)?;
    let output = zero_when_absent(token_cell(row, "tokens_output")?)?;
    let reasoning = zero_when_absent(token_cell(row, "tokens_reasoning")?)?;
    let cache_read_tokens = match token_cell(row, "tokens_cache_read")? {
        Cell::Absent => None,
        Cell::Value(v) => Some(v),
        Cell::Malformed => return None,
    };
    let cache_write_tokens = match token_cell(row, "tokens_cache_write")? {
        Cell::Absent => None,
        Cell::Value(v) => Some(v),
        Cell::Malformed => return None,
    };
    let ValueRef::Integer(created_ms) = row.get_ref("time_created").ok()? else {
        return None;
    };
    Some(UsageEvent {
        timestamp: Timestamp::from_millisecond(created_ms).ok()?,
        provider,
        model,
        input_tokens,
        output_tokens: output.checked_add(reasoning)?,
        cache_read_tokens,
        cache_write_tokens,
        cache_write_1h_tokens: None,
    })
}

fn row_usage(row: &Row<'_>) -> RowUsage {
    let Some(event) = row_event(row) else {
        return RowUsage::Malformed;
    };
    if event.input_tokens == 0
        && event.output_tokens == 0
        && event.cache_read_tokens.is_none_or(|tokens| tokens == 0)
        && event.cache_write_tokens.is_none_or(|tokens| tokens == 0)
    {
        return RowUsage::Zero;
    }
    RowUsage::Event(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_value_wins_over_every_derived_path() {
        let r#override = Path::new("/elsewhere/custom.db");
        assert_eq!(
            history_db_path(
                Some(r#override),
                Some(Path::new("/data")),
                Path::new("/home/u"),
                Some("beta")
            ),
            Some(PathBuf::from("/elsewhere/custom.db"))
        );
    }

    #[test]
    fn empty_override_is_treated_as_unset() {
        assert_eq!(
            history_db_path(
                Some(Path::new("")),
                Some(Path::new("/data")),
                Path::new("/home/u"),
                None
            ),
            Some(PathBuf::from("/data/opencode/opencode.db"))
        );
    }

    #[test]
    fn default_channel_uses_the_plain_file_name() {
        assert_eq!(
            history_db_path(None, Some(Path::new("/data")), Path::new("/home/u"), None),
            Some(PathBuf::from("/data/opencode/opencode.db"))
        );
    }

    #[test]
    fn blank_channel_is_the_default_channel() {
        assert_eq!(
            history_db_path(
                None,
                Some(Path::new("/data")),
                Path::new("/home/u"),
                Some("  ")
            ),
            Some(PathBuf::from("/data/opencode/opencode.db"))
        );
    }

    #[test]
    fn non_default_channel_suffixed_file_name() {
        assert_eq!(
            history_db_path(
                None,
                Some(Path::new("/data")),
                Path::new("/home/u"),
                Some("beta")
            ),
            Some(PathBuf::from("/data/opencode/opencode-beta.db"))
        );
    }

    #[test]
    fn channel_must_be_one_safe_file_name_component() {
        for escaping in [
            "a/b",
            "..",
            ".",
            "x/../../escape",
            "win\\path",
            "sp ace",
            "semi;colon",
            "nul\0byte",
            "c:drive",
        ] {
            assert_eq!(
                history_db_path(
                    None,
                    Some(Path::new("/data")),
                    Path::new("/home/u"),
                    Some(escaping)
                ),
                None,
                "channel `{escaping}` must be refused"
            );
        }
    }

    #[test]
    fn unset_xdg_data_home_falls_back_to_home_local_share() {
        assert_eq!(
            history_db_path(None, None, Path::new("/home/u"), None),
            Some(PathBuf::from("/home/u/.local/share/opencode/opencode.db"))
        );
    }

    #[test]
    fn relative_xdg_data_home_is_ignored_per_the_xdg_rule() {
        assert_eq!(
            history_db_path(
                None,
                Some(Path::new("relative/data")),
                Path::new("/home/u"),
                Some("beta")
            ),
            Some(PathBuf::from(
                "/home/u/.local/share/opencode/opencode-beta.db"
            ))
        );
    }

    #[test]
    fn history_query_selects_from_allowlisted_tables_only() {
        let (refs, ctes) = referenced_tables(HISTORY_QUERY);
        let cte_names: std::collections::BTreeSet<&str> = ctes.iter().map(String::as_str).collect();
        let tables: std::collections::BTreeSet<&str> = refs
            .iter()
            .map(String::as_str)
            .filter(|table| !cte_names.contains(table))
            .collect();
        let allowlist: std::collections::BTreeSet<&str> = ["session_message", "session_v2"].into();
        assert_eq!(tables, allowlist, "reader SQL left the table allowlist");

        let words: std::collections::BTreeSet<String> = HISTORY_QUERY
            .to_ascii_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .filter(|word| !word.is_empty())
            .map(str::to_owned)
            .collect();
        for forbidden in [
            "account",
            "account_state",
            "control_account",
            "credential",
            "kv",
            "permission",
        ] {
            assert!(
                !words.contains(forbidden),
                "credential-adjacent table `{forbidden}` appears in the reader's SQL"
            );
        }
    }

    #[test]
    fn table_capture_flags_comma_lists_and_cte_bodies() {
        let (comma_refs, _) =
            referenced_tables("SELECT 1 FROM session_message AS m, credential WHERE m.id = 1");
        assert!(
            comma_refs.iter().any(|table| table == "credential"),
            "a comma-joined table must be captured, not only the first token after FROM"
        );

        let (cte_refs, ctes) = referenced_tables(
            "WITH leaked AS (SELECT * FROM credential) SELECT * FROM leaked JOIN session_v2",
        );
        assert_eq!(ctes, ["leaked"]);
        let cte_names: std::collections::BTreeSet<&str> = ctes.iter().map(String::as_str).collect();
        let real: Vec<&str> = cte_refs
            .iter()
            .map(String::as_str)
            .filter(|table| !cte_names.contains(table))
            .collect();
        assert_eq!(real, ["credential", "session_v2"]);
    }

    const CLAUSE_KEYWORDS: [&str; 15] = [
        "where", "on", "group", "order", "limit", "offset", "join", "left", "right", "inner",
        "outer", "cross", "natural", "full", "union",
    ];

    /// Every table reference after FROM/JOIN (comma lists and aliases
    /// included), plus every CTE name; a CTE is derived, so only its body's
    /// FROMs count as real tables.
    fn referenced_tables(sql: &str) -> (Vec<String>, Vec<String>) {
        let words = sql_words(sql);
        let mut refs = Vec::new();
        let mut ctes = Vec::new();
        let mut i = 0;
        while let Some(word) = words.get(i) {
            let keyword = word.to_ascii_lowercase();
            if keyword == "from" || keyword == "join" {
                i = tables_after_from(&words, i, &mut refs);
            } else if keyword == "with" {
                collect_cte_names(&words, i, &mut ctes);
                i += 1;
            } else {
                i += 1;
            }
        }
        (refs, ctes)
    }

    /// Walk index just past one FROM/JOIN clause: the first table (a
    /// subquery's inner FROMs are found by the caller's own walk), then any
    /// comma-joined tables up to the next clause keyword.
    fn tables_after_from(words: &[String], i: usize, refs: &mut Vec<String>) -> usize {
        let Some(first) = words.get(i + 1) else {
            return i + 1;
        };
        if first.as_str() != "(" {
            refs.push(first.clone());
        }
        comma_joined_tables(words, i + 2, refs)
    }

    fn comma_joined_tables(words: &[String], start: usize, refs: &mut Vec<String>) -> usize {
        let mut j = start;
        while let Some(token) = words.get(j) {
            let keyword = token.to_ascii_lowercase();
            if keyword == "," {
                push_unless_subquery(words, j + 1, refs);
                j += 2;
            } else if keyword == "as" {
                j += 2;
            } else if ends_from_list(&keyword, token) {
                return j;
            } else {
                j += 1;
            }
        }
        j
    }

    fn ends_from_list(keyword: &str, token: &str) -> bool {
        keyword == "select" || token == "(" || token == ")" || CLAUSE_KEYWORDS.contains(&keyword)
    }

    fn push_unless_subquery(words: &[String], at: usize, refs: &mut Vec<String>) {
        if let Some(name) = words.get(at).filter(|name| name.as_str() != "(") {
            refs.push(name.clone());
        }
    }

    fn collect_cte_names(words: &[String], with: usize, ctes: &mut Vec<String>) {
        let mut j = with + 1;
        if words
            .get(j)
            .is_some_and(|w| w.eq_ignore_ascii_case("recursive"))
        {
            j += 1;
        }
        while let Some(name) = words.get(j) {
            ctes.push(name.clone());
            let end = cte_body_end(words, j + 1);
            match words.get(end).map(String::as_str) {
                Some(",") => j = end + 1,
                _ => break,
            }
        }
    }

    /// Walk index just past one CTE's optional column list, `AS`, and body.
    fn cte_body_end(words: &[String], mut k: usize) -> usize {
        if words.get(k).map(String::as_str) == Some("(") {
            k = after_balanced_parens(words, k);
        }
        if words.get(k).is_some_and(|w| w.eq_ignore_ascii_case("as")) {
            k += 1;
        }
        if words.get(k).map(String::as_str) == Some("(") {
            k = after_balanced_parens(words, k);
        }
        k
    }

    fn after_balanced_parens(words: &[String], open: usize) -> usize {
        let mut depth = 0usize;
        for (i, word) in words.iter().enumerate().skip(open) {
            if word.as_str() == "(" {
                depth += 1;
            } else if word.as_str() == ")" {
                depth = depth.saturating_sub(1);
            } else {
                continue;
            }
            if depth == 0 {
                return i + 1;
            }
        }
        words.len()
    }

    fn sql_words(sql: &str) -> Vec<String> {
        let mut words = Vec::new();
        let mut current = String::new();
        for c in sql.chars() {
            if c.is_ascii_alphanumeric() || c == '_' {
                current.push(c);
                continue;
            }
            flush_word(&mut words, &mut current);
            if matches!(c, ',' | '(' | ')') {
                words.push(c.to_string());
            }
        }
        flush_word(&mut words, &mut current);
        words
    }

    fn flush_word(words: &mut Vec<String>, current: &mut String) {
        if !current.is_empty() {
            words.push(std::mem::take(current));
        }
    }
}
