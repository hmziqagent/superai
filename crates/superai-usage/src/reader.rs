//! Local usage-artifact readers (USG-03): transcripts, rollouts, local
//! databases. Reading never writes; no credentials leave their file.

/// JSONL transcript and rollout scanning (claude-code, codex-cli, pi/omp).
pub mod jsonl;
/// Read-only scan of opencode's local v2 history database.
pub mod opencode;

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use crate::cost::{RecordedUsage, UsageEvent};

/// Defensive-scan tally for one scan (USG-03): unknown line shapes are
/// skipped and counted, never fatal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScanCounts {
    /// Lines read as records of the expected format, usage-bearing or not.
    pub parsed: u64,
    /// Lines that could not be interpreted: malformed bytes, foreign
    /// shapes, and usage records whose payload the reader could not read.
    pub skipped: u64,
}

/// One scan's result across every artifact a reader owns.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScanOutcome {
    /// Events parsed out of the scanned artifacts.
    pub events: Vec<UsageEvent>,
    /// Line tally across all scanned artifacts.
    pub counts: ScanCounts,
    /// Files and directories the reader could not open at all.
    pub unreadable_artifacts: u32,
}

impl ScanOutcome {
    /// The cost engine's input; skipped lines and unreadable artifacts
    /// make every period total a floor, never a silent zero.
    pub fn into_recorded_usage(self) -> RecordedUsage {
        RecordedUsage {
            events: self.events,
            unscanned_records: self
                .counts
                .skipped
                .saturating_add(u64::from(self.unreadable_artifacts)),
        }
    }
}

/// Record caps, one per surface, each matching its `CodexBar` scanner
/// (claude 512 KiB, codex 256 KiB, pi 16 MiB); a longer record is skipped
/// whole so a hostile file cannot exhaust memory.
pub(crate) const CLAUDE_LINE_CAP: usize = 512 * 1024;
/// Codex rollout records cap at 256 KiB (`CostUsageScanner.swift:4923`).
pub(crate) const CODEX_LINE_CAP: usize = 256 * 1024;
/// Pi session records cap at 16 MiB (`PiSessionCostScanner.swift:106`).
pub(crate) const PI_LINE_CAP: usize = 16 * 1024 * 1024;

enum RawLine {
    Complete(Vec<u8>),
    Oversized,
}

struct CappedLines<R> {
    reader: R,
    cap: usize,
}

impl<R: BufRead> CappedLines<R> {
    fn new(reader: R, cap: usize) -> Self {
        Self { reader, cap }
    }
}

enum Filled {
    Eof,
    Run(Vec<u8>),
    Newline(Vec<u8>),
}

impl<R: BufRead> CappedLines<R> {
    fn fill(&mut self) -> std::io::Result<Filled> {
        let buf = self.reader.fill_buf()?;
        if buf.is_empty() {
            return Ok(Filled::Eof);
        }
        if let Some(at) = buf.iter().position(|&b| b == b'\n') {
            let head: Vec<u8> = buf.iter().take(at).copied().collect();
            self.reader.consume(at + 1);
            return Ok(Filled::Newline(head));
        }
        let run = buf.to_vec();
        let len = run.len();
        self.reader.consume(len);
        Ok(Filled::Run(run))
    }

    fn absorb(&self, run: &[u8], line: &mut Vec<u8>, oversized: &mut bool) {
        if *oversized {
            return;
        }
        if line.len() + run.len() > self.cap {
            *oversized = true;
            line.clear();
            return;
        }
        line.extend_from_slice(run);
    }

    fn complete(&self, mut line: Vec<u8>, head: &[u8], oversized: bool) -> RawLine {
        if oversized || line.len() + head.len() > self.cap {
            return RawLine::Oversized;
        }
        line.extend_from_slice(head);
        RawLine::Complete(line)
    }
}

fn at_eof(line: Vec<u8>, oversized: bool) -> Option<RawLine> {
    if line.is_empty() && !oversized {
        return None;
    }
    Some(if oversized {
        RawLine::Oversized
    } else {
        RawLine::Complete(line)
    })
}

