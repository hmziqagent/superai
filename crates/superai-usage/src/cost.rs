//! Cost periods and pricing metadata (USG-05): rolling windows,
//! month-to-date, all-history. Recorded spend is never a bill.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use superai_core::template_fetch::{
    BoundedFetch, FETCH_TIMEOUT_SECS, TemplateFetchError, fetch_bounded, validate_fetch_url,
};

/// Seconds a cached models.dev catalog stays fresh.
pub const CACHE_TTL_SECS: i64 = 24 * 60 * 60;

/// Minimum age of the cache before an unknown-model lookup may trigger
/// another refresh attempt.
pub const UNKNOWN_MODEL_COOLDOWN_SECS: i64 = 15 * 60;

/// Largest models.dev response accepted, bytes.
pub const MAX_MODELS_DEV_BYTES: usize = 8 * 1024 * 1024;

/// Largest cache file read from disk, bytes; larger files are corrupt.
pub const MAX_CACHE_FILE_BYTES: usize = 32 * 1024 * 1024;

/// models.dev's published catalog endpoint; no API key.
pub const MODELS_DEV_API_URL: &str = "https://models.dev/api.json";

const CACHE_SCHEMA_VERSION: u16 = 1;
const PICOS_PER_USD: i128 = 1_000_000_000_000;
const TOKENS_PER_MILLION: i128 = 1_000_000;

/// Everything the cost engine and pricing registry can fail with.
#[derive(Debug, thiserror::Error)]
pub enum CostError {
    /// Zone name jiff could not resolve.
    #[error("unknown time zone `{name}`")]
    UnknownZone {
        /// The unresolved name.
        name: String,
    },
    /// A period boundary does not exist in its zone (DST can delete a
    /// midnight) or the date arithmetic overflowed.
    #[error("cannot resolve period boundary in `{zone}`: {reason}")]
    PeriodBoundary {
        /// Zone the boundary was resolved in.
        zone: String,
        /// Display-safe reason.
        reason: String,
    },
    /// A decimal that is not a plain number (`-?[0-9]+(\.[0-9]+)?([eE][-+]?[0-9]+)?`).
    #[error("decimal `{text}` is not a plain decimal number")]
    MalformedDecimal {
        /// Offending text, truncated for display.
        text: String,
    },
    /// A parseable decimal that needs more precision than the money scale
    /// (or the per-million rate scale) can represent exactly.
    #[error("decimal `{text}` is too precise to represent exactly")]
    DecimalTooPrecise {
        /// Offending text, truncated for display.
        text: String,
    },
    /// A rolling-window day count outside 1..=365; both the constructor
    /// and period arithmetic reject it.
    #[error("rolling cost window of {days} days is outside 1..=365")]
    RollingWindowOutOfRange {
        /// Day count that was rejected.
        days: u16,
    },
    /// A cached rate is not exactly divisible into a per-token rate.
    #[error("rate for `{provider}/{model}` cannot be converted per token exactly")]
    RateNotConvertible {
        /// Provider whose catalog holds the rate.
        provider: String,
        /// Model whose rate cannot convert.
        model: String,
    },
    /// Exact integer cost arithmetic overflowed.
    #[error("cost arithmetic overflow")]
    CostOverflow,
    /// Cache directory or file I/O failed.
    #[error("pricing cache io: {reason}")]
    CacheIo {
        /// Display-safe reason; no cache content is echoed.
        reason: String,
    },
    /// The bounded HTTPS fetch failed.
    #[error("pricing fetch failed: {reason}")]
    Fetch {
        /// Display-safe reason; response bodies are never echoed.
        reason: String,
    },
    /// The response exceeded the size cap.
    #[error("pricing response exceeds {max_bytes} bytes")]
    FetchSizeLimit {
        /// The cap that was exceeded.
        max_bytes: usize,
    },
    /// The catalog JSON could not be parsed.
    #[error("pricing catalog is not valid json: {reason}")]
    CatalogMalformed {
        /// Display-safe parse reason.
        reason: String,
    },
    /// The fetched catalog looks empty or partial; the previous cache
    /// stays usable.
    #[error("fetched pricing catalog is not plausible: {reason}")]
    ImplausibleCatalog {
        /// Display-safe reason.
        reason: String,
    },
}

/// Result alias for cost-engine and pricing operations.
type CostResult<T> = Result<T, CostError>;

fn echo(text: &str) -> String {
    const LIMIT: usize = 32;
    if text.chars().count() <= LIMIT {
        return text.to_owned();
    }
    text.chars().take(LIMIT).collect()
}

/// An exact USD amount held as an integer count of 10^-12 (no float ever
/// touches cost math; llmproxy's unquoted-decimal loss is the precedent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Usd {
    picos: i128,
}

impl Usd {
    /// The zero amount.
    pub const ZERO: Self = Self { picos: 0 };

