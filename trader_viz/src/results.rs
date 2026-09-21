use std::{
    fs,
    path::{Path, PathBuf},
};

use execlab_core::{Manifest, SessionRow};
use serde::Serialize;

#[derive(Serialize)]
pub struct Performance {
    pub strategies: Vec<StrategyResult>,
    pub warnings: Vec<String>,
}

#[derive(Serialize)]
pub struct StrategyResult {
    pub alpha: String,
    pub hash: String,
    pub params: serde_json::Value,
    pub stats: Stats,
    pub intervals: Vec<IntervalResult>,
}

#[derive(Default, Serialize)]
pub struct Stats {
    pub runs: usize,
    pub mean_is_pct: Option<f64>,
    pub p15_is_pct: Option<f64>,
    pub p50_is_pct: Option<f64>,
    pub p90_is_pct: Option<f64>,
    pub completion_pct: Option<f64>,
    pub avg_fee: Option<f64>,
    pub avg_trades: Option<f64>,
    pub avg_percent_filled: Option<f64>,
    pub avg_mean_divergence: Option<f64>,
    pub avg_max_divergence: Option<f64>,
    pub status_pct: f64,
}

#[derive(Serialize)]
pub struct IntervalResult {
    pub session_id: String,
    pub session_start_ts: Option<i64>,
    pub session_ts: i64,
    pub arrival_mid_price: Option<f64>,
    pub implementation_shortfall_pct: Option<f64>,
    pub completion_pct: Option<f64>,
    pub fee: f64,
    pub mean_divergence_score: Option<f64>,
    pub max_divergence_score: Option<f64>,
    pub num_orders: usize,
    pub num_trades: i64,
    pub n_maker: usize,
    pub percent_filled: Option<f64>,
    pub avg_filled_price: Option<f64>,
    pub start_position: f64,
    pub final_inventory: f64,
    pub final_balance: f64,
    pub status_ok: bool,
    pub error: Option<String>,
    pub timeframe: String,
    pub symbol: String,
}

pub fn best_strategy(report: &Performance) -> Option<&StrategyResult> {
    report.strategies.iter().min_by(|a, b| {
        b.stats
            .completion_pct
            .unwrap_or(f64::NEG_INFINITY)
            .total_cmp(&a.stats.completion_pct.unwrap_or(f64::NEG_INFINITY))
            .then_with(|| {
                a.stats
                    .mean_is_pct
                    .unwrap_or(f64::INFINITY)
                    .total_cmp(&b.stats.mean_is_pct.unwrap_or(f64::INFINITY))
            })
            .then_with(|| {
                a.stats
                    .avg_mean_divergence
                    .unwrap_or(f64::INFINITY)
                    .total_cmp(&b.stats.avg_mean_divergence.unwrap_or(f64::INFINITY))
            })
            .then_with(|| a.hash.cmp(&b.hash))
    })
}

fn child_dirs(path: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<_> = fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs
}

fn percentile(values: &[f64], p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted
        .get(((sorted.len() - 1) as f64 * p).ceil() as usize)
        .copied()
}

fn completion(row: &SessionRow) -> Option<f64> {
    (row.start_position.is_finite()
        && row.start_position.abs() > f64::EPSILON
        && row.final_inventory.is_finite())
    .then_some(100.0 - row.final_inventory / row.start_position * 100.0)
}

fn interval(row: SessionRow, timeframe: String, symbol: String) -> IntervalResult {
    let percent_filled =
        (row.num_orders > 0).then_some(row.num_trades as f64 / row.num_orders as f64 * 100.0);
    let avg_filled_price =
        (row.trading_volume > 0.0).then_some(row.trading_value / row.trading_volume);
    let completion_pct = completion(&row);
    IntervalResult {
        session_id: row.session_id,
        session_start_ts: row.session_start_ts,
        session_ts: row.session_ts,
        arrival_mid_price: row.arrival_mid_price,
        implementation_shortfall_pct: row.implementation_shortfall_pct,
        completion_pct,
        fee: row.fee,
        mean_divergence_score: row.mean_divergence_score,
        max_divergence_score: row.max_divergence_score,
        num_orders: row.num_orders,
        num_trades: row.num_trades,
        n_maker: row.n_maker,
        percent_filled,
        avg_filled_price,
        start_position: row.start_position,
        final_inventory: row.final_inventory,
        final_balance: row.balance,
        status_ok: row.error.is_none(),
        error: row.error,
        timeframe,
        symbol,
    }
}

