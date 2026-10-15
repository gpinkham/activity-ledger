use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPrice {
    pub input_per_m: f64,
    pub output_per_m: f64,
    /// Cache hits. Not always 0.1x of input (0.05x on Opus 5.5, 0.025x on Fable 5.1),
    /// so each row carries its own; missing = the standard 0.1x.
    #[serde(default)]
    pub cache_read_per_m: Option<f64>,
    /// Fast-mode input/output rates, for the models that offer it.
    #[serde(default)]
    pub fast_input_per_m: Option<f64>,
    #[serde(default)]
    pub fast_output_per_m: Option<f64>,
    /// Rates for prompts over a size, on a model priced by prompt size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_prompt: Option<LongPrompt>,
    /// Where a row's numbers come from, when not from the published list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The input and output rates for a call whose prompt (input + cache write + cache
/// read) is over `above_tokens`; a prompt of exactly that size takes the row's rates.
/// Cache reads and writes keep the row's multipliers on the higher input rate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LongPrompt {
    pub above_tokens: i64,
    pub input_per_m: f64,
    pub output_per_m: f64,
}

impl ModelPrice {
    fn rates(&self, long: bool) -> (f64, f64) {
        match (long, &self.long_prompt) {
            (true, Some(l)) => (l.input_per_m, l.output_per_m),
            _ => (self.input_per_m, self.output_per_m),
        }
    }
}