    /// Build from an exact 10^-12 count.
    pub const fn from_picos(picos: i128) -> Self {
        Self { picos }
    }

    /// The exact 10^-12 count.
    pub const fn as_picos(self) -> i128 {
        self.picos
    }

    /// Exact addition; `None` on overflow.
    pub fn checked_add(self, other: Self) -> Option<Self> {
        self.picos.checked_add(other.picos).map(Self::from_picos)
    }

    /// Parse an exact decimal such as `3`, `0.075`, or `4.2929e-2`.
    /// More than 12 fractional digits is an error, not a rounding.
    pub fn parse(text: &str) -> CostResult<Self> {
        let trimmed = text.trim();
        let (negative, unsigned) = match trimmed.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, trimmed),
        };
        let (mantissa_text, exponent_text) = match unsigned.split_once(['e', 'E']) {
            Some((mantissa, exponent)) => (mantissa, Some(exponent)),
            None => (unsigned, None),
        };
        let (int_digits, frac_digits) = match mantissa_text.split_once('.') {
            Some((i, f)) => (i, Some(f)),
            None => (mantissa_text, None),
        };
        if int_digits.is_empty() || int_digits.chars().any(|c| !c.is_ascii_digit()) {
            return Err(CostError::MalformedDecimal { text: echo(text) });
        }
        if let Some(frac) = frac_digits
            && (frac.is_empty() || frac.chars().any(|c| !c.is_ascii_digit()))
        {
            return Err(CostError::MalformedDecimal { text: echo(text) });
        }
        let mut digits = int_digits.to_owned();
        if let Some(frac) = frac_digits {
            digits.push_str(frac);
        }
        let mantissa: i128 = digits
            .parse()
            .map_err(|_int_err| CostError::MalformedDecimal { text: echo(text) })?;
        let exponent: i64 = match exponent_text {
            Some(e) => e
                .parse()
                .map_err(|_int_err| CostError::MalformedDecimal { text: echo(text) })?,
            None => 0,
        };
        let frac_len = i64::try_from(frac_digits.map_or(0, str::len)).unwrap_or(i64::MAX);
        let exp10 = exponent
            .checked_sub(frac_len)
            .ok_or(CostError::MalformedDecimal { text: echo(text) })?;
        let picos = scaled_picos(mantissa, exp10, negative, text)?;
        Ok(Self::from_picos(picos))
    }

    /// Parse a per-million-token rate; at most 6 fractional digits so the
    /// per-token conversion stays exact.
    pub fn parse_rate_per_mtok(text: &str) -> CostResult<Self> {
        let rate = Self::parse(text)?;
        if rate.picos % TOKENS_PER_MILLION != 0 {
            return Err(CostError::DecimalTooPrecise { text: echo(text) });
        }
        Ok(rate)
    }
}

/// Compute `mantissa * 10^exponent * PICOS_PER_USD` as an exact integer.
fn scaled_picos(mantissa: i128, exponent: i64, negative: bool, text: &str) -> CostResult<i128> {
    if mantissa == 0 {
        return Ok(0);
    }
    let malformed = || CostError::MalformedDecimal { text: echo(text) };
    let shift = exponent.checked_add(12).ok_or_else(malformed)?;
    let power = match u32::try_from(shift.unsigned_abs()) {
        Ok(p) => 10i128.checked_pow(p),
        Err(_) => None,
    };
    let magnitude = if shift >= 0 {
        power.and_then(|factor| mantissa.checked_mul(factor))
    } else {
        match power {
            Some(divisor) if mantissa % divisor == 0 => Some(mantissa / divisor),
            Some(_) => return Err(CostError::DecimalTooPrecise { text: echo(text) }),
            None => None,
        }
    }
    .ok_or_else(malformed)?;
    if negative {
        return magnitude.checked_neg().ok_or_else(malformed);
    }
    Ok(magnitude)
}

impl fmt::Display for Usd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let negative = self.picos < 0;
        let magnitude = self.picos.unsigned_abs();
        let integer = magnitude / PICOS_PER_USD.unsigned_abs();
        let fraction = magnitude % PICOS_PER_USD.unsigned_abs();
        if negative {
            f.write_str("-")?;
        }
        write!(f, "{integer}")?;
        if fraction > 0 {
            let digits = format!("{fraction:0>12}");
            let trimmed = digits.trim_end_matches('0');
            write!(f, ".{trimmed}")?;
        }
        Ok(())
    }
}

impl Serialize for Usd {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Usd {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// Token rates for one model, all USD per 1M tokens (models.dev units).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ModelRates {
    /// Input tokens.
    pub input: Usd,
    /// Output tokens.
    pub output: Usd,
    /// Cache-read tokens, when the provider prices them.
    pub cache_read: Option<Usd>,
    /// Cache-creation tokens, when the provider prices them.
    pub cache_write: Option<Usd>,
}

/// Provider- and model-scoped rates: two providers' same-named model never
/// share pricing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RateTable {
    providers: BTreeMap<String, BTreeMap<String, ModelRates>>,
}

