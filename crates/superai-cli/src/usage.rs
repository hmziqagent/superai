//! `superai usage` subcommands (USG-06): local-artifact scans composed
//! into window and cost reports, one row per usage source.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::Serialize;
use superai_usage::cost::{
    CostEngine, CostPeriod, PeriodRange, PricingCache, RateTable, RecordedUsage, Spend, UsageEvent,
    Usd,
};
use superai_usage::llmproxy::{
    self, LedgerClient, RequestRow, RequestsQuery, load_admin_token, resolve_base_url,
};
use superai_usage::poller::PollLimits;
use superai_usage::reader::jsonl::{
    PiSelectors, claude_projects_roots, codex_sessions_roots, pi_session_roots,
    scan_claude_transcripts, scan_codex_rollouts, scan_pi_sessions,
};
use superai_usage::reader::opencode::{OpenCodeError, history_db_path, scan_history};

use crate::CliError;

/// Raw ledger rows one usage query asks for; more would silently floor
/// every total, so hitting the cap adds a note instead.
const LEDGER_ROW_CAP: usize = 10_000;

pub(crate) fn run(args: &[OsString], home: &Path) -> Result<(), CliError> {
    let (command, rest) = args.split_first().ok_or_else(|| CliError::Args {
        reason: "usage needs a subcommand: show or cost".to_owned(),
    })?;
    let command = command.to_str().ok_or_else(not_utf8)?;
    match command {
        "show" => run_show(rest, home),
        "cost" => run_cost(rest, home),
        other => Err(CliError::Args {
            reason: format!(
                "unknown usage subcommand `{}`; available: show, cost",
                crate::bounded(other)
            ),
        }),
    }
}

fn run_show(args: &[OsString], home: &Path) -> Result<(), CliError> {
    let parsed = ShowArgs::parse(args)?;
    let env = UsageEnv::capture(home)?;
    let families = select_families(parsed.harness.as_deref())?;
    let report = build_show(
        &env,
        &families,
        parsed.window,
        Timestamp::now(),
        &TimeZone::system(),
    )?;
    if parsed.json {
        write_json(&report)?;
    } else {
        crate::write_stdout(&render_show(&report))?;
    }
    Ok(())
}

fn run_cost(args: &[OsString], home: &Path) -> Result<(), CliError> {
    let parsed = CostArgs::parse(args)?;
    let env = UsageEnv::capture(home)?;
    let families = select_families(parsed.harness.as_deref())?;
    let report = build_cost(
        &env,
        &families,
        parsed.period,
        Timestamp::now(),
        &TimeZone::system(),
    )?;
    if parsed.json {
        write_json(&report)?;
    } else {
        crate::write_stdout(&render_cost(&report))?;
    }
    Ok(())
}

fn write_json(report: &impl Serialize) -> Result<(), CliError> {
    let text = serde_json::to_string_pretty(report).map_err(|e| CliError::Report {
        reason: format!("encoding failed: {e}"),
    })?;
    crate::write_stdout(&format!("{text}\n"))
}