/// Whether fast-mode rates apply: as each call recorded it (`speed: "fast"`),
/// never, or to every call on a model that has fast rates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FastMode {
    #[default]
    Auto,
    Never,
    Always,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceTable {
    pub models: HashMap<String, ModelPrice>,
    pub default: ModelPrice,
    /// US-only inference (`inference_geo: "us"`): 1.1x on every bucket. Off by default.
    #[serde(default)]
    pub us_only: bool,
    #[serde(default)]
    pub fast_mode: FastMode,
    /// When these prices were copied from the published list.
    #[serde(default)]
    pub prices_as_of: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TokenBreakdown {
    pub input: i64,
    pub output: i64,
    pub cache_creation_5m: i64,
    pub cache_creation_1h: i64,
    pub cache_read: i64,
    /// Server-side web searches (`usage.server_tool_use.web_search_requests`).
    pub web_searches: i64,
}

const CACHE_READ_MULT: f64 = 0.1;
const CACHE_WRITE_5M_MULT: f64 = 1.25;
const CACHE_WRITE_1H_MULT: f64 = 2.0;
const US_ONLY_MULT: f64 = 1.1;
/// The US-only premium applies to Claude 4.6 and later; these rows predate it.
const NO_GEO_PREMIUM: [&str; 7] =
    ["claude-opus-4-5", "claude-opus-4-1", "claude-opus-4", "claude-sonnet-4-5", "claude-sonnet-4", "claude-haiku-4-5", "claude-3-5-haiku"];
const WEB_SEARCH_USD: f64 = 10.0 / 1000.0;
/// Model ids recorded with `speed: "fast"` carry this suffix in the store, so every
/// rollup that groups by model prices fast calls separately without a new column.
pub const FAST_SUFFIX: &str = ":fast";

pub fn fast_key(model: &str) -> String {
    format!("{model}{FAST_SUFFIX}")
}

/// Rollups mark each call over its row's prompt size with this suffix (see
/// `model_sql`), after any FAST_SUFFIX, so long prompts are priced apart.
pub const LONG_SUFFIX: &str = ":long";

pub fn long_key(model: &str) -> String {
    format!("{model}{LONG_SUFFIX}")
}

/// A rollup's model key without the prompt-size mark, for showing and merging rows.
pub fn without_long(model: &str) -> &str {
    model.trim_end_matches(LONG_SUFFIX)
}

fn row(input: f64, output: f64, read: f64, fast: Option<(f64, f64)>) -> ModelPrice {
    ModelPrice {
        input_per_m: input,
        output_per_m: output,
        cache_read_per_m: Some(read),
        fast_input_per_m: fast.map(|f| f.0),
        fast_output_per_m: fast.map(|f| f.1),
        long_prompt: None,
        note: None,
    }
}

impl PriceTable {
    /// Standard direct-API list prices, copied from
    /// platform.claude.com/docs/en/about-claude/pricing on 2026-10-02, and Haiku 5.5
    /// added on 2026-10-15 from figures still to check against that page.
    pub fn seed() -> PriceTable {
        let mut models = HashMap::new();
        let mut add = |ids: &[&str], p: ModelPrice| {
            for id in ids {
                models.insert((*id).to_string(), p.clone());
            }
        };
        add(&["claude-fable-5-1", "claude-mythos-5-1"], row(10.0, 50.0, 0.25, None));
        add(&["claude-fable-5", "claude-mythos-5"], row(10.0, 50.0, 1.0, None));
        add(&["claude-opus-5-5"], row(4.0, 20.0, 0.20, Some((8.0, 40.0))));
        add(&["claude-opus-5", "claude-opus-4-8"], row(5.0, 25.0, 0.50, Some((10.0, 50.0))));
        add(&["claude-opus-4-7", "claude-opus-4-6", "claude-opus-4-5"], row(5.0, 25.0, 0.50, None));
        add(&["claude-opus-4-1", "claude-opus-4"], row(15.0, 75.0, 1.50, None));
        add(&["claude-sonnet-5-5", "claude-sonnet-5"], row(2.0, 10.0, 0.20, None));
        add(&["claude-sonnet-4-6", "claude-sonnet-4-5", "claude-sonnet-4"], row(3.0, 15.0, 0.30, None));
        add(&["claude-haiku-5-5"], ModelPrice {
            long_prompt: Some(LongPrompt { above_tokens: 100_000, input_per_m: 0.50, output_per_m: 2.50 }),
            note: Some("Supplied by the maintainer on 2026-10-15; verify against the provider's pricing page. \
                        Over 100,000 prompt tokens: $0.50 in, $2.50 out. No cache prices were given, so the \
                        standard multipliers (0.1x read, 1.25x and 2x writes) are an assumption."
                .into()),
            ..row(0.10, 0.50, 0.01, None)
        });
        add(&["claude-haiku-4-5"], row(1.0, 5.0, 0.10, None));
        add(&["claude-3-5-haiku"], row(0.80, 4.0, 0.08, None));
        PriceTable {
            models,
            default: row(5.0, 25.0, 0.50, None),
            us_only: false,
            fast_mode: FastMode::Auto,
            prices_as_of: Some("2026-10-15".into()),
        }
    }

    /// A saved table, or the seed when there is none or it does not parse.
    pub fn load_or_seed(path: &Path) -> PriceTable {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(PriceTable::seed)
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    /// The row for a model id: an exact key, else the longest key the id starts with
    /// (dated ids such as `claude-haiku-4-5-20251001`), else None.
    fn lookup(&self, model: &str) -> Option<(&str, &ModelPrice)> {
        if let Some((k, p)) = self.models.get_key_value(model) {
            return Some((k.as_str(), p));
        }
        self.models
            .iter()
            .filter(|(k, _)| model.starts_with(k.as_str()) && model[k.len()..].starts_with('-'))
            .max_by_key(|(k, _)| k.len())
            .map(|(k, p)| (k.as_str(), p))
    }

    pub fn is_known(&self, model: &str) -> bool {
        self.lookup(without_long(model).trim_end_matches(FAST_SUFFIX)).is_some()
    }

    /// SQL for the `model` column that adds LONG_SUFFIX to each call over its row's
    /// prompt size, for the rollups that sum calls by model before pricing them.
    /// Plain `model` while no row is priced by prompt size.
    pub fn model_sql(&self) -> String {
        let mut whens: Vec<String> = self
            .models
            .iter()
            .filter_map(|(k, p)| p.long_prompt.as_ref().map(|l| (k, l.above_tokens)))
            .map(|(k, above)| {
                let n = k.chars().count() + 1;
                let k = k.replace('\'', "''");
                format!(
                    "WHEN (model = '{k}' OR substr(model, 1, {n}) IN ('{k}-', '{k}:')) \
                     AND input_tokens + cache_creation + cache_read > {above} THEN model || '{LONG_SUFFIX}'"
                )
            })
            .collect();
        if whens.is_empty() {
            return "model".into();
        }
        whens.sort();
        format!("(CASE {} ELSE model END)", whens.join(" "))
    }

    fn price(&self, model: &str) -> &ModelPrice {
        self.lookup(model).map(|(_, p)| p).unwrap_or(&self.default)
    }

    fn geo_multiplier(&self, model: &str) -> f64 {
        let older = self.lookup(model).is_some_and(|(k, _)| NO_GEO_PREMIUM.contains(&k));
        if self.us_only && !older { US_ONLY_MULT } else { 1.0 }
    }

    pub fn cost(&self, model: &str, tb: &TokenBreakdown) -> f64 {
        let long = model.ends_with(LONG_SUFFIX);
        let model = without_long(model);
        let recorded_fast = model.ends_with(FAST_SUFFIX);
        let base = model.trim_end_matches(FAST_SUFFIX);
        let p = self.price(base);
        let fast = match self.fast_mode {
            FastMode::Auto => recorded_fast,
            FastMode::Never => false,
            FastMode::Always => true,
        };
        let read_mult = p.cache_read_per_m.map(|r| r / p.input_per_m).unwrap_or(CACHE_READ_MULT);
        let (in_rate, out_rate) = match (fast, p.fast_input_per_m, p.fast_output_per_m) {
            (true, Some(i), Some(o)) => (i, o),
            _ => p.rates(long),
        };
        let input = tb.input as f64 * in_rate;
        let output = tb.output as f64 * out_rate;
        let read = tb.cache_read as f64 * in_rate * read_mult;
        let write5 = tb.cache_creation_5m as f64 * in_rate * CACHE_WRITE_5M_MULT;
        let write1 = tb.cache_creation_1h as f64 * in_rate * CACHE_WRITE_1H_MULT;
        let geo = self.geo_multiplier(base);
        (input + output + read + write5 + write1) / 1_000_000.0 * geo + tb.web_searches as f64 * WEB_SEARCH_USD
    }

    pub fn saved(&self, model: &str, cache_read: i64) -> f64 {
        let long = model.ends_with(LONG_SUFFIX);
        let p = self.price(without_long(model).trim_end_matches(FAST_SUFFIX));
        let read_mult = p.cache_read_per_m.map(|r| r / p.input_per_m).unwrap_or(CACHE_READ_MULT);
        cache_read as f64 * p.rates(long).0 * (1.0 - read_mult) / 1_000_000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_input_output_cost() {
        let t = PriceTable::seed();
        let tb = TokenBreakdown { input: 1_000_000, output: 1_000_000, ..Default::default() };
        // 1M input @ $5 + 1M output @ $25 = $30.
        assert!((t.cost("claude-opus-4-8", &tb) - 30.0).abs() < 1e-9);
    }

    #[test]
    fn cache_multipliers_applied_per_bucket() {
        let t = PriceTable::seed();
        let tb = TokenBreakdown { cache_read: 1_000_000, cache_creation_5m: 1_000_000, cache_creation_1h: 1_000_000, ..Default::default() };
        // read: 1M*5*0.1=0.5, 5m: 1M*5*1.25=6.25, 1h: 1M*5*2.0=10.0 → 16.75 (/1M applied inside).
        assert!((t.cost("claude-opus-4-8", &tb) - 16.75).abs() < 1e-9);
    }

    #[test]
    fn unknown_model_uses_default() {
        let t = PriceTable::seed();
        let tb = TokenBreakdown { input: 1_000_000, ..Default::default() };
        assert!((t.cost("mystery-model", &tb) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn opus_5_5_cache_reads_use_their_own_rate() {
        let t = PriceTable::seed();
        // Opus 5.5 reads are $0.20/MTok (0.05x of $4), not the generic 0.1x.
        let tb = TokenBreakdown { cache_read: 1_000_000, ..Default::default() };
        assert!((t.cost("claude-opus-5-5", &tb) - 0.20).abs() < 1e-9);
        let fable = TokenBreakdown { cache_read: 1_000_000, ..Default::default() };
        assert!((t.cost("claude-fable-5-1", &fable) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn dated_model_ids_match_their_family_by_longest_prefix() {
        let t = PriceTable::seed();
        let tb = TokenBreakdown { input: 1_000_000, ..Default::default() };
        assert!((t.cost("claude-haiku-4-5-20251001", &tb) - 1.0).abs() < 1e-9);
        // claude-opus-5 is a prefix of claude-opus-5-5; the longer key must win.
        assert!((t.cost("claude-opus-5-5", &tb) - 4.0).abs() < 1e-9);
        assert!((t.cost("claude-opus-5-20260101", &tb) - 5.0).abs() < 1e-9);
        assert!(!t.is_known("mystery-model"));
        assert!(t.is_known("claude-haiku-4-5-20251001"));
    }

    #[test]
    fn fast_calls_use_fast_rates_and_cache_multipliers_on_top() {
        let t = PriceTable::seed();
        let tb = TokenBreakdown { input: 1_000_000, output: 1_000_000, cache_read: 1_000_000, cache_creation_5m: 1_000_000, ..Default::default() };
        // Opus 5.5 fast: $8 in, $40 out; read 0.05x of 8 = 0.40; 5m write 1.25x of 8 = 10.
        let fast = t.cost(&fast_key("claude-opus-5-5"), &tb);
        assert!((fast - (8.0 + 40.0 + 0.40 + 10.0)).abs() < 1e-9, "{fast}");
    }

    #[test]
    fn fast_mode_setting_overrides_the_transcript() {
        let mut t = PriceTable::seed();
        let tb = TokenBreakdown { input: 1_000_000, ..Default::default() };
        t.fast_mode = FastMode::Never;
        assert!((t.cost(&fast_key("claude-opus-5-5"), &tb) - 4.0).abs() < 1e-9);
        t.fast_mode = FastMode::Always;
        assert!((t.cost("claude-opus-5-5", &tb) - 8.0).abs() < 1e-9);
        // A model with no fast rate stays at its standard rate.
        assert!((t.cost("claude-haiku-4-5", &tb) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn us_only_inference_is_ten_percent_more_on_every_bucket() {
        let mut t = PriceTable::seed();
        t.us_only = true;
        let tb = TokenBreakdown { input: 1_000_000, output: 1_000_000, cache_read: 1_000_000, ..Default::default() };
        assert!((t.cost("claude-opus-5-5", &tb) - (4.0 + 20.0 + 0.20) * 1.1).abs() < 1e-9);
    }

    #[test]
    fn us_only_premium_applies_from_the_4_6_models_on() {
        let mut t = PriceTable::seed();
        t.us_only = true;
        let tb = TokenBreakdown { input: 1_000_000, ..Default::default() };
        assert!((t.cost("claude-haiku-4-5-20251001", &tb) - 1.0).abs() < 1e-9, "Haiku 4.5 predates 4.6");
        assert!((t.cost("claude-sonnet-4-5", &tb) - 3.0).abs() < 1e-9);
        assert!((t.cost("claude-sonnet-4-6", &tb) - 3.3).abs() < 1e-9);
    }

    #[test]
    fn web_searches_cost_ten_dollars_per_thousand() {
        let t = PriceTable::seed();
        let tb = TokenBreakdown { web_searches: 1000, ..Default::default() };
        assert!((t.cost("claude-opus-5-5", &tb) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn a_saved_table_round_trips_and_a_bad_file_falls_back_to_the_seed() {
        let dir = std::env::temp_dir().join(format!("ci_price_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("prices.json");
        let mut t = PriceTable::seed();
        t.us_only = true;
        t.save(&path).unwrap();
        assert!(PriceTable::load_or_seed(&path).us_only);
        std::fs::write(&path, "not json").unwrap();
        assert!(!PriceTable::load_or_seed(&path).us_only);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn haiku_5_5_is_priced_by_prompt_size() {
        let t = PriceTable::seed();
        let tb = TokenBreakdown { input: 1_000_000, output: 1_000_000, ..Default::default() };
        // Up to 100,000 prompt tokens: $0.10 in, $0.50 out. Over: $0.50 in, $2.50 out.
        assert!((t.cost("claude-haiku-5-5", &tb) - 0.60).abs() < 1e-9);
        assert!((t.cost(&long_key("claude-haiku-5-5"), &tb) - 3.0).abs() < 1e-9);
        assert!((t.cost(&long_key("claude-haiku-5-5-20261001"), &tb) - 3.0).abs() < 1e-9, "dated ids too");
        // Cache prices were not given: the standard 0.1x read and 1.25x / 2x writes, on the tier's input.
        let cache = TokenBreakdown { cache_read: 1_000_000, cache_creation_5m: 1_000_000, cache_creation_1h: 1_000_000, ..Default::default() };
        assert!((t.cost("claude-haiku-5-5", &cache) - (0.01 + 0.125 + 0.20)).abs() < 1e-9);
        assert!((t.cost(&long_key("claude-haiku-5-5"), &cache) - (0.05 + 0.625 + 1.0)).abs() < 1e-9);
        assert!((t.saved(&long_key("claude-haiku-5-5"), 1_000_000) - 0.45).abs() < 1e-9);
        assert!(t.is_known("claude-haiku-5-5-20261001") && t.is_known(&long_key("claude-haiku-5-5")));
        assert!(t.models["claude-haiku-5-5"].note.as_deref().is_some_and(|n| n.contains("assumption")));
    }

    #[test]
    fn the_long_mark_changes_nothing_on_a_row_without_a_prompt_size_price() {
        let t = PriceTable::seed();
        let tb = TokenBreakdown { input: 1_000_000, output: 1_000_000, cache_read: 1_000_000, ..Default::default() };
        for m in ["claude-opus-5-5", "claude-haiku-4-5", "mystery-model"] {
            assert!((t.cost(&long_key(m), &tb) - t.cost(m, &tb)).abs() < 1e-12, "{m}");
        }
        assert!((t.cost(&long_key(&fast_key("claude-opus-5-5")), &tb) - t.cost(&fast_key("claude-opus-5-5"), &tb)).abs() < 1e-12);
    }

    #[test]
    fn model_sql_is_the_plain_column_until_a_row_is_priced_by_prompt_size() {
        let mut t = PriceTable::seed();
        assert!(t.model_sql().contains("100000"));
        t.models.remove("claude-haiku-5-5");
        assert_eq!(t.model_sql(), "model");
        // A row without the new fields still loads, and saves without them.
        let old = r#"{"models":{"m":{"input_per_m":1.0,"output_per_m":2.0}},"default":{"input_per_m":5.0,"output_per_m":25.0}}"#;
        let t: PriceTable = serde_json::from_str(old).unwrap();
        let back = serde_json::to_string(&t.models["m"]).unwrap();
        assert!(!back.contains("long_prompt") && !back.contains("note"), "{back}");
    }

    #[test]
    fn saved_is_ninety_percent_of_full_read() {
        let t = PriceTable::seed();
        // 1M read @ $5 full would be $5; saved = $5 * 0.9 = $4.50.
        assert!((t.saved("claude-opus-4-8", 1_000_000) - 4.5).abs() < 1e-9);
    }
}