struct BundledRate {
    provider: &'static str,
    model: &'static str,
    input: &'static str,
    output: &'static str,
    cache_read: Option<&'static str>,
    cache_write: Option<&'static str>,
}

/// First-party list prices fetched 2026-10-13 from the models.dev catalog
/// (anomalyco/models.dev, providers/{anthropic,openai}/models). Estimates,
/// never a bill.
const BUNDLED_RATES: &[BundledRate] = &[
    BundledRate {
        provider: "anthropic",
        model: "claude-haiku-4-5",
        input: "1",
        output: "5",
        cache_read: Some("0.1"),
        cache_write: Some("1.25"),
    },
    BundledRate {
        provider: "anthropic",
        model: "claude-opus-4-5",
        input: "5",
        output: "25",
        cache_read: Some("0.5"),
        cache_write: Some("6.25"),
    },
    BundledRate {
        provider: "anthropic",
        model: "claude-sonnet-4-5",
        input: "3",
        output: "15",
        cache_read: Some("0.3"),
        cache_write: Some("3.75"),
    },
    BundledRate {
        provider: "openai",
        model: "gpt-5-mini",
        input: "0.25",
        output: "2",
        cache_read: Some("0.025"),
        cache_write: None,
    },
    BundledRate {
        provider: "openai",
        model: "gpt-5.1",
        input: "1.25",
        output: "10",
        cache_read: Some("0.125"),
        cache_write: None,
    },
];

impl RateTable {
    /// Rates for one provider/model pair, when priced.
    pub fn lookup(&self, provider: &str, model: &str) -> Option<&ModelRates> {
        self.providers.get(provider)?.get(model)
    }

    /// Insert or replace one model's rates.
    pub fn insert(&mut self, provider: &str, model: &str, rates: ModelRates) {
        self.providers
            .entry(provider.to_owned())
            .or_default()
            .insert(model.to_owned(), rates);
    }

    /// Whether any rates are held.
    pub fn is_empty(&self) -> bool {
        self.providers.values().all(BTreeMap::is_empty)
    }

    /// The offline fallback table.
    pub fn bundled() -> CostResult<Self> {
        let mut table = Self::default();
        for rate in BUNDLED_RATES {
            table.insert(
                rate.provider,
                rate.model,
                ModelRates {
                    input: Usd::parse_rate_per_mtok(rate.input)?,
                    output: Usd::parse_rate_per_mtok(rate.output)?,
                    cache_read: rate.cache_read.map(Usd::parse_rate_per_mtok).transpose()?,
                    cache_write: rate.cache_write.map(Usd::parse_rate_per_mtok).transpose()?,
                },
            );
        }
        Ok(table)
    }

    /// Overlay `additive` (a fetched or cached models.dev catalog) on
    /// `self` (bundled): additive entries win, bundled fills the gaps.
    #[must_use]
    pub fn merge_additive(&self, additive: &Self) -> Self {
        let mut merged = additive.clone();
        for (provider, models) in &self.providers {
            merge_missing_provider(&mut merged, provider, models);
        }
        merged
    }

    /// Parse models.dev `api.json`. Unpriceable or over-precise models are
    /// skipped; they surface later as unpriced lookups.
    pub fn from_models_dev_json(bytes: &[u8]) -> CostResult<Self> {
        let mut table = Self::default();
        for (key, provider_raw) in api_providers(bytes)? {
            add_models_dev_provider(&mut table, key, provider_raw);
        }
        Ok(table)
    }

    /// A fetch is plausible only when both anchor providers carry priced
    /// models; this rejects empty or partial responses.
    fn is_plausible(&self) -> bool {
        ["anthropic", "openai"]
            .iter()
            .all(|id| self.providers.get(*id).is_some_and(|m| !m.is_empty()))
    }
}

#[derive(Deserialize)]
struct ApiWrapper<'a> {
    #[serde(borrow)]
    providers: BTreeMap<&'a str, &'a RawValue>,
}

#[derive(Deserialize)]
struct ApiProvider<'a> {
    #[serde(borrow)]
    id: Option<&'a str>,
    #[serde(borrow)]
    models: BTreeMap<&'a str, Option<&'a RawValue>>,
}

#[derive(Deserialize)]
struct ApiModel<'a> {
    #[serde(borrow)]
    cost: Option<&'a RawValue>,
}

fn merge_missing_provider(
    merged: &mut RateTable,
    provider: &str,
    models: &BTreeMap<String, ModelRates>,
) {
    for (model, rates) in models {
        if merged.lookup(provider, model).is_none() {
            merged.insert(provider, model, *rates);
        }
    }
}