impl<R: BufRead> Iterator for CappedLines<R> {
    type Item = std::io::Result<RawLine>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut line = Vec::new();
        let mut oversized = false;
        loop {
            match self.fill() {
                Ok(Filled::Newline(head)) => {
                    return Some(Ok(self.complete(line, &head, oversized)));
                }
                Ok(Filled::Run(run)) => self.absorb(&run, &mut line, &mut oversized),
                Ok(Filled::Eof) => return at_eof(line, oversized).map(Ok),
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

/// Every `*.jsonl` file under `roots`, deduplicated by canonical path.
/// Symlinked entries are skipped (loop and escape hazard); missing roots
/// are known-empty; listing failures count as unreadable artifacts.
fn jsonl_files(roots: &[PathBuf], unreadable: &mut u32) -> Vec<PathBuf> {
    let mut seen_roots = BTreeSet::new();
    let mut files = BTreeSet::new();
    for root in roots {
        let Ok(root) = root.canonicalize() else {
            continue;
        };
        if seen_roots.insert(root.clone()) {
            walk_tree(root, &mut files, unreadable);
        }
    }
    files.into_iter().collect()
}

fn walk_tree(root: PathBuf, files: &mut BTreeSet<PathBuf>, unreadable: &mut u32) {
    let mut queue = vec![root];
    while let Some(dir) = queue.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            *unreadable += 1;
            continue;
        };
        let mut children = Vec::new();
        for entry in entries.flatten() {
            if let Some(path) = classify_entry(&entry, files) {
                children.push(path);
            }
        }
        children.sort();
        queue.extend(children);
    }
}

fn classify_entry(entry: &std::fs::DirEntry, files: &mut BTreeSet<PathBuf>) -> Option<PathBuf> {
    let name = entry.file_name();
    let name = name.to_str()?;
    if name.starts_with('.') {
        return None;
    }
    let Ok(kind) = entry.file_type() else {
        return None;
    };
    if kind.is_symlink() {
        return None;
    }
    let path = entry.path();
    if kind.is_dir() {
        return Some(path);
    }
    // Regular files only: opening a FIFO or device node would block the
    // scan forever (verified: File::open on a writerless FIFO hangs).
    if kind.is_file()
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
    {
        files.insert(path);
    }
    None
}

/// Scan one JSONL file, feeding every non-empty line to `on_line`; an
/// I/O failure mid-file counts one unreadable artifact and stops there.
fn for_each_line(
    path: &Path,
    outcome: &mut ScanOutcome,
    cap: usize,
    mut on_line: impl FnMut(&[u8], &mut ScanCounts),
) {
    let Ok(file) = File::open(path) else {
        outcome.unreadable_artifacts += 1;
        return;
    };
    for item in CappedLines::new(BufReader::new(file), cap) {
        match item {
            Ok(RawLine::Oversized) => outcome.counts.skipped += 1,
            Ok(RawLine::Complete(mut bytes)) => {
                trim_cr(&mut bytes);
                if !bytes.is_empty() {
                    on_line(&bytes, &mut outcome.counts);
                }
            }
            Err(_) => {
                outcome.unreadable_artifacts += 1;
                return;
            }
        }
    }
}

fn trim_cr(bytes: &mut Vec<u8>) {
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp(tag: &str, lines: &[String]) -> PathBuf {
        static DIR_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = DIR_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("superai-reader-unit-{tag}-{n}"));
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("temp dir: {e}"));
        let path = dir.join("lines.jsonl");
        std::fs::write(&path, lines.join("\n")).unwrap_or_else(|e| panic!("write: {e}"));
        path
    }

    fn scan(path: &Path) -> (ScanCounts, u32) {
        let mut outcome = ScanOutcome::default();
        for_each_line(path, &mut outcome, CLAUDE_LINE_CAP, |_, counts| {
            counts.parsed += 1;
        });
        (outcome.counts, outcome.unreadable_artifacts)
    }

    #[test]
    fn every_nonempty_line_reaches_the_callback_exactly_once() {
        let path = write_temp("lines", &["a".to_owned(), String::new(), "b".to_owned()]);
        let (counts, unreadable) = scan(&path);
        assert_eq!(counts.parsed, 2);
        assert_eq!(counts.skipped, 0);
        assert_eq!(unreadable, 0);
    }

    #[test]
    fn a_final_line_without_newline_is_still_a_line() {
        let path = write_temp("tail", &["tail-no-newline".to_owned()]);
        let (counts, _) = scan(&path);
        assert_eq!(counts.parsed, 1);
    }

    #[test]
    fn a_record_past_the_cap_is_skipped_and_does_not_exhaust_memory() {
        let huge = format!("x{}", "a".repeat(CLAUDE_LINE_CAP + 1));
        let path = write_temp("cap", &[huge, "after".to_owned()]);
        let (counts, _) = scan(&path);
        assert_eq!(counts.skipped, 1);
        assert_eq!(counts.parsed, 1);
    }
}