fn not_utf8() -> CliError {
    CliError::Args {
        reason: "an argument is not valid UTF-8".to_owned(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShowArgs {
    harness: Option<String>,
    window: WindowSel,
    json: bool,
}

impl ShowArgs {
    fn parse(args: &[OsString]) -> Result<Self, CliError> {
        let mut harness: Option<String> = None;
        let mut window: Option<WindowSel> = None;
        let mut json = false;
        let mut words = args.iter();
        while let Some(word) = words.next() {
            match word.to_str().ok_or_else(not_utf8)? {
                "--json" => json = true,
                "--harness" => once(
                    &mut harness,
                    "--harness",
                    value_of(&mut words, "--harness")?,
                )?,
                "--window" => once(
                    &mut window,
                    "--window",
                    WindowSel::parse(&value_of(&mut words, "--window")?)?,
                )?,
                other => return Err(unknown_flag(other)),
            }
        }
        Ok(Self {
            harness,
            window: window.unwrap_or(WindowSel::Session),
            json,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CostArgs {
    harness: Option<String>,
    period: PeriodSel,
    json: bool,
}

impl CostArgs {
    fn parse(args: &[OsString]) -> Result<Self, CliError> {
        let mut harness: Option<String> = None;
        let mut days: Option<u16> = None;
        let mut period: Option<PeriodSel> = None;
        let mut json = false;
        let mut words = args.iter();
        while let Some(word) = words.next() {
            match word.to_str().ok_or_else(not_utf8)? {
                "--json" => json = true,
                "--harness" => once(
                    &mut harness,
                    "--harness",
                    value_of(&mut words, "--harness")?,
                )?,
                "--days" => once(
                    &mut days,
                    "--days",
                    day_count(&value_of(&mut words, "--days")?)?,
                )?,
                "--period" => once(
                    &mut period,
                    "--period",
                    period_sel(&value_of(&mut words, "--period")?)?,
                )?,
                other => return Err(unknown_flag(other)),
            }
        }
        let period = match (days, period) {
            (Some(_), Some(_)) => {
                return Err(CliError::Args {
                    reason: "--days and --period are mutually exclusive".to_owned(),
                });
            }
            (Some(days), None) => PeriodSel::Days(days),
            (None, Some(period)) => period,
            (None, None) => PeriodSel::MonthToDate,
        };
        Ok(Self {
            harness,
            period,
            json,
        })
    }
}

fn once<T>(slot: &mut Option<T>, flag: &str, value: T) -> Result<(), CliError> {
    if slot.is_some() {
        return Err(dup(flag));
    }
    *slot = Some(value);
    Ok(())
}

fn day_count(text: &str) -> Result<u16, CliError> {
    text.parse().map_err(|_not_a_number| CliError::Args {
        reason: "--days expects a whole number of days".to_owned(),
    })
}

fn period_sel(text: &str) -> Result<PeriodSel, CliError> {
    match text {
        "month-to-date" => Ok(PeriodSel::MonthToDate),
        "all" => Ok(PeriodSel::All),
        _ => Err(CliError::Args {
            reason: "--period expects month-to-date or all".to_owned(),
        }),
    }
}

fn dup(flag: &str) -> CliError {
    CliError::Args {
        reason: format!("{flag} given twice"),
    }
}

fn unknown_flag(flag: &str) -> CliError {
    CliError::Args {
        reason: format!("unknown flag `{}`", crate::bounded(flag)),
    }
}

fn value_of(words: &mut std::slice::Iter<'_, OsString>, flag: &str) -> Result<String, CliError> {
    let Some(value) = words.next() else {
        return Err(CliError::Args {
            reason: format!("{flag} needs a value"),
        });
    };
    value
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| CliError::Args {
            reason: format!("{flag} value is not valid UTF-8"),
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum WindowSel {
    Session,
    Week,
    Month,
}

impl WindowSel {
    fn parse(text: &str) -> Result<Self, CliError> {
        match text {
            "session" => Ok(Self::Session),
            "week" => Ok(Self::Week),
            "month" => Ok(Self::Month),
            _ => Err(CliError::Args {
                reason: "--window expects session, week, or month".to_owned(),
            }),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Week => "week",
            Self::Month => "month",
        }
    }

    /// Local artifacts cannot define a session boundary (USG-04), so the
    /// session window reports every recorded event and says so.
    fn totals_scope(self) -> &'static str {
        match self {
            Self::Session => "all-history",
            Self::Week => "last-7-days",
            Self::Month => "month-to-date",
        }
    }

    fn cost_period(self, zone: &TimeZone) -> Result<CostPeriod, CliError> {
        match self {
            Self::Session => Ok(CostPeriod::AllHistory),
            Self::Week => CostPeriod::rolling(7).map_err(|e| CliError::Pricing {
                reason: e.to_string(),
            }),
            Self::Month => Ok(CostPeriod::MonthToDate {
                time_zone: zone_name(zone).to_owned(),
            }),
        }
    }
}

impl std::fmt::Display for WindowSel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeriodSel {
    Days(u16),
    MonthToDate,
    All,
}

impl PeriodSel {
    fn to_cost_period(self, zone: &TimeZone) -> Result<CostPeriod, CliError> {
        match self {
            Self::Days(days) => CostPeriod::rolling(days).map_err(|e| CliError::Pricing {
                reason: e.to_string(),
            }),
            Self::MonthToDate => Ok(CostPeriod::MonthToDate {
                time_zone: zone_name(zone).to_owned(),
            }),
            Self::All => Ok(CostPeriod::AllHistory),
        }
    }

    fn echo(self) -> PeriodEcho {
        match self {
            Self::Days(days) => PeriodEcho {
                kind: "days",
                days: Some(days),
            },
            Self::MonthToDate => PeriodEcho {
                kind: "month-to-date",
                days: None,
            },
            Self::All => PeriodEcho {
                kind: "all",
                days: None,
            },
        }
    }
}

#[derive(Debug, Serialize)]
struct PeriodEcho {
    kind: &'static str,
    days: Option<u16>,
}

impl PeriodEcho {
    fn human(&self) -> String {
        match (self.kind, self.days) {
            ("days", Some(days)) => format!("last {days} days"),
            ("month-to-date", _) => "month to date".to_owned(),
            _ => "all history".to_owned(),
        }
    }
}

/// jiff zones without an IANA name (fixed offsets) have no name
/// `CostPeriod::MonthToDate` could re-resolve, so they report as UTC.
fn zone_name(zone: &TimeZone) -> &str {
    zone.iana_name().unwrap_or("UTC")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum TruthSource {
    LocalArtifacts,
    Llmproxy,
}

impl TruthSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::LocalArtifacts => "local artifacts",
            Self::Llmproxy => "llmproxy ledger",
        }
    }
}

/// One usage reader family: several harness ids can share a single
/// artifact store (oh-my-codex reads codex's rollouts), so the store is
/// scanned once and the row names every harness that shares it. The VPS
/// ledger is a family too (subplan 16 VPS-03): account-level, so it serves
/// no harness id and only joins unfiltered reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Claude,
    Codex,
    OpenCode,
    Pi,
    Ledger,
}

impl Family {
    const ALL: [Self; 5] = [
        Self::Claude,
        Self::Codex,
        Self::OpenCode,
        Self::Pi,
        Self::Ledger,
    ];

    fn local_families() -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|family| *family != Self::Ledger)
            .collect()
    }

    fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude-code",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
            Self::Ledger => "vps-ledger",
        }
    }

    fn harnesses(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["claude-code"],
            Self::Codex => &["codex-cli", "oh-my-codex"],
            Self::OpenCode => &["opencode", "oh-my-opencode"],
            Self::Pi => &["pi", "omp"],
            Self::Ledger => &[],
        }
    }

    fn truth(self) -> TruthSource {
        match self {
            Self::Ledger => TruthSource::Llmproxy,
            Self::Claude | Self::Codex | Self::OpenCode | Self::Pi => TruthSource::LocalArtifacts,
        }
    }
}

fn select_families(harness: Option<&str>) -> Result<Vec<Family>, CliError> {
    let Some(harness) = harness else {
        return Ok(Family::ALL.to_vec());
    };
    Family::local_families()
        .into_iter()
        .find(|family| family.harnesses().contains(&harness))
        .map(|family| vec![family])
        .ok_or_else(|| CliError::Args {
            reason: format!(
                "unknown harness `{}`; usage readers serve: claude-code, codex-cli, \
                 oh-my-codex, opencode, oh-my-opencode, pi, omp",
                crate::bounded(harness)
            ),
        })
}

#[derive(Debug, Default)]
struct UsageEnv {
    home: PathBuf,
    claude_config_dir: Option<String>,
    codex_home: Option<String>,
    xdg_data_home: Option<PathBuf>,
    opencode_db: Option<PathBuf>,
    pi_session_dir: Option<String>,
    pi_agent_dir: Option<String>,
    pi_config_dir: Option<String>,
    omp_profile: Option<String>,
    pi_profile: Option<String>,
    xdg_cache_home: Option<PathBuf>,
    llmproxy_url: Option<String>,
    llmproxy_token: Option<String>,
}

impl UsageEnv {
    fn capture(home: &Path) -> Result<Self, CliError> {
        Ok(Self {
            home: home.to_owned(),
            claude_config_dir: utf8_env("CLAUDE_CONFIG_DIR")?,
            codex_home: utf8_env("CODEX_HOME")?,
            xdg_data_home: path_env("XDG_DATA_HOME"),
            opencode_db: path_env("OPENCODE_DB"),
            pi_session_dir: utf8_env("PI_CODING_AGENT_SESSION_DIR")?,
            pi_agent_dir: utf8_env("PI_CODING_AGENT_DIR")?,
            pi_config_dir: utf8_env("PI_CONFIG_DIR")?,
            omp_profile: utf8_env("OMP_PROFILE")?,
            pi_profile: utf8_env("PI_PROFILE")?,
            xdg_cache_home: path_env("XDG_CACHE_HOME"),
            llmproxy_url: utf8_env(llmproxy::PROXY_URL_ENV_VAR)?,
            llmproxy_token: utf8_env(llmproxy::ADMIN_TOKEN_ENV_VAR)?,
        })
    }
}