fn add_models_dev_provider(table: &mut RateTable, key: &str, provider_raw: &RawValue) {
    let Ok(provider) = serde_json::from_str::<ApiProvider<'_>>(provider_raw.get()) else {
        return;
    };
    let provider_id = provider.id.unwrap_or(key).trim().to_ascii_lowercase();
    for (model_id, model_raw) in provider.models {
        if let Some(rates) = parse_model_rates(model_raw) {
            table.insert(&provider_id, model_id, rates);
        }
    }
}

fn parse_model_rates(model_raw: Option<&RawValue>) -> Option<ModelRates> {
    let model_raw = model_raw?;
    let model = serde_json::from_str::<ApiModel<'_>>(model_raw.get()).ok()?;
    api_rates(model.cost?.get()).ok()
}

#[derive(Deserialize)]
struct ApiCost<'a> {
    #[serde(borrow)]
    input: Option<&'a RawValue>,
    #[serde(borrow)]
    output: Option<&'a RawValue>,
    #[serde(borrow)]
    cache_read: Option<&'a RawValue>,
    #[serde(borrow)]
    cache_write: Option<&'a RawValue>,
}

/// Top-level provider map: flat (models.dev today) or wrapped in a
/// `providers` key.
fn api_providers(bytes: &[u8]) -> CostResult<Vec<(&'_ str, &'_ RawValue)>> {
    let malformed = |reason: String| CostError::CatalogMalformed { reason };
    let providers = match serde_json::from_slice::<ApiWrapper<'_>>(bytes) {
        Ok(wrapper) => wrapper.providers,
        Err(_) => serde_json::from_slice::<BTreeMap<&str, &RawValue>>(bytes)
            .map_err(|e| malformed(echo(&format!("{e}"))))?,
    };
    Ok(providers.into_iter().collect())
}

fn api_rates(cost_json: &str) -> CostResult<ModelRates> {
    let cost: ApiCost<'_> =
        serde_json::from_str(cost_json).map_err(|e| CostError::CatalogMalformed {
            reason: echo(&format!("{e}")),
        })?;
    let field = |raw: Option<&RawValue>| -> CostResult<Option<Usd>> {
        match raw {
            Some(value) => Usd::parse_rate_per_mtok(value.get()).map(Some),
            None => Ok(None),
        }
    };
    Ok(ModelRates {
        input: field(cost.input)?.ok_or(CostError::CatalogMalformed {
            reason: "model without input rate".to_owned(),
        })?,
        output: field(cost.output)?.ok_or(CostError::CatalogMalformed {
            reason: "model without output rate".to_owned(),
        })?,
        cache_read: field(cost.cache_read)?,
        cache_write: field(cost.cache_write)?,
    })
}

/// One cost-reporting period. A period's total is spend superai recorded,
/// never an invoice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostPeriod {
    /// Last N days, 1..=365.
    RollingDays {
        /// Window length in days.
        days: u16,
    },
    /// Calendar month so far in a named time zone.
    MonthToDate {
        /// IANA zone name the month boundary is pinned to.
        time_zone: String,
    },
    /// Every recorded event.
    AllHistory,
}

impl CostPeriod {
    /// Rolling window; USG-05 bounds the day count to 1..=365.
    pub fn rolling(days: u16) -> CostResult<Self> {
        if days == 0 || days > 365 {
            return Err(CostError::RollingWindowOutOfRange { days });
        }
        Ok(Self::RollingDays { days })
    }

    /// The half-open instant window [`PeriodRange::contains`] tests against.
    /// `bucket_zone` resolves rolling day boundaries; [`CostPeriod::MonthToDate`]
    /// ignores it and uses its own pinned zone.
    pub fn range(&self, now: Timestamp, bucket_zone: &str) -> CostResult<PeriodRange> {
        match self {
            Self::AllHistory => Ok(PeriodRange {
                start: None,
                end: now,
            }),
            Self::RollingDays { days } => {
                if *days == 0 || *days > 365 {
                    return Err(CostError::RollingWindowOutOfRange { days: *days });
                }
                let day_start = now
                    .to_zoned(resolve_zone(bucket_zone)?)
                    .start_of_day()
                    .map_err(|e| boundary_error(bucket_zone, &e))?;
                let window_start = day_start
                    .checked_sub(jiff::Span::new().days(i64::from(*days) - 1))
                    .map_err(|e| boundary_error(bucket_zone, &e))?;
                Ok(PeriodRange {
                    start: Some(window_start.timestamp()),
                    end: now,
                })
            }
            Self::MonthToDate { time_zone } => {
                let zone = resolve_zone(time_zone)?;
                let month_start = now
                    .to_zoned(zone)
                    .start_of_day()
                    .and_then(|day| day.with().day(1).build())
                    .map_err(|e| boundary_error(time_zone, &e))?;
                Ok(PeriodRange {
                    start: Some(month_start.timestamp()),
                    end: now,
                })
            }
        }
    }
}

