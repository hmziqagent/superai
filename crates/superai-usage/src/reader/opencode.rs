//! opencode local v2 history database reader (USG-03), read-only. Only
//! `session_message` and `session_v2` are ever selected; credential tables
//! never leave the file.

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, Row};

use crate::cost::{RecordedUsage, UsageEvent};

/// Resolve the v2 history database per docs/harness-configs/opencode.md: an
/// `OPENCODE_DB` value wins, else `$XDG_DATA_HOME/opencode/` with a
/// channel-suffixed file name on non-default channels.
pub fn history_db_path(
    opencode_db_override: Option<&Path>,
    xdg_data_home: &Path,
    channel: Option<&str>,
) -> PathBuf {
    if let Some(path) = opencode_db_override {
        return path.to_path_buf();
    }
    let file_name = match channel {
        Some(channel) => format!("opencode-{channel}.db"),
        None => "opencode.db".to_owned(),
    };
    xdg_data_home.join("opencode").join(file_name)
}

/// Errors from the opencode v2 history scan.
#[derive(Debug, thiserror::Error)]
pub enum OpenCodeError {
    /// The database could not be opened read-only (missing, unreadable, or locked).
    #[error("opencode database open failed: {0}")]
    Open(#[from] rusqlite::Error),
    /// The history query failed.
    #[error("opencode history query failed: {0}")]
    Scan(rusqlite::Error),
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
/// and never writing; unmappable rows are counted, never fatal.
pub fn scan_history(db_path: &Path) -> Result<RecordedUsage, OpenCodeError> {
    let connection = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement = connection
        .prepare(HISTORY_QUERY)
        .map_err(OpenCodeError::Scan)?;
    let mut rows = statement.query([]).map_err(OpenCodeError::Scan)?;
    let mut usage = RecordedUsage::default();
    while let Some(row) = rows.next().map_err(OpenCodeError::Scan)? {
        match row_event(row) {
            Some(event) => usage.events.push(event),
            None => usage.unscanned_records += 1,
        }
    }
    Ok(usage)
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_value_wins_over_every_derived_path() {
        let r#override = Path::new("/elsewhere/custom.db");
        assert_eq!(
            history_db_path(Some(r#override), Path::new("/data"), Some("beta")),
            PathBuf::from("/elsewhere/custom.db")
        );
    }

    #[test]
    fn default_channel_uses_the_plain_file_name() {
        assert_eq!(
            history_db_path(None, Path::new("/data"), None),
            PathBuf::from("/data/opencode/opencode.db")
        );
    }

    #[test]
    fn non_default_channel_suffixed_file_name() {
        assert_eq!(
            history_db_path(None, Path::new("/data"), Some("beta")),
            PathBuf::from("/data/opencode/opencode-beta.db")
        );
    }

    #[test]
    fn history_query_selects_from_allowlisted_tables_only() {
        let allowlist = ["session_message", "session_v2"];
        let tokens: Vec<&str> = HISTORY_QUERY.split_whitespace().collect();
        let tables: Vec<String> = tokens
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                matches!(tokens.get(i.wrapping_sub(1)), Some(prev)
                    if prev.eq_ignore_ascii_case("from") || prev.eq_ignore_ascii_case("join"))
            })
            .map(|(_, token)| token.trim_matches(|c: char| !c.is_alphanumeric() && c != '_'))
            .filter(|table| !table.is_empty())
            .map(str::to_owned)
            .collect();
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
}
