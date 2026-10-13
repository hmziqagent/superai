//! `superai usage` subcommands (USG-06): local-artifact scans composed
//! into window and cost reports, one row per usage source.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::Serialize;
use superai_usage::cost::{
    CostEngine, CostPeriod, PricingCache, RateTable, RecordedUsage, Spend, UsageEvent,
};
use superai_usage::reader::jsonl::{
    PiSelectors, claude_projects_roots, codex_sessions_roots, pi_session_roots,
    scan_claude_transcripts, scan_codex_rollouts, scan_pi_sessions,
};
use superai_usage::reader::opencode::{OpenCodeError, history_db_path, scan_history};

use crate::CliError;

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
        print_json(&report)?;
    } else {
        print_show(&report);
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
        print_json(&report)?;
    } else {
        print_cost(&report);
    }
    Ok(())
}

fn print_json(report: &impl Serialize) -> Result<(), CliError> {
    let text = serde_json::to_string_pretty(report).map_err(|e| CliError::Scan {
        reason: format!("report encoding failed: {e}"),
    })?;
    println!("{text}");
    Ok(())
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
    words
        .next()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
        .ok_or_else(|| CliError::Args {
            reason: format!("{flag} needs a value"),
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
}

impl TruthSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::LocalArtifacts => "local artifacts",
        }
    }
}

/// One usage reader family: several harness ids can share a single
/// artifact store (oh-my-codex reads codex's rollouts), so the store is
/// scanned once and the row names every harness that shares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Claude,
    Codex,
    OpenCode,
    Pi,
}

impl Family {
    const ALL: [Self; 4] = [Self::Claude, Self::Codex, Self::OpenCode, Self::Pi];

    fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude-code",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
        }
    }

    fn harnesses(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["claude-code"],
            Self::Codex => &["codex-cli", "oh-my-codex"],
            Self::OpenCode => &["opencode", "oh-my-opencode"],
            Self::Pi => &["pi", "omp"],
        }
    }
}

fn select_families(harness: Option<&str>) -> Result<Vec<Family>, CliError> {
    let Some(harness) = harness else {
        return Ok(Family::ALL.to_vec());
    };
    Family::ALL
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
}