fn stats(intervals: &[IntervalResult]) -> Stats {
    let costs: Vec<_> = intervals
        .iter()
        .filter_map(|i| i.implementation_shortfall_pct)
        .collect();
    let completions: Vec<_> = intervals.iter().filter_map(|i| i.completion_pct).collect();
    let means: Vec<_> = intervals
        .iter()
        .filter_map(|i| i.mean_divergence_score)
        .collect();
    let maxes: Vec<_> = intervals
        .iter()
        .filter_map(|i| i.max_divergence_score)
        .collect();
    let trades: Vec<_> = intervals.iter().map(|i| i.num_trades as f64).collect();
    let fees: Vec<_> = intervals.iter().map(|i| i.fee).collect();
    let percent_filled: Vec<_> = intervals.iter().filter_map(|i| i.percent_filled).collect();
    let avg = |v: &[f64]| (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64);
    Stats {
        runs: intervals.len(),
        mean_is_pct: avg(&costs),
        p15_is_pct: percentile(&costs, 0.15),
        p50_is_pct: percentile(&costs, 0.5),
        p90_is_pct: percentile(&costs, 0.9),
        completion_pct: avg(&completions),
        avg_fee: avg(&fees),
        avg_trades: avg(&trades),
        avg_percent_filled: avg(&percent_filled),
        avg_mean_divergence: avg(&means),
        avg_max_divergence: avg(&maxes),
        status_pct: if intervals.is_empty() {
            0.0
        } else {
            intervals.iter().filter(|i| i.status_ok).count() as f64 / intervals.len() as f64 * 100.0
        },
    }
}

pub fn load(results: &Path) -> Result<Performance, String> {
    let alpha_dir = results.join("twap_sell");
    let mut strategies = Vec::new();
    let mut warnings = Vec::new();
    for dir in child_dirs(&alpha_dir) {
        let loaded = (|| -> Result<StrategyResult, String> {
            let manifest: Manifest = serde_json::from_str(
                &fs::read_to_string(dir.join("manifest.json")).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let mut intervals = Vec::new();
            for tf_dir in child_dirs(&dir) {
                if tf_dir.file_name().is_some_and(|v| v == "replay") {
                    continue;
                }
                let timeframe = tf_dir.file_name().unwrap().to_string_lossy().into_owned();
                for entry in fs::read_dir(&tf_dir).map_err(|e| e.to_string())?.flatten() {
                    let path = entry.path();
                    if path.extension().is_none_or(|v| v != "json") {
                        continue;
                    }
                    let symbol = path.file_stem().unwrap().to_string_lossy().into_owned();
                    let rows: Vec<SessionRow> = serde_json::from_str(
                        &fs::read_to_string(&path).map_err(|e| e.to_string())?,
                    )
                    .map_err(|e| e.to_string())?;
                    intervals.extend(
                        rows.into_iter()
                            .map(|row| interval(row, timeframe.clone(), symbol.clone())),
                    );
                }
            }
            intervals.sort_by_key(|i| i.session_start_ts.unwrap_or(i.session_ts));
            Ok(StrategyResult {
                alpha: manifest.alpha,
                hash: manifest.hash,
                params: manifest.params,
                stats: stats(&intervals),
                intervals,
            })
        })();
        match loaded {
            Ok(value) => strategies.push(value),
            Err(error) => warnings.push(format!("{}: {error}", dir.display())),
        }
    }
    strategies.sort_by(|a, b| {
        a.stats
            .mean_is_pct
            .unwrap_or(f64::INFINITY)
            .total_cmp(&b.stats.mean_is_pct.unwrap_or(f64::INFINITY))
    });
    if strategies.is_empty() {
        return Err(warnings
            .first()
            .cloned()
            .unwrap_or_else(|| "no completed strategy results".into()));
    }
    Ok(Performance {
        strategies,
        warnings,
    })
}

/// Validate that the sweep produced a complete, readable result set.
///
/// A session-level backtest error is still a valid result row and is exposed
/// as `FAIL` by Interval Explorer. Only missing configurations or interval
/// rows make the whole run unusable.
pub fn validate_complete(
    results: &Path,
    expected_configs: usize,
    expected_intervals: usize,
) -> Result<(), String> {
    let report = load(results)?;
    if report.strategies.len() != expected_configs {
        return Err(format!(
            "expected {expected_configs} configurations, found {}",
            report.strategies.len()
        ));
    }
    for strategy in &report.strategies {
        if strategy.intervals.len() != expected_intervals {
            return Err(format!(
                "{} expected {expected_intervals} intervals, found {}",
                strategy.hash,
                strategy.intervals.len()
            ));
        }
    }
    Ok(())
}