/// Env values that feed path or selector strings must be UTF-8; the value
/// itself is never echoed back.
fn utf8_env(name: &str) -> Result<Option<String>, CliError> {
    let Some(value) = std::env::var_os(name) else {
        return Ok(None);
    };
    match value.into_string() {
        Ok(text) => Ok(Some(text)),
        Err(_not_utf8) => Err(CliError::Args {
            reason: format!("{name} is not valid UTF-8"),
        }),
    }
}

fn path_env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

struct SourceScan {
    recorded: RecordedUsage,
    notes: Vec<String>,
    /// Raw ledger rows; empty for every local family.
    ledger_rows: Vec<RequestRow>,
}

fn scan_family(family: Family, env: &UsageEnv) -> SourceScan {
    match family {
        Family::Claude => {
            let roots = claude_projects_roots(
                env.claude_config_dir.as_deref(),
                env.xdg_data_home.as_deref(),
                &env.home,
            );
            SourceScan {
                recorded: scan_claude_transcripts(&roots).into_recorded_usage(),
                notes: Vec::new(),
                ledger_rows: Vec::new(),
            }
        }
        Family::Codex => SourceScan {
            recorded: scan_codex_rollouts(&codex_sessions_roots(
                env.codex_home.as_deref(),
                &env.home,
            ))
            .into_recorded_usage(),
            notes: Vec::new(),
            ledger_rows: Vec::new(),
        },
        // The channel is not env-selectable, so only the default database
        // or an OPENCODE_DB override is read (docs/harness-configs/opencode.md §6).
        Family::OpenCode => {
            let Some(path) = history_db_path(
                env.opencode_db.as_deref(),
                env.xdg_data_home.as_deref(),
                &env.home,
                None,
            ) else {
                return SourceScan {
                    recorded: RecordedUsage::default(),
                    notes: vec!["usage database path could not be resolved".to_owned()],
                    ledger_rows: Vec::new(),
                };
            };
            // A database that fails before any row folds yields no recorded
            // usage here, not a failed report; mid-scan corruption keeps a floor.
            match scan_history(&path) {
                Ok(recorded) => SourceScan {
                    recorded,
                    notes: Vec::new(),
                    ledger_rows: Vec::new(),
                },
                Err(OpenCodeError::Open(e)) => SourceScan {
                    recorded: RecordedUsage::default(),
                    notes: vec![format!("history database could not be opened: {e}")],
                    ledger_rows: Vec::new(),
                },
                Err(OpenCodeError::Scan(e)) => SourceScan {
                    recorded: RecordedUsage::default(),
                    notes: vec![format!("history database could not be scanned: {e}")],
                    ledger_rows: Vec::new(),
                },
            }
        }
        Family::Pi => {
            let selectors = PiSelectors {
                session_dir: env.pi_session_dir.as_deref(),
                agent_dir: env.pi_agent_dir.as_deref(),
                config_dir: env.pi_config_dir.as_deref(),
                omp_profile: env.omp_profile.as_deref(),
                pi_profile: env.pi_profile.as_deref(),
                xdg_data_home: env.xdg_data_home.as_deref(),
            };
            let roots = pi_session_roots(&selectors, &env.home);
            let mut notes = Vec::new();
            if let Some(refusal) = roots.refusal {
                notes.push(refusal.to_owned());
                return SourceScan {
                    recorded: RecordedUsage::default(),
                    notes,
                    ledger_rows: Vec::new(),
                };
            }
            if roots.profile_cap_hit {
                notes.push(
                    "more than 64 profiles were found; totals over the discovered set are a floor"
                        .to_owned(),
                );
            }
            SourceScan {
                recorded: scan_pi_sessions(&roots.roots).into_recorded_usage(),
                notes,
                ledger_rows: Vec::new(),
            }
        }
        Family::Ledger => ledger_scan(env),
    }
}

/// The VPS ledger as a usage source (subplan 16 VPS-03): bounded raw rows
/// whose recorded cost is the ledger's own, never a rate estimate. Any
/// failure is a note on the row, never a failed report.
fn ledger_scan(env: &UsageEnv) -> SourceScan {
    let note = |reason: String| SourceScan {
        recorded: RecordedUsage::default(),
        notes: vec![reason],
        ledger_rows: Vec::new(),
    };
    let base = match resolve_base_url(None, env.llmproxy_url.as_deref(), &[]) {
        Ok(Some(base)) => base,
        Ok(None) => {
            return note(format!(
                "set {} (or point it at the deployment's admin listener) to read the ledger",
                llmproxy::PROXY_URL_ENV_VAR
            ));
        }
        Err(error) => return note(format!("ledger base url refused: {error}")),
    };
    let token_file = env.home.join(".superai").join("llmproxy-admin-token");
    let token = match load_admin_token(env.llmproxy_token.as_deref(), Some(&token_file)) {
        Ok(token) => token,
        Err(error) => return note(format!("ledger admin token unavailable: {error}")),
    };
    let limits = match PollLimits::new(Duration::from_secs(10), 1024 * 1024) {
        Ok(limits) => limits,
        Err(error) => return note(format!("ledger poll limits rejected: {error}")),
    };
    let client = match LedgerClient::new(&base, &token, limits) {
        Ok(client) => client,
        Err(error) => return note(format!("ledger client refused: {error}")),
    };
    let query = RequestsQuery {
        since: None,
        until: None,
        provider: None,
        limit: Some(u32::try_from(LEDGER_ROW_CAP).unwrap_or(u32::MAX)),
    };
    let rows = match client.requests(&query) {
        Ok(page) => page.rows,
        Err(error) => return note(format!("ledger could not be read: {error}")),
    };
    let capped = rows.len() >= LEDGER_ROW_CAP;
    let mut recorded = RecordedUsage::default();
    for row in &rows {
        // Rows without a timestamp cannot enter any window; rows without
        // provider or model cannot be attributed. Both stay counted skips.
        match (&row.provider, &row.model, row.received_at) {
            (Some(provider), Some(model), Some(at)) => recorded.events.push(UsageEvent {
                timestamp: at,
                provider: provider.clone(),
                model: model.clone(),
                input_tokens: row.input_tokens.unwrap_or(0),
                output_tokens: row.output_tokens.unwrap_or(0),
                cache_read_tokens: row.cache_read_tokens,
                cache_write_tokens: row.cache_write_tokens,
                cache_write_1h_tokens: None,
            }),
            _ => recorded.unscanned_records += 1,
        }
    }
    let mut notes = Vec::new();
    if capped {
        notes.push(format!(
            "ledger returned its row cap of {LEDGER_ROW_CAP}; totals over the returned rows are a floor"
        ));
    }
    SourceScan {
        recorded,
        notes,
        ledger_rows: rows,
    }
}

