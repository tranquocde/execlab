//! Renderer-neutral sweep-output loading and statistics.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use execlab_core::{Manifest, SessionRow};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Report {
    pub strategies: Vec<StrategyView>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct StrategyView {
    pub alpha: String,
    pub hash: String,
    pub source_path: String,
    pub params: serde_json::Value,
    pub backtest_config: serde_json::Value,
    pub strategy_source: String,
    pub stats: AggregateStats,
    pub assets: Vec<AssetView>,
}

#[derive(Debug, Serialize)]
pub struct AssetView {
    pub timeframe: String,
    pub asset: String,
    pub ok: bool,
    pub stats: AggregateStats,
    pub intervals: Vec<IntervalView>,
}

#[derive(Debug, Serialize)]
pub struct AggregateStats {
    pub runs: usize,
    pub mean_cost: Option<f64>,
    pub p15_cost: Option<f64>,
    pub p50_cost: Option<f64>,
    pub p90_cost: Option<f64>,
    pub avg_mean_divergence: Option<f64>,
    pub avg_max_divergence: Option<f64>,
    pub completion_pct: f64,
    pub status_pct: f64,
    pub fees: f64,
}

#[derive(Debug, Serialize)]
pub struct IntervalView {
    pub session_id: String,
    pub session_ts: i64,
    pub session_start_ts: Option<i64>,
    pub num_orders: usize,
    pub num_trades: i64,
    pub n_maker: usize,
    pub arrival_mid_price: Option<f64>,
    pub final_mid_price: Option<f64>,
    pub avg_filled_price: Option<f64>,
    pub percent_filled: Option<f64>,
    pub filled_cost: Option<f64>,
    pub filled_cost_pct: Option<f64>,
    pub residual_cost: Option<f64>,
    pub residual_cost_pct: Option<f64>,
    pub cost: Option<f64>,
    pub mean_divergence_score: Option<f64>,
    pub max_divergence_score: Option<f64>,
    pub completion_pct: Option<f64>,
    pub status_ok: bool,
    pub fees: f64,
    pub start_position: f64,
    pub final_balance: f64,
    pub final_inventory: f64,
}

fn child_dirs(path: &Path) -> Result<Vec<PathBuf>, String> {
    let mut children: Vec<PathBuf> = fs::read_dir(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|child| child.is_dir())
        .collect();
    children.sort();
    Ok(children)
}

/// Detects output-root, alpha, and exact-strategy paths by their contents.
fn resolve_input(path: &Path) -> Result<Vec<PathBuf>, String> {
    if path.join("manifest.json").is_file() {
        return Ok(vec![path.to_path_buf()]);
    }

    let children = child_dirs(path)?;
    let direct: Vec<PathBuf> = children
        .iter()
        .filter(|child| child.join("manifest.json").is_file())
        .cloned()
        .collect();
    if !direct.is_empty() {
        return Ok(direct);
    }

    let mut nested = Vec::new();
    for alpha in children {
        for candidate in child_dirs(&alpha)? {
            if candidate.join("manifest.json").is_file() {
                nested.push(candidate);
            }
        }
    }
    if nested.is_empty() {
        Err(format!(
            "{} is not an output root, alpha directory, or strategy directory",
            path.display()
        ))
    } else {
        Ok(nested)
    }
}

fn resolve_outputs(inputs: &[String]) -> Result<Vec<PathBuf>, String> {
    let mut strategies = BTreeSet::new();
    for input in inputs {
        let canonical =
            fs::canonicalize(input).map_err(|error| format!("cannot resolve {input}: {error}"))?;
        for strategy in resolve_input(&canonical)? {
            strategies.insert(
                fs::canonicalize(&strategy)
                    .map_err(|error| format!("cannot resolve {}: {error}", strategy.display()))?,
            );
        }
    }
    Ok(strategies.into_iter().collect())
}

fn interval_cost(row: &SessionRow) -> Option<f64> {
    row.implementation_shortfall_pct
}

fn interval_view(row: SessionRow) -> IntervalView {
    let avg_filled_price =
        (row.trading_volume > 0.0).then_some(row.trading_value / row.trading_volume);
    let percent_filled =
        (row.num_orders > 0).then_some(row.num_trades as f64 / row.num_orders as f64 * 100.0);
    let completion_pct = (row.start_position.is_finite()
        && row.start_position.abs() > f64::EPSILON
        && row.final_inventory.is_finite())
    .then_some(100.0 - row.final_inventory / row.start_position * 100.0);
    let cost = interval_cost(&row);

    IntervalView {
        session_id: row.session_id,
        session_ts: row.session_ts,
        session_start_ts: row.session_start_ts,
        num_orders: row.num_orders,
        num_trades: row.num_trades,
        n_maker: row.n_maker,
        arrival_mid_price: row.arrival_mid_price,
        final_mid_price: row.final_mid_price,
        avg_filled_price,
        percent_filled,
        filled_cost: row.filled_cost,
        filled_cost_pct: row.filled_cost_pct,
        residual_cost: row.residual_cost,
        residual_cost_pct: row.residual_cost_pct,
        cost,
        mean_divergence_score: row.mean_divergence_score,
        max_divergence_score: row.max_divergence_score,
        completion_pct,
        status_ok: row.error.is_none(),
        fees: row.fee,
        start_position: row.start_position,
        final_balance: row.balance,
        final_inventory: row.final_inventory,
    }
}

fn percentile(values: &[f64], percentile: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let index = ((sorted.len() - 1) as f64 * percentile).ceil() as usize;
    sorted.get(index).copied()
}