fn scan_family(family: Family, env: &UsageEnv) -> Result<SourceScan, CliError> {
    match family {
        Family::Claude => {
            let roots = claude_projects_roots(
                env.claude_config_dir.as_deref(),
                env.xdg_data_home.as_deref(),
                &env.home,
            );
            Ok(SourceScan {
                recorded: scan_claude_transcripts(&roots).into_recorded_usage(),
                notes: Vec::new(),
            })
        }
        Family::Codex => Ok(SourceScan {
            recorded: scan_codex_rollouts(&codex_sessions_roots(
                env.codex_home.as_deref(),
                &env.home,
            ))
            .into_recorded_usage(),
            notes: Vec::new(),
        }),
        // The channel is not env-selectable, so only the default database
        // or an OPENCODE_DB override is read (docs/harness-configs/opencode.md §6).
        Family::OpenCode => {
            let Some(path) = history_db_path(
                env.opencode_db.as_deref(),
                env.xdg_data_home.as_deref(),
                &env.home,
                None,
            ) else {
                return Ok(SourceScan {
                    recorded: RecordedUsage::default(),
                    notes: vec!["usage database path could not be resolved".to_owned()],
                });
            };
            match scan_history(&path) {
                Ok(recorded) => Ok(SourceScan {
                    recorded,
                    notes: Vec::new(),
                }),
                // A missing or locked database means no recorded usage, not
                // a failed query; the note keeps the row honest.
                Err(OpenCodeError::Open(e)) => Ok(SourceScan {
                    recorded: RecordedUsage::default(),
                    notes: vec![format!("history database could not be opened: {e}")],
                }),
                Err(OpenCodeError::Scan(e)) => Err(CliError::Scan {
                    reason: format!("opencode: {e}"),
                }),
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
                return Ok(SourceScan {
                    recorded: RecordedUsage::default(),
                    notes,
                });
            }
            if roots.profile_cap_hit {
                notes.push(
                    "more than 64 profiles were found; totals over the discovered set are a floor"
                        .to_owned(),
                );
            }
            Ok(SourceScan {
                recorded: scan_pi_sessions(&roots.roots).into_recorded_usage(),
                notes,
            })
        }
    }
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

fn token_totals(usage: &RecordedUsage) -> Result<TokenTotals, CliError> {
    fn sum(usage: &RecordedUsage, select: impl Fn(&UsageEvent) -> u64) -> Result<u64, CliError> {
        usage.events.iter().try_fold(0u64, |acc, event| {
            acc.checked_add(select(event)).ok_or(CliError::Scan {
                reason: "token totals overflow".to_owned(),
            })
        })
    }
    let events = u64::try_from(usage.events.len()).map_err(|_count| CliError::Scan {
        reason: "event count overflow".to_owned(),
    })?;
    Ok(TokenTotals {
        input: sum(usage, |event| event.input_tokens)?,
        output: sum(usage, |event| event.output_tokens)?,
        cache_read: sum(usage, |event| event.cache_read_tokens.unwrap_or(0))?,
        cache_write: sum(usage, |event| event.cache_write_tokens.unwrap_or(0))?,
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
    let mut sources = Vec::new();
    for family in families {
        let scan = scan_family(*family, env)?;
        let report = engine
            .total(
                &CostPeriod::AllHistory,
                now,
                zone_name(zone),
                &scan.recorded,
            )
            .map_err(|e| CliError::Pricing {
                reason: e.to_string(),
            })?;
        sources.push(SourceRow {
            source: family.id(),
            harnesses: family.harnesses().to_vec(),
            truth: TruthSource::LocalArtifacts,
            window: WindowRow {
                lane: "provider-only",
                resets_at: resets_at.clone(),
            },
            tokens: token_totals(&scan.recorded)?,
            coverage: CoverageView::of(&scan.recorded),
            cost: SpendView::of(&report.total.spend),
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
        let scan = scan_family(*family, env)?;
        let report = engine
            .total(&period, now, zone_name(zone), &scan.recorded)
            .map_err(|e| CliError::Pricing {
                reason: e.to_string(),
            })?;
        sources.push(CostSourceRow {
            source: family.id(),
            harnesses: family.harnesses().to_vec(),
            truth: TruthSource::LocalArtifacts,
            total: SpendView::of(&report.total.spend),
            coverage: CoverageView::of(&scan.recorded),
            models: report
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
                .collect(),
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

fn print_show(report: &ShowReport) {
    println!("window: {}", report.window);
    for row in &report.sources {
        println!(
            "{} [{}] — {}",
            row.source,
            row.harnesses.join(", "),
            TruthSource::LocalArtifacts.as_str()
        );
        println!(
            "  fill: provider-only; resets: {}",
            row.window.resets_at.as_deref().unwrap_or("unknown")
        );
        println!(
            "  tokens: in {} out {} cache-read {} cache-write {} events {}",
            row.tokens.input,
            row.tokens.output,
            row.tokens.cache_read,
            row.tokens.cache_write,
            row.tokens.events
        );
        print_spend_and_coverage(&row.cost, &row.coverage);
        print_notes(&row.notes);
    }
}

fn print_cost(report: &CostReport) {
    println!(
        "period: {}; range: {} — {}; zone: {}",
        report.period.human(),
        report
            .range
            .start
            .as_deref()
            .unwrap_or("beginning of records"),
        report.range.end,
        report.zone
    );
    for row in &report.sources {
        println!(
            "{} [{}] — {}",
            row.source,
            row.harnesses.join(", "),
            TruthSource::LocalArtifacts.as_str()
        );
        print_spend_and_coverage(&row.total, &row.coverage);
        for model in &row.models {
            println!(
                "  {}/{}: in {} out {} events {} cost {}",
                model.provider,
                model.model,
                model.input_tokens,
                model.output_tokens,
                model.events,
                model.cost.as_deref().unwrap_or("unpriced")
            );
        }
        print_notes(&row.notes);
    }
}

fn print_spend_and_coverage(spend: &SpendView, coverage: &CoverageView) {
    let amount = match (spend.state, spend.usd.as_deref()) {
        ("exact", Some(amount)) => format!("${amount}"),
        ("zero", _) => "0".to_owned(),
        _ => spend.missing_pricing.join(", "),
    };
    println!(
        "  cost: {amount} ({}); coverage: {}",
        spend.state, coverage.state
    );
    if coverage.unscanned_records > 0 {
        println!(
            "  {} records could not be scanned; totals are a floor",
            coverage.unscanned_records
        );
    }
}

fn print_notes(notes: &[String]) {
    for note in notes {
        println!("  note: {note}");
    }
}

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::fs;
    use std::hash::{Hash, Hasher};

    use superai_usage::cost::{ModelRates, Usd};

    // rusqlite is this package's dev-dependency; the bin itself never
    // opens a database, so this ack is all it gets.
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
        assert_eq!(all.len(), 4);

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
            &Family::ALL,
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
            &Family::ALL,
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
        });
        usage.events.push(UsageEvent {
            timestamp: ts("2026-10-13T12:00:01Z"),
            provider: "anthropic".to_owned(),
            model: "claude-sonnet-4-5".to_owned(),
            input_tokens: 1,
            output_tokens: 1,
            cache_read_tokens: None,
            cache_write_tokens: None,
        });

        assert!(err_text(token_totals(&usage)).contains("overflow"));
    }
}