fn resolve_zone(name: &str) -> CostResult<TimeZone> {
    TimeZone::get(name).map_err(|_zone_err| CostError::UnknownZone { name: echo(name) })
}

fn boundary_error(zone: &str, err: &jiff::Error) -> CostError {
    CostError::PeriodBoundary {
        zone: echo(zone),
        reason: echo(&format!("{err}")),
    }
}

/// The instants one cost period covers; `start: None` means all history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeriodRange {
    /// Inclusive first instant, `None` for all-history.
    pub start: Option<Timestamp>,
    /// Inclusive last instant, always the query's "now".
    pub end: Timestamp,
}

impl PeriodRange {
    /// Whether an event instant falls inside the period.
    pub fn contains(&self, ts: Timestamp) -> bool {
        self.start.is_none_or(|start| ts >= start) && ts <= self.end
    }
}

/// One recorded usage event; readers map harness artifacts into these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageEvent {
    /// When the event was recorded.
    pub timestamp: Timestamp,
    /// Provider the pricing lookup is scoped to.
    pub provider: String,
    /// Model id exactly as recorded.
    pub model: String,
    /// Input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Cache-read tokens, when the harness records them.
    pub cache_read_tokens: Option<u64>,
    /// Cache-creation tokens, when the harness records them.
    pub cache_write_tokens: Option<u64>,
}

/// Everything a reader scanned for one query, plus the records it could
/// not read; any nonzero gap makes every period total a floor.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecordedUsage {
    /// Parsed events.
    pub events: Vec<UsageEvent>,
    /// Lines or artifacts the reader skipped or failed to read.
    pub unscanned_records: u64,
}

/// Recorded spend for a scope. `Zero` means no priced or unpriced records
/// matched at all; a recorded zero-dollar amount is `Exact(Usd::ZERO)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Spend {
    /// No usage records matched the period.
    Zero,
    /// Every matched record is priced; the exact recorded sum. Recorded
    /// spend, never a bill.
    Exact(Usd),
    /// At least one matched record references a model (or a cache class
    /// of a model) with no known price; the aggregate cannot be
    /// represented. Entries are `provider/model`, sorted and unique.
    Unavailable {
        /// Models that blocked the aggregate.
        missing_pricing: Vec<String>,
    },
}

/// How completely the underlying scan covered its artifacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanCoverage {
    /// Every artifact was readable and parsed.
    Complete,
    /// The reader counted records it could not scan.
    Incomplete {
        /// Skipped or unreadable record count.
        unscanned_records: u64,
    },
}

/// A period total: what was recorded, and how completely.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostTotal {
    /// The recorded spend.
    pub spend: Spend,
    /// The scan's completeness.
    pub coverage: ScanCoverage,
}

/// One provider/model row of a period report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelCost {
    /// Provider the pricing lookup was scoped to.
    pub provider: String,
    /// Model id as recorded.
    pub model: String,
    /// Summed input tokens.
    pub input_tokens: u64,
    /// Summed output tokens.
    pub output_tokens: u64,
    /// Exact recorded cost, `None` when the model has no usable rates.
    pub cost: Option<Usd>,
    /// Event count.
    pub events: u64,
}

/// A cost query's answer for one period.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeriodReport {
    /// The period that was queried.
    pub period: CostPeriod,
    /// The instants the period covered.
    pub range: PeriodRange,
    /// The period total.
    pub total: CostTotal,
    /// Per provider/model rows, sorted by provider then model.
    pub models: Vec<ModelCost>,
}

/// Sums recorded events into period totals using exact decimal arithmetic.
#[derive(Debug, Clone, PartialEq)]
pub struct CostEngine {
    rates: RateTable,
}

impl CostEngine {
    /// Build over an explicit rate table (usually bundled merged with the
    /// cached models.dev catalog).
    pub fn new(rates: RateTable) -> Self {
        Self { rates }
    }