fn aggregate(intervals: &[&IntervalView]) -> AggregateStats {
    let costs: Vec<f64> = intervals.iter().filter_map(|row| row.cost).collect();
    let completions: Vec<f64> = intervals
        .iter()
        .filter_map(|row| row.completion_pct)
        .collect();
    let mean_divergences: Vec<f64> = intervals
        .iter()
        .filter_map(|row| row.mean_divergence_score)
        .collect();
    let max_divergences: Vec<f64> = intervals
        .iter()
        .filter_map(|row| row.max_divergence_score)
        .collect();
    let ok = intervals.iter().filter(|row| row.status_ok).count();
    AggregateStats {
        runs: intervals.len(),
        mean_cost: (!costs.is_empty()).then(|| costs.iter().sum::<f64>() / costs.len() as f64),
        p15_cost: percentile(&costs, 0.15),
        p50_cost: percentile(&costs, 0.50),
        p90_cost: percentile(&costs, 0.90),
        avg_mean_divergence: (!mean_divergences.is_empty())
            .then(|| mean_divergences.iter().sum::<f64>() / mean_divergences.len() as f64),
        avg_max_divergence: (!max_divergences.is_empty())
            .then(|| max_divergences.iter().sum::<f64>() / max_divergences.len() as f64),
        completion_pct: if completions.is_empty() {
            0.0
        } else {
            completions.iter().sum::<f64>() / completions.len() as f64
        },
        status_pct: if intervals.is_empty() {
            0.0
        } else {
            ok as f64 / intervals.len() as f64 * 100.0
        },
        fees: intervals.iter().map(|row| row.fees).sum(),
    }
}

fn load_strategy(path: &Path) -> Result<StrategyView, String> {
    let manifest_path = path.join("manifest.json");
    let manifest: Manifest = serde_json::from_str(
        &fs::read_to_string(&manifest_path)
            .map_err(|error| format!("cannot read {}: {error}", manifest_path.display()))?,
    )
    .map_err(|error| format!("cannot parse {}: {error}", manifest_path.display()))?;

    let mut assets = Vec::new();
    for timeframe_dir in child_dirs(path)? {
        if timeframe_dir
            .file_name()
            .is_some_and(|name| name == "replay")
        {
            continue;
        }
        let timeframe = timeframe_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let mut files: Vec<PathBuf> = fs::read_dir(&timeframe_dir)
            .map_err(|error| format!("cannot read {}: {error}", timeframe_dir.display()))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|file| {
                file.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect();
        files.sort();

        for file in files {
            let rows: Vec<SessionRow> = serde_json::from_str(
                &fs::read_to_string(&file)
                    .map_err(|error| format!("cannot read {}: {error}", file.display()))?,
            )
            .map_err(|error| format!("cannot parse {}: {error}", file.display()))?;
            let mut intervals: Vec<IntervalView> = rows.into_iter().map(interval_view).collect();
            intervals.sort_by_key(|interval| interval.session_ts);
            let refs: Vec<&IntervalView> = intervals.iter().collect();
            let stats = aggregate(&refs);
            assets.push(AssetView {
                timeframe: timeframe.clone(),
                asset: file.file_stem().unwrap().to_string_lossy().into_owned(),
                ok: !intervals.is_empty() && intervals.iter().all(|interval| interval.status_ok),
                stats,
                intervals,
            });
        }
    }
    assets.sort_by(|left, right| {
        (&left.timeframe, &left.asset).cmp(&(&right.timeframe, &right.asset))
    });
    let all_intervals: Vec<&IntervalView> = assets
        .iter()
        .flat_map(|asset| asset.intervals.iter())
        .collect();

    let mut strategy_stats = aggregate(&all_intervals);
    let asset_mean_divergences: Vec<f64> = assets
        .iter()
        .filter_map(|asset| asset.stats.avg_mean_divergence)
        .collect();
    let asset_max_divergences: Vec<f64> = assets
        .iter()
        .filter_map(|asset| asset.stats.avg_max_divergence)
        .collect();
    strategy_stats.avg_mean_divergence = (!asset_mean_divergences.is_empty())
        .then(|| asset_mean_divergences.iter().sum::<f64>() / asset_mean_divergences.len() as f64);
    strategy_stats.avg_max_divergence = (!asset_max_divergences.is_empty())
        .then(|| asset_max_divergences.iter().sum::<f64>() / asset_max_divergences.len() as f64);
    strategy_stats.status_pct = if assets.is_empty() {
        0.0
    } else {
        assets.iter().filter(|asset| asset.ok).count() as f64 / assets.len() as f64 * 100.0
    };

    Ok(StrategyView {
        alpha: manifest.alpha,
        hash: manifest.hash,
        source_path: path.to_string_lossy().into_owned(),
        params: manifest.params,
        backtest_config: manifest.backtest_config,
        strategy_source: fs::read_to_string(path.join("strategy.rs")).unwrap_or_default(),
        stats: strategy_stats,
        assets,
    })
}

pub fn output_process(inputs: &[String]) -> Result<Report, String> {
    let paths = resolve_outputs(inputs)?;
    let mut strategies = Vec::new();
    let mut warnings = Vec::new();

    for path in paths {
        match load_strategy(&path) {
            Ok(strategy) => strategies.push(strategy),
            Err(error) => warnings.push(error),
        }
    }
    strategies.sort_by(|left, right| {
        (&left.alpha, &left.hash, &left.source_path).cmp(&(
            &right.alpha,
            &right.hash,
            &right.source_path,
        ))
    });
    if strategies.is_empty() {
        return Err(format!(
            "no readable strategies found{}",
            warnings
                .first()
                .map(|warning| format!(": {warning}"))
                .unwrap_or_default()
        ));
    }
    Ok(Report {
        strategies,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::percentile;

    #[test]
    fn percentile_uses_nearest_rank() {
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0], 0.90), Some(4.0));
        assert_eq!(percentile(&[], 0.90), None);
    }
}