/// Ledger spend and per-model rows over one period: recorded costs only.
fn ledger_report(
    rows: &[RequestRow],
    range: &PeriodRange,
) -> Result<(Spend, Vec<ModelRow>), CliError> {
    #[derive(Default)]
    struct Group {
        input: u64,
        output: u64,
        events: u64,
        cost: Option<Usd>,
    }
    let mut groups: BTreeMap<(String, String), Group> = BTreeMap::new();
    let mut missing: BTreeSet<String> = BTreeSet::new();
    let mut total = Usd::ZERO;
    let mut matched = false;
    let overflow = || CliError::Scan {
        reason: "ledger totals overflow".to_owned(),
    };
    for row in rows {
        let Some(at) = row.received_at else {
            continue;
        };
        if !range.contains(at) {
            continue;
        }
        matched = true;
        let (Some(provider), Some(model)) = (&row.provider, &row.model) else {
            missing.insert("ledger/unattributed-row".to_owned());
            continue;
        };
        let entry = groups.entry((provider.clone(), model.clone())).or_default();
        entry.input = entry
            .input
            .checked_add(row.input_tokens.unwrap_or(0))
            .ok_or_else(overflow)?;
        entry.output = entry
            .output
            .checked_add(row.output_tokens.unwrap_or(0))
            .ok_or_else(overflow)?;
        entry.events = entry.events.checked_add(1).ok_or_else(overflow)?;
        if let Some(cost) = row.cost_usd {
            let sum = entry.cost.unwrap_or(Usd::ZERO);
            entry.cost = Some(sum.checked_add(cost).ok_or_else(overflow)?);
            total = total.checked_add(cost).ok_or_else(overflow)?;
        } else {
            entry.cost = None;
            missing.insert(format!("{provider}/{model}"));
        }
    }
    let spend = if !matched {
        Spend::Zero
    } else if missing.is_empty() {
        Spend::Exact(total)
    } else {
        Spend::Unavailable {
            missing_pricing: missing.into_iter().collect(),
        }
    };
    let models = groups
        .into_iter()
        .map(|((provider, model), group)| ModelRow {
            provider,
            model,
            input_tokens: group.input,
            output_tokens: group.output,
            events: group.events,
            cost: group.cost.map(|cost| cost.to_string()),
        })
        .collect();
    Ok((spend, models))
}

fn load_rates(env: &UsageEnv) -> Result<RateTable, CliError> {
    let bundled = RateTable::bundled().map_err(|e| CliError::Pricing {
        reason: e.to_string(),
    })?;
    let Ok(root) = PricingCache::default_root(env.xdg_cache_home.as_deref(), &env.home) else {
        return Ok(bundled);
    };
    let cache = PricingCache::new(&root);
    Ok(match cache.load() {
        Some(cached) => bundled.merge_additive(&cached.rates),
        None => bundled,
    })
}

#[derive(Debug, Serialize)]
struct ShowReport {
    window: WindowSel,
    sources: Vec<SourceRow>,
}

#[derive(Debug, Serialize)]
struct SourceRow {
    source: &'static str,
    harnesses: Vec<&'static str>,
    truth: TruthSource,
    window: WindowRow,
    totals_scope: &'static str,
    tokens: TokenTotals,
    coverage: CoverageView,
    cost: SpendView,
    notes: Vec<String>,
}