    /// Total one period. `bucket_zone` resolves rolling day boundaries and
    /// is ignored for [`CostPeriod::MonthToDate`].
    pub fn total(
        &self,
        period: &CostPeriod,
        now: Timestamp,
        bucket_zone: &str,
        usage: &RecordedUsage,
    ) -> CostResult<PeriodReport> {
        let range = period.range(now, bucket_zone)?;
        let mut groups: BTreeMap<(String, String), ModelCost> = BTreeMap::new();
        let mut missing: Vec<String> = Vec::new();
        let mut exact_total = Usd::ZERO;
        for event in &usage.events {
            if !range.contains(event.timestamp) {
                continue;
            }
            let entry = groups
                .entry((event.provider.clone(), event.model.clone()))
                .or_insert_with(|| ModelCost {
                    provider: event.provider.clone(),
                    model: event.model.clone(),
                    input_tokens: 0,
                    output_tokens: 0,
                    cost: Some(Usd::ZERO),
                    events: 0,
                });
            entry.input_tokens = entry
                .input_tokens
                .checked_add(event.input_tokens)
                .ok_or(CostError::CostOverflow)?;
            entry.output_tokens = entry
                .output_tokens
                .checked_add(event.output_tokens)
                .ok_or(CostError::CostOverflow)?;
            entry.events = entry.events.checked_add(1).ok_or(CostError::CostOverflow)?;
            let rates = self.rates.lookup(&event.provider, &event.model);
            let Some(cost) = rates.and_then(|rates| event_cost(rates, event).transpose()) else {
                entry.cost = None;
                continue;
            };
            let cost = cost?;
            if let Some(row_total) = entry.cost.as_mut() {
                *row_total = row_total.checked_add(cost).ok_or(CostError::CostOverflow)?;
            }
            exact_total = exact_total
                .checked_add(cost)
                .ok_or(CostError::CostOverflow)?;
        }
        let mut models: Vec<ModelCost> = groups.into_values().collect();
        for row in &mut models {
            if row.cost.is_none() {
                missing.push(format!("{}/{}", row.provider, row.model));
            }
        }
        let spend = if models.is_empty() {
            Spend::Zero
        } else if missing.is_empty() {
            Spend::Exact(exact_total)
        } else {
            Spend::Unavailable {
                missing_pricing: missing,
            }
        };
        let coverage = if usage.unscanned_records > 0 {
            ScanCoverage::Incomplete {
                unscanned_records: usage.unscanned_records,
            }
        } else {
            ScanCoverage::Complete
        };
        Ok(PeriodReport {
            period: period.clone(),
            range,
            total: CostTotal { spend, coverage },
            models,
        })
    }
}

/// Exact event cost; `None` when a consumed token class has no rate.
fn event_cost(rates: &ModelRates, event: &UsageEvent) -> CostResult<Option<Usd>> {
    let class_cost = |rate: Option<Usd>, tokens: Option<u64>| -> CostResult<Option<Usd>> {
        let Some(tokens) = tokens else {
            return Ok(Some(Usd::ZERO));
        };
        let Some(rate) = rate else {
            return Ok(None);
        };
        if rate.as_picos() % TOKENS_PER_MILLION != 0 {
            return Err(CostError::RateNotConvertible {
                provider: event.provider.clone(),
                model: event.model.clone(),
            });
        }
        i128::from(tokens)
            .checked_mul(rate.as_picos() / TOKENS_PER_MILLION)
            .map(|picos| Some(Usd::from_picos(picos)))
            .ok_or(CostError::CostOverflow)
    };
    let parts = [
        class_cost(Some(rates.input), Some(event.input_tokens))?,
        class_cost(Some(rates.output), Some(event.output_tokens))?,
        class_cost(rates.cache_read, event.cache_read_tokens)?,
        class_cost(rates.cache_write, event.cache_write_tokens)?,
    ];
    let mut sum = Usd::ZERO;
    for part in parts {
        let Some(part) = part else {
            return Ok(None);
        };
        sum = sum.checked_add(part).ok_or(CostError::CostOverflow)?;
    }
    Ok(Some(sum))
}

/// The cached models.dev catalog superai keeps in its own cache root.
#[derive(Debug, Clone, PartialEq)]
pub struct PricingCache {
    path: PathBuf,
}

/// Rates plus when they were fetched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CachedRates {
    /// Fetched-at instant.
    pub fetched_at: Timestamp,
    /// The fetched table, before any bundled merge.
    pub rates: RateTable,
}

#[derive(Serialize, Deserialize)]
struct CachedRatesDoc {
    schema_version: u16,
    fetched_at: Timestamp,
    rates: RateTable,
}

/// What the cache holds relative to the 24h TTL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Fetched within the TTL.
    Fresh {
        /// Fetched-at instant.
        fetched_at: Timestamp,
    },
    /// Older than the TTL; usable until a refresh succeeds.
    Stale {
        /// Fetched-at instant.
        fetched_at: Timestamp,
    },
    /// No readable cache.
    Missing,
}

impl PricingCache {
    /// Cache at `<superai_cache_root>/model-pricing/models-dev.json`.
    pub fn new(superai_cache_root: &Path) -> Self {
        Self {
            path: superai_cache_root
                .join("model-pricing")
                .join("models-dev.json"),
        }
    }

    /// superai's own cache root: `$XDG_CACHE_HOME/superai` else
    /// `$HOME/.cache/superai`. Never inside a harness config dir.
    pub fn default_root(xdg_cache_home: Option<&Path>, home: &Path) -> PathBuf {
        let base = xdg_cache_home.unwrap_or(&home.join(".cache")).to_path_buf();
        base.join("superai")
    }

    /// The cache file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the cache fresh from disk; `None` when missing, corrupt, or
    /// oversized.
    pub fn load(&self) -> Option<CachedRates> {
        let meta = std::fs::metadata(&self.path).ok()?;
        if meta.len() > MAX_CACHE_FILE_BYTES as u64 {
            return None;
        }
        let bytes = std::fs::read(&self.path).ok()?;
        let doc: CachedRatesDoc = serde_json::from_slice(&bytes).ok()?;
        (doc.schema_version == CACHE_SCHEMA_VERSION).then_some(CachedRates {
            fetched_at: doc.fetched_at,
            rates: doc.rates,
        })
    }

    /// Cache state against the TTL at `now`.
    pub fn freshness(&self, now: Timestamp) -> Freshness {
        let Some(cached) = self.load() else {
            return Freshness::Missing;
        };
        if now.as_second() < cached.fetched_at.as_second().saturating_add(CACHE_TTL_SECS) {
            Freshness::Fresh {
                fetched_at: cached.fetched_at,
            }
        } else {
            Freshness::Stale {
                fetched_at: cached.fetched_at,
            }
        }
    }

    /// Whether an unknown-model lookup may trigger a refresh yet; the
    /// cooldown runs from the cache's effective fetch time.
    pub fn unknown_model_refresh_allowed(&self, now: Timestamp) -> bool {
        let Some(cached) = self.load() else {
            return true;
        };
        now.as_second()
            >= cached
                .fetched_at
                .as_second()
                .saturating_add(UNKNOWN_MODEL_COOLDOWN_SECS)
    }