#[derive(Debug, Serialize)]
struct WindowRow {
    lane: &'static str,
    resets_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct TokenTotals {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    events: u64,
}

#[derive(Debug, Serialize)]
struct CoverageView {
    state: &'static str,
    unscanned_records: u64,
}

impl CoverageView {
    fn of(usage: &RecordedUsage) -> Self {
        if usage.unscanned_records > 0 {
            Self {
                state: "incomplete",
                unscanned_records: usage.unscanned_records,
            }
        } else {
            Self {
                state: "complete",
                unscanned_records: 0,
            }
        }
    }
}

#[derive(Debug, Serialize)]
struct SpendView {
    state: &'static str,
    usd: Option<String>,
    missing_pricing: Vec<String>,
}

impl SpendView {
    fn of(spend: &Spend) -> Self {
        match spend {
            Spend::Zero => Self {
                state: "zero",
                usd: None,
                missing_pricing: Vec::new(),
            },
            Spend::Exact(amount) => Self {
                state: "exact",
                usd: Some(amount.to_string()),
                missing_pricing: Vec::new(),
            },
            Spend::Unavailable { missing_pricing } => Self {
                state: "unavailable",
                usd: None,
                missing_pricing: missing_pricing.clone(),
            },
        }
    }
}

fn token_totals(usage: &RecordedUsage, range: &PeriodRange) -> Result<TokenTotals, CliError> {
    fn sum(events: &[&UsageEvent], select: impl Fn(&UsageEvent) -> u64) -> Result<u64, CliError> {
        events.iter().try_fold(0u64, |acc, event| {
            acc.checked_add(select(event)).ok_or(CliError::Scan {
                reason: "token totals overflow".to_owned(),
            })
        })
    }
    let scoped: Vec<&UsageEvent> = usage
        .events
        .iter()
        .filter(|event| range.contains(event.timestamp))
        .collect();
    let events = u64::try_from(scoped.len()).map_err(|_count| CliError::Scan {
        reason: "event count overflow".to_owned(),
    })?;
    Ok(TokenTotals {
        input: sum(&scoped, |event| event.input_tokens)?,
        output: sum(&scoped, |event| event.output_tokens)?,
        cache_read: sum(&scoped, |event| event.cache_read_tokens.unwrap_or(0))?,
        cache_write: sum(&scoped, |event| event.cache_write_tokens.unwrap_or(0))?,
        events,
    })
}

fn build_show(
    env: &UsageEnv,
    families: &[Family],
    window: WindowSel,
    now: Timestamp,
    zone: &TimeZone,
) -> Result<ShowReport, CliError> {
    let engine = CostEngine::new(load_rates(env)?);
    let resets_at = match window {
        WindowSel::Month => Some(next_month_start(now, zone)?),
        WindowSel::Session | WindowSel::Week => None,
    };
    let period = window.cost_period(zone)?;
    let range = period
        .range(now, zone_name(zone))
        .map_err(|e| CliError::Pricing {
            reason: e.to_string(),
        })?;
    let mut sources = Vec::new();
    for family in families {
        let scan = scan_family(*family, env);
        // The ledger's cost is its own recorded sum; local rows are priced
        // by the engine. The ledger endpoints report no quota lane at all.
        let cost = if *family == Family::Ledger {
            let (spend, _) = ledger_report(&scan.ledger_rows, &range)?;
            SpendView::of(&spend)
        } else {
            let report = engine
                .total(&period, now, zone_name(zone), &scan.recorded)
                .map_err(|e| CliError::Pricing {
                    reason: e.to_string(),
                })?;
            SpendView::of(&report.total.spend)
        };
        sources.push(SourceRow {
            source: family.id(),
            harnesses: family.harnesses().to_vec(),
            truth: family.truth(),
            window: WindowRow {
                lane: if *family == Family::Ledger {
                    "unavailable"
                } else {
                    "provider-only"
                },
                resets_at: resets_at.clone(),
            },
            totals_scope: window.totals_scope(),
            tokens: token_totals(&scan.recorded, &range)?,
            coverage: CoverageView::of(&scan.recorded),
            cost,
            notes: scan.notes,
        });
    }
    Ok(ShowReport { window, sources })
}

#[derive(Debug, Serialize)]
struct CostReport {
    period: PeriodEcho,
    range: RangeView,
    zone: String,
    sources: Vec<CostSourceRow>,
}

#[derive(Debug, Serialize)]
struct RangeView {
    start: Option<String>,
    end: String,
}

#[derive(Debug, Serialize)]
struct CostSourceRow {
    source: &'static str,
    harnesses: Vec<&'static str>,
    truth: TruthSource,
    total: SpendView,
    coverage: CoverageView,
    models: Vec<ModelRow>,
    notes: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ModelRow {
    provider: String,
    model: String,
    input_tokens: u64,
    output_tokens: u64,
    events: u64,
    cost: Option<String>,
}

fn build_cost(
    env: &UsageEnv,
    families: &[Family],
    period_sel: PeriodSel,
    now: Timestamp,
    zone: &TimeZone,
) -> Result<CostReport, CliError> {
    let period = period_sel.to_cost_period(zone)?;
    let range = period
        .range(now, zone_name(zone))
        .map_err(|e| CliError::Pricing {
            reason: e.to_string(),
        })?;
    let engine = CostEngine::new(load_rates(env)?);
    let mut sources = Vec::new();
    for family in families {
        let scan = scan_family(*family, env);
        let (total, models) = if *family == Family::Ledger {
            let (spend, models) = ledger_report(&scan.ledger_rows, &range)?;
            (SpendView::of(&spend), models)
        } else {
            let report = engine
                .total(&period, now, zone_name(zone), &scan.recorded)
                .map_err(|e| CliError::Pricing {
                    reason: e.to_string(),
                })?;
            let models = report
                .models
                .iter()
                .map(|row| ModelRow {
                    provider: row.provider.clone(),
                    model: row.model.clone(),
                    input_tokens: row.input_tokens,
                    output_tokens: row.output_tokens,
                    events: row.events,
                    cost: row.cost.map(|amount| amount.to_string()),
                })
                .collect();
            (SpendView::of(&report.total.spend), models)
        };
        sources.push(CostSourceRow {
            source: family.id(),
            harnesses: family.harnesses().to_vec(),
            truth: family.truth(),
            total,
            coverage: CoverageView::of(&scan.recorded),
            models,
            notes: scan.notes,
        });
    }
    Ok(CostReport {
        period: period_sel.echo(),
        range: RangeView {
            start: range.start.map(|start| start.to_string()),
            end: range.end.to_string(),
        },
        zone: zone_name(zone).to_owned(),
        sources,
    })
}

/// Calendar-month windows reset at the first instant of next month in the
/// display zone; session and rolling weeks have no locally documented
/// reset instant, so they report none instead of inventing one.
fn next_month_start(now: Timestamp, zone: &TimeZone) -> Result<String, CliError> {
    now.to_zoned(zone.clone())
        .start_of_day()
        .and_then(|day| day.with().day(1).build())
        .and_then(|first| first.checked_add(jiff::Span::new().months(1)))
        .map(|reset| reset.to_string())
        .map_err(|e| CliError::Window {
            reason: e.to_string(),
        })
}

fn render_show(report: &ShowReport) -> String {
    let mut lines = vec![format!("window: {}", report.window)];
    for row in &report.sources {
        lines.push(format!(
            "{} [{}] {}",
            row.source,
            row.harnesses.join(", "),
            row.truth.as_str()
        ));
        lines.push(format!(
            "  fill: {}; resets: {}",
            row.window.lane,
            row.window.resets_at.as_deref().unwrap_or("unknown")
        ));
        lines.push(format!("  totals: {}", row.totals_scope));
        lines.push(format!(
            "  tokens: in {} out {} cache-read {} cache-write {} events {}",
            row.tokens.input,
            row.tokens.output,
            row.tokens.cache_read,
            row.tokens.cache_write,
            row.tokens.events
        ));
        lines.extend(spend_lines(&row.cost, &row.coverage));
        lines.extend(note_lines(&row.notes));
    }
    crate::finish_lines(lines)
}

fn render_cost(report: &CostReport) -> String {
    let mut lines = vec![format!(
        "period: {}; range: {} to {}; zone: {}",
        report.period.human(),
        report
            .range
            .start
            .as_deref()
            .unwrap_or("beginning of records"),
        report.range.end,
        report.zone
    )];
    for row in &report.sources {
        lines.push(format!(
            "{} [{}] {}",
            row.source,
            row.harnesses.join(", "),
            row.truth.as_str()
        ));
        lines.extend(spend_lines(&row.total, &row.coverage));
        for model in &row.models {
            lines.push(format!(
                "  {}/{}: in {} out {} events {} cost {}",
                model.provider,
                model.model,
                model.input_tokens,
                model.output_tokens,
                model.events,
                model.cost.as_deref().unwrap_or("unpriced")
            ));
        }
        lines.extend(note_lines(&row.notes));
    }
    crate::finish_lines(lines)
}

fn spend_lines(spend: &SpendView, coverage: &CoverageView) -> Vec<String> {
    let amount = match (spend.state, spend.usd.as_deref()) {
        ("exact", Some(amount)) => format!("${amount}"),
        ("zero", _) => "0".to_owned(),
        _ => spend.missing_pricing.join(", "),
    };
    let mut lines = vec![format!(
        "  cost: {amount} ({}); coverage: {}",
        spend.state, coverage.state
    )];
    if coverage.unscanned_records > 0 {
        lines.push(format!(
            "  {} records could not be scanned; totals are a floor",
            coverage.unscanned_records
        ));
    }
    lines
}

fn note_lines(notes: &[String]) -> Vec<String> {
    notes.iter().map(|note| format!("  note: {note}")).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::fs;
    use std::hash::{Hash, Hasher};

    use superai_usage::cost::ModelRates;

    // unused_crate_dependencies demands an ack for the dev-dependency.
    use rusqlite as _;

    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn err_text(result: Result<impl Sized, CliError>) -> String {
        match result {
            Ok(_) => panic!("expected an error"),
            Err(e) => e.to_string(),
        }
    }

    fn temp_home(tag: &str) -> PathBuf {
        let mut hasher = DefaultHasher::new();
        std::process::id().hash(&mut hasher);
        tag.hash(&mut hasher);
        let dir = std::env::temp_dir().join(format!(
            "superai-cli-unit-{}-{:08x}",
            std::process::id(),
            hasher.finish() & 0xffff_ffff
        ));
        fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("temp home {}: {e}", dir.display()));
        dir
    }