    /// Atomically replace the cache: temp file in the same directory,
    /// `rename(2)` over the target. A failed store leaves the previous
    /// cache intact.
    pub fn store(&self, rates: &RateTable, fetched_at: Timestamp) -> CostResult<()> {
        let doc = CachedRatesDoc {
            schema_version: CACHE_SCHEMA_VERSION,
            fetched_at,
            rates: rates.clone(),
        };
        let bytes = serde_json::to_vec(&doc).map_err(|e| CostError::CacheIo {
            reason: format!("encode: {e}"),
        })?;
        let parent = self.path.parent().ok_or_else(|| CostError::CacheIo {
            reason: "cache path has no parent directory".to_owned(),
        })?;
        std::fs::create_dir_all(parent).map_err(|e| CostError::CacheIo {
            reason: format!("create {}: {e}", parent.display()),
        })?;
        let temp = parent.join(format!(
            ".models-dev.json.tmp-{}-{}",
            fetched_at.as_second(),
            std::process::id()
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| CostError::CacheIo {
                reason: format!("create temp {}: {e}", temp.display()),
            })?;
        let write_result = file
            .write_all(&bytes)
            .and_then(|()| file.sync_all())
            .and_then(|()| std::fs::rename(&temp, &self.path));
        if let Err(e) = write_result {
            if let Err(cleanup) = std::fs::remove_file(&temp) {
                return Err(CostError::CacheIo {
                    reason: format!("store failed: {e}; temp cleanup also failed: {cleanup}"),
                });
            }
            return Err(CostError::CacheIo {
                reason: format!("store failed: {e}"),
            });
        }
        Ok(())
    }

    /// Fetch `url`, validate plausibility, and store atomically. On any
    /// failure the previous cache is left untouched and remains loadable.
    pub fn refresh(&self, url: &str, now: Timestamp) -> CostResult<CachedRates> {
        let bytes = fetch_models_dev_bytes(url)?;
        let rates = RateTable::from_models_dev_json(&bytes)?;
        if !rates.is_plausible() {
            return Err(CostError::ImplausibleCatalog {
                reason: "anthropic and openai must both carry priced models".to_owned(),
            });
        }
        let cached = CachedRates {
            fetched_at: now,
            rates,
        };
        self.store(&cached.rates, now)?;
        Ok(cached)
    }
}

/// Bounded fetch of the pricing catalog over core's shared client
/// (`https://` only, redirects re-validated per hop, size-capped,
/// timeout-bounded); `file://` reads local bytes for tests.
pub fn fetch_models_dev_bytes(url: &str) -> CostResult<Vec<u8>> {
    validate_fetch_url(url, "models.dev pricing").map_err(|e| CostError::Fetch {
        reason: echo(&format!("{e}")),
    })?;
    if let Some(path) = url.strip_prefix("file://") {
        let bytes = std::fs::read(path).map_err(|e| CostError::Fetch {
            reason: format!("read {path}: {e}"),
        })?;
        if bytes.len() > MAX_MODELS_DEV_BYTES {
            return Err(CostError::FetchSizeLimit {
                max_bytes: MAX_MODELS_DEV_BYTES,
            });
        }
        return Ok(bytes);
    }
    fetch_bounded(
        url,
        "models.dev pricing",
        &BoundedFetch {
            max_bytes: MAX_MODELS_DEV_BYTES,
            timeout: Duration::from_secs(FETCH_TIMEOUT_SECS),
        },
    )
    .map_err(|e| match e {
        TemplateFetchError::SizeLimit { .. } => CostError::FetchSizeLimit {
            max_bytes: MAX_MODELS_DEV_BYTES,
        },
        other => CostError::Fetch {
            reason: echo(&format!("{other}")),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolling_window_rejects_zero_and_days_beyond_365() {
        for days in [0u16, 366, u16::MAX] {
            assert!(matches!(
                CostPeriod::rolling(days),
                Err(CostError::RollingWindowOutOfRange { days: rejected })
                    if rejected == days
            ));
        }
    }

    #[test]
    fn rolling_window_accepts_both_documented_edges() {
        for days in [1u16, 365] {
            assert!(matches!(
                CostPeriod::rolling(days),
                Ok(CostPeriod::RollingDays { days: accepted }) if accepted == days
            ));
        }
    }

    #[test]
    fn usd_parse_and_display_round_trip_exactly() {
        for text in [
            "0",
            "1",
            "0.075",
            "3",
            "3.75",
            "0.1",
            "0.2",
            "0.3",
            "4.2929",
            "0.000000000001",
            "-1.25",
            "1000000",
        ] {
            let parsed = Usd::parse(text).unwrap_or_else(|e| panic!("`{text}` rejected: {e}"));
            assert_eq!(parsed.to_string(), text, "`{text}` did not round-trip");
        }
    }

    #[test]
    fn usd_parse_rejects_non_decimal_shapes() {
        for text in [
            "", "  ", ".5", "1.", "1.2.3", "abc", "1e", "1eX", "--1", "+1", "\"3\"",
        ] {
            assert!(
                Usd::parse(text).is_err(),
                "`{text}` should not parse as a decimal"
            );
        }
    }

    #[test]
    fn usd_parse_errors_on_extreme_exponents_instead_of_overflowing() {
        for text in [
            "0.1e-9223372036854775808",
            "1e-9223372036854775808",
            "0.1e9223372036854775807",
        ] {
            assert!(
                matches!(Usd::parse(text), Err(CostError::MalformedDecimal { .. })),
                "`{text}` must be a MalformedDecimal, not a panic or a wrap"
            );
        }
    }

    #[test]
    fn usd_addition_stays_exact_where_f64_would_drift() {
        let sum = [Usd::parse("0.1").unwrap(), Usd::parse("0.2").unwrap()]
            .into_iter()
            .try_fold(Usd::ZERO, Usd::checked_add)
            .unwrap();
        assert_eq!(sum.to_string(), "0.3");
    }

    #[test]
    fn usd_parse_rejects_precision_beyond_the_scale() {
        assert!(matches!(
            Usd::parse("0.0000000000001"),
            Err(CostError::DecimalTooPrecise { .. })
        ));
    }

    #[test]
    fn rate_parsing_allows_at_most_six_fractional_digits() {
        assert_eq!(
            Usd::parse_rate_per_mtok("0.123456").unwrap().to_string(),
            "0.123456"
        );
        assert_eq!(
            Usd::parse_rate_per_mtok("4.2929").unwrap().to_string(),
            "4.2929"
        );
        assert!(matches!(
            Usd::parse_rate_per_mtok("0.1234567"),
            Err(CostError::DecimalTooPrecise { .. })
        ));
    }

    #[test]
    fn exponent_notation_normalizes_exactly() {
        assert_eq!(Usd::parse("3e-6").unwrap().to_string(), "0.000003");
        assert_eq!(Usd::parse("1.25e2").unwrap().to_string(), "125");
        assert_eq!(Usd::parse("125e-2").unwrap().to_string(), "1.25");
    }

    #[test]
    fn bundled_rates_all_parse() {
        let table = RateTable::bundled().unwrap_or_else(|e| panic!("bundled table invalid: {e}"));
        assert!(table.lookup("anthropic", "claude-sonnet-4-5").is_some());
        assert!(table.lookup("openai", "gpt-5.1").is_some());
        assert!(!table.is_empty());
    }

    #[test]
    fn usd_serde_travels_as_an_exact_string() {
        let amount = Usd::parse("0.075").unwrap();
        let json = serde_json::to_string(&amount).unwrap();
        assert_eq!(json, "\"0.075\"");
        assert_eq!(serde_json::from_str::<Usd>(&json).unwrap(), amount);
    }
}