    fn ts(text: &str) -> Timestamp {
        text.parse::<Timestamp>()
            .unwrap_or_else(|e| panic!("timestamp `{text}`: {e}"))
    }

    #[test]
    fn show_args_default_to_the_session_window() {
        let parsed = ShowArgs::parse(&os(&["--json"])).unwrap_or_else(|e| panic!("parse: {e}"));

        assert_eq!(parsed.window, WindowSel::Session);
        assert!(parsed.json);
        assert_eq!(parsed.harness, None);
    }

    #[test]
    fn show_args_parse_every_flag() {
        let parsed = ShowArgs::parse(&os(&["--harness", "pi", "--window", "week", "--json"]))
            .unwrap_or_else(|e| panic!("parse: {e}"));

        assert_eq!(parsed.harness.as_deref(), Some("pi"));
        assert_eq!(parsed.window, WindowSel::Week);
        assert!(parsed.json);
    }

    #[test]
    fn show_args_reject_unknown_and_malformed_input() {
        assert!(
            err_text(ShowArgs::parse(&os(&["--window", "fortnight"])))
                .contains("session, week, or month")
        );
        assert!(err_text(ShowArgs::parse(&os(&["--wat"]))).contains("unknown flag"));
        assert!(err_text(ShowArgs::parse(&os(&["--window"]))).contains("needs a value"));
        assert!(
            err_text(ShowArgs::parse(&os(&[
                "--harness",
                "pi",
                "--harness",
                "omp"
            ])))
            .contains("given twice")
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_present_but_non_utf8_flag_value_is_not_reported_as_missing() {
        use std::os::unix::ffi::OsStringExt;
        let args = vec![
            OsString::from("--harness"),
            OsString::from_vec(vec![0xff, 0xfe]),
        ];

        let text = err_text(ShowArgs::parse(&args));

        assert!(text.contains("not valid UTF-8"), "{text}");
        assert!(!text.contains("needs a value"), "{text}");
    }

    #[test]
    fn cost_args_default_to_month_to_date() {
        let parsed = CostArgs::parse(&[]).unwrap_or_else(|e| panic!("parse: {e}"));

        assert_eq!(parsed.period, PeriodSel::MonthToDate);
        assert!(!parsed.json);
    }

    #[test]
    fn cost_args_take_days_or_period_but_not_both() {
        let days = CostArgs::parse(&os(&["--days", "7"])).unwrap_or_else(|e| panic!("parse: {e}"));
        assert_eq!(days.period, PeriodSel::Days(7));

        let all = CostArgs::parse(&os(&["--period", "all", "--json"]))
            .unwrap_or_else(|e| panic!("parse: {e}"));
        assert_eq!(all.period, PeriodSel::All);
        assert!(all.json);

        assert!(
            err_text(CostArgs::parse(&os(&["--days", "1", "--period", "all"])))
                .contains("mutually exclusive")
        );
        assert!(err_text(CostArgs::parse(&os(&["--days", "chocolate"]))).contains("whole number"));
    }

    #[test]
    fn out_of_range_day_counts_fail_period_construction() {
        let zone = TimeZone::UTC;
        for days in [0u16, 366] {
            let result = PeriodSel::Days(days).to_cost_period(&zone);
            assert!(
                err_text(result).contains("outside 1..=365"),
                "day count {days}"
            );
        }
    }

    #[test]
    fn every_served_harness_maps_to_a_family() {
        let expected = [
            ("claude-code", "claude-code"),
            ("codex-cli", "codex"),
            ("oh-my-codex", "codex"),
            ("opencode", "opencode"),
            ("oh-my-opencode", "opencode"),
            ("pi", "pi"),
            ("omp", "pi"),
        ];
        for (harness, source) in expected {
            let families = select_families(Some(harness)).unwrap_or_else(|e| panic!("select: {e}"));
            assert_eq!(families.len(), 1, "{harness}");
            assert_eq!(families[0].id(), source, "{harness}");
        }

        let all = select_families(None).unwrap_or_else(|e| panic!("select: {e}"));
        assert_eq!(all.len(), 5);
        assert!(
            all.contains(&Family::Ledger),
            "the ledger must join unfiltered reports"
        );

        let unknown = err_text(select_families(Some("nope")));
        assert!(unknown.contains("unknown harness"));
        assert!(unknown.contains("oh-my-opencode"));
    }

    #[test]
    fn next_month_start_lands_on_the_first_instant_of_next_month() {
        let utc = TimeZone::UTC;
        let reset =
            next_month_start(ts("2026-10-13T12:34:56Z"), &utc).unwrap_or_else(|e| panic!("{e}"));
        assert!(reset.starts_with("2026-11-01T00:00:00"), "{reset}");

        let new_years =
            next_month_start(ts("2026-12-31T23:59:59Z"), &utc).unwrap_or_else(|e| panic!("{e}"));
        assert!(new_years.starts_with("2027-01-01T00:00:00"), "{new_years}");

        let eastern = TimeZone::get("America/New_York").unwrap_or_else(|e| panic!("zone: {e}"));
        let reset = next_month_start(ts("2026-07-04T12:00:00Z"), &eastern)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(reset.contains("2026-08-01T00:00:00"), "{reset}");
    }

    #[test]
    fn session_and_week_windows_report_no_locally_computed_reset() {
        let env = UsageEnv::default();
        let report = build_show(
            &env,
            &Family::local_families(),
            WindowSel::Session,
            ts("2026-10-13T12:00:00Z"),
            &TimeZone::UTC,
        )
        .unwrap_or_else(|e| panic!("build: {e}"));

        assert_eq!(report.window, WindowSel::Session);
        for row in &report.sources {
            assert_eq!(row.window.lane, "provider-only");
            assert_eq!(row.window.resets_at, None);
        }
    }

    #[test]
    fn month_windows_carry_the_computed_reset_on_every_row() {
        let env = UsageEnv::default();
        let report = build_show(
            &env,
            &Family::local_families(),
            WindowSel::Month,
            ts("2026-10-13T12:00:00Z"),
            &TimeZone::UTC,
        )
        .unwrap_or_else(|e| panic!("build: {e}"));

        for row in &report.sources {
            let reset = row.window.resets_at.as_deref().unwrap_or("missing");
            assert!(reset.starts_with("2026-11-01T00:00:00"), "{reset}");
        }
    }

    #[test]
    fn an_unconfigured_ledger_joins_the_report_as_a_noted_row() {
        let env = UsageEnv::default();
        let report = build_show(
            &env,
            &Family::ALL,
            WindowSel::Session,
            ts("2026-10-13T12:00:00Z"),
            &TimeZone::UTC,
        )
        .unwrap_or_else(|e| panic!("build: {e}"));

        assert_eq!(report.sources.len(), 5);
        let ledger = report
            .sources
            .iter()
            .find(|row| row.source == "vps-ledger")
            .unwrap_or_else(|| panic!("no ledger row: {:?}", report.sources));
        assert_eq!(ledger.truth, TruthSource::Llmproxy);
        assert_eq!(ledger.harnesses, Vec::<&str>::new());
        assert_eq!(ledger.window.lane, "unavailable");
        assert_eq!(ledger.tokens.events, 0);
        assert_eq!(ledger.cost.state, "zero");
        assert!(
            ledger
                .notes
                .first()
                .is_some_and(|note| note.contains("SUPERAI_LLMPROXY_URL")),
            "unconfigured note missing: {:?}",
            ledger.notes
        );
        assert!(
            report
                .sources
                .iter()
                .any(|row| row.truth == TruthSource::LocalArtifacts),
            "local rows must still flow"
        );
    }

    fn ledger_row(
        at: &str,
        provider: Option<&str>,
        model: Option<&str>,
        input: u64,
        output: u64,
        cost: Option<&str>,
    ) -> RequestRow {
        RequestRow {
            request_id: None,
            received_at: Some(ts(at)),
            completed_at: None,
            provider: provider.map(str::to_owned),
            model: model.map(str::to_owned),
            upstream_model: None,
            route_kind: None,
            client_protocol: None,
            streaming: None,
            caller: None,
            input_tokens: (input > 0).then_some(input),
            output_tokens: (output > 0).then_some(output),
            cache_read_tokens: None,
            cache_write_tokens: None,
            cost_usd: cost
                .map(|text| Usd::parse(text).unwrap_or_else(|e| panic!("cost `{text}`: {e}"))),
            stop_reason: None,
            error_kind: None,
            http_status: None,
            latency_ms: None,
            body_hash: None,
        }
    }

    #[test]
    fn ledger_report_scopes_rows_and_keeps_recorded_costs_exact() {
        let rows = vec![
            ledger_row(
                "2026-10-13T11:00:00Z",
                Some("anthropic"),
                Some("claude-sonnet-4-5"),
                100,
                50,
                Some("0.001"),
            ),
            ledger_row(
                "2026-01-15T09:00:00Z",
                Some("openai"),
                Some("gpt-5.1"),
                10,
                5,
                Some("0.0005"),
            ),
            ledger_row("2026-01-15T09:06:00Z", None, None, 0, 0, None),
        ];
        let week = PeriodRange {
            start: Some(ts("2026-10-07T00:00:00Z")),
            end: ts("2026-10-13T12:00:00Z"),
        };

        let (spend, models) =
            ledger_report(&rows, &week).unwrap_or_else(|e| panic!("ledger report: {e}"));

        assert_eq!(
            spend,
            Spend::Exact(Usd::parse("0.001").unwrap_or_else(|e| panic!("sum: {e}")))
        );
        let row = models.first().unwrap_or_else(|| panic!("model row"));
        assert_eq!(row.provider, "anthropic");
        assert_eq!(row.input_tokens, 100);
        assert_eq!(row.output_tokens, 50);
        assert_eq!(row.events, 1);
        assert_eq!(row.cost.as_deref(), Some("0.001"));

        let all = PeriodRange {
            start: None,
            end: ts("2026-10-13T12:00:00Z"),
        };
        let (spend, models) =
            ledger_report(&rows, &all).unwrap_or_else(|e| panic!("ledger report: {e}"));
        assert_eq!(
            spend,
            Spend::Unavailable {
                missing_pricing: vec!["ledger/unattributed-row".to_owned()]
            }
        );
        assert_eq!(models.len(), 2);
        let openai = models
            .iter()
            .find(|row| row.provider == "openai")
            .unwrap_or_else(|| panic!("openai row: {models:?}"));
        assert_eq!(openai.cost.as_deref(), Some("0.0005"));
    }

    #[test]
    fn build_show_reports_tokens_exact_cost_and_coverage() {
        let home = temp_home("show-build");
        let corpus = home
            .join(".claude")
            .join("projects")
            .join("p")
            .join("s.jsonl");
        fs::create_dir_all(corpus.parent().unwrap_or_else(|| panic!("parent")))
            .unwrap_or_else(|e| panic!("mkdir: {e}"));
        fs::write(
            &corpus,
            concat!(
                r#"{"type":"assistant","timestamp":"2026-10-13T09:00:05Z","sessionId":"s1","#,
                r#""message":{"id":"m1","model":"claude-sonnet-4-5","stop_reason":"end_turn","#,
                r#""usage":{"input_tokens":100,"cache_read_input_tokens":50,"#,
                r#""cache_creation_input_tokens":10,"output_tokens":200}}}"#,
                "\nthis line is {broken\n"
            ),
        )
        .unwrap_or_else(|e| panic!("corpus: {e}"));
        let env = UsageEnv {
            home,
            ..UsageEnv::default()
        };

        let report = build_show(
            &env,
            &[Family::Claude],
            WindowSel::Session,
            ts("2026-10-13T12:00:00Z"),
            &TimeZone::UTC,
        )
        .unwrap_or_else(|e| panic!("build: {e}"));

        let row = report.sources.first().unwrap_or_else(|| panic!("row"));
        assert_eq!(row.source, "claude-code");
        assert_eq!(row.truth, TruthSource::LocalArtifacts);
        assert_eq!(row.totals_scope, "all-history");
        assert_eq!(row.tokens.input, 100);
        assert_eq!(row.tokens.output, 200);
        assert_eq!(row.tokens.cache_read, 50);
        assert_eq!(row.tokens.cache_write, 10);
        assert_eq!(row.tokens.events, 1);
        assert_eq!(row.cost.state, "exact");
        assert_eq!(row.cost.usd.as_deref(), Some("0.0033525"));
        assert_eq!(row.coverage.state, "incomplete");
        assert_eq!(row.coverage.unscanned_records, 1);
    }

    #[test]
    fn week_and_month_windows_scope_totals_while_session_reads_history() {
        let home = temp_home("window-scope");
        let corpus = home
            .join(".claude")
            .join("projects")
            .join("p")
            .join("s.jsonl");
        fs::create_dir_all(corpus.parent().unwrap_or_else(|| panic!("parent")))
            .unwrap_or_else(|e| panic!("mkdir: {e}"));
        fs::write(
            &corpus,
            concat!(
                r#"{"type":"assistant","timestamp":"2026-10-13T11:00:00Z","sessionId":"s1","#,
                r#""message":{"id":"m1","model":"claude-sonnet-4-5","stop_reason":"end_turn","#,
                r#""usage":{"input_tokens":100,"cache_read_input_tokens":50,"#,
                r#""cache_creation_input_tokens":10,"output_tokens":200}}}"#,
                "\n",
                r#"{"type":"assistant","timestamp":"2026-09-01T00:00:00Z","sessionId":"s1","#,
                r#""message":{"id":"m2","model":"claude-sonnet-4-5","stop_reason":"end_turn","#,
                r#""usage":{"input_tokens":1,"output_tokens":1}}}"#,
                "\n"
            ),
        )
        .unwrap_or_else(|e| panic!("corpus: {e}"));
        let env = UsageEnv {
            home,
            ..UsageEnv::default()
        };
        let now = ts("2026-10-13T12:00:00Z");

        for (window, scope, events, cost) in [
            (WindowSel::Session, "all-history", 2, "0.0033705"),
            (WindowSel::Week, "last-7-days", 1, "0.0033525"),
            (WindowSel::Month, "month-to-date", 1, "0.0033525"),
        ] {
            let report = build_show(&env, &[Family::Claude], window, now, &TimeZone::UTC)
                .unwrap_or_else(|e| panic!("build {window}: {e}"));
            let row = report
                .sources
                .first()
                .unwrap_or_else(|| panic!("row for {window}"));
            assert_eq!(row.totals_scope, scope, "window {window}");
            assert_eq!(row.tokens.events, events, "window {window}");
            assert_eq!(row.tokens.input, if events == 2 { 101 } else { 100 });
            assert_eq!(row.cost.state, "exact", "window {window}");
            assert_eq!(row.cost.usd.as_deref(), Some(cost), "window {window}");
        }
    }

    #[test]
    fn cached_catalog_overlays_bundled_rates() {
        let cache_base = temp_home("rates");
        let mut fetched = RateTable::default();
        fetched.insert(
            "anthropic",
            "claude-dr-4",
            ModelRates {
                input: Usd::parse_rate_per_mtok("1").unwrap_or_else(|e| panic!("rate: {e}")),
                output: Usd::parse_rate_per_mtok("5").unwrap_or_else(|e| panic!("rate: {e}")),
                cache_read: None,
                cache_write: None,
            },
        );
        let cache = PricingCache::new(&cache_base.join("superai"));
        cache
            .store(&fetched, ts("2026-10-13T10:00:00Z"))
            .unwrap_or_else(|e| panic!("store: {e}"));

        let env = UsageEnv {
            xdg_cache_home: Some(cache_base),
            ..UsageEnv::default()
        };
        let rates = load_rates(&env).unwrap_or_else(|e| panic!("rates: {e}"));

        assert!(rates.lookup("anthropic", "claude-dr-4").is_some());
        assert!(rates.lookup("anthropic", "claude-sonnet-4-5").is_some());
    }

    #[test]
    fn a_missing_cache_falls_back_to_bundled_rates() {
        let env = UsageEnv {
            xdg_cache_home: Some(PathBuf::from("/nonexistent-superai-test-cache")),
            ..UsageEnv::default()
        };
        let rates = load_rates(&env).unwrap_or_else(|e| panic!("rates: {e}"));

        assert!(rates.lookup("anthropic", "claude-sonnet-4-5").is_some());
        assert!(rates.lookup("anthropic", "claude-dr-4").is_none());
    }

    #[test]
    fn json_reports_serialize_the_documented_shapes() {
        let env = UsageEnv::default();
        let show = build_show(
            &env,
            &[Family::Claude],
            WindowSel::Week,
            ts("2026-10-13T12:00:00Z"),
            &TimeZone::UTC,
        )
        .unwrap_or_else(|e| panic!("build: {e}"));
        let json = serde_json::to_value(&show).unwrap_or_else(|e| panic!("json: {e}"));
        assert_eq!(json["window"], "week");
        assert_eq!(json["sources"][0]["truth"], "local-artifacts");
        assert_eq!(json["sources"][0]["window"]["lane"], "provider-only");

        let cost = build_cost(
            &env,
            &[Family::Codex],
            PeriodSel::Days(7),
            ts("2026-10-13T12:00:00Z"),
            &TimeZone::UTC,
        )
        .unwrap_or_else(|e| panic!("build: {e}"));
        let json = serde_json::to_value(&cost).unwrap_or_else(|e| panic!("json: {e}"));
        assert_eq!(json["period"]["kind"], "days");
        assert_eq!(json["period"]["days"], 7);
        assert_eq!(json["zone"], "UTC");
        assert_eq!(json["sources"][0]["source"], "codex");
    }

    #[test]
    fn token_totals_refuse_to_overflow() {
        let mut usage = RecordedUsage::default();
        usage.events.push(UsageEvent {
            timestamp: ts("2026-10-13T12:00:00Z"),
            provider: "anthropic".to_owned(),
            model: "claude-sonnet-4-5".to_owned(),
            input_tokens: u64::MAX,
            output_tokens: 1,
            cache_read_tokens: None,
            cache_write_tokens: None,
            cache_write_1h_tokens: None,
        });
        usage.events.push(UsageEvent {
            timestamp: ts("2026-10-13T12:00:01Z"),
            provider: "anthropic".to_owned(),
            model: "claude-sonnet-4-5".to_owned(),
            input_tokens: 1,
            output_tokens: 1,
            cache_read_tokens: None,
            cache_write_tokens: None,
            cache_write_1h_tokens: None,
        });

        let range = PeriodRange {
            start: None,
            end: ts("2026-10-13T13:00:00Z"),
        };

        assert!(err_text(token_totals(&usage, &range)).contains("overflow"));
    }
}
